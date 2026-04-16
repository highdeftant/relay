use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::{
    config::AppConfig,
    storage::{AgentPresence, MessageEvent},
    types::{AgentName, AgentRole, AgentStatus, AgentTask, ChannelName},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    Send {
        agent: AgentName,
        channel: ChannelName,
        message: String,
    },
    Join {
        agent: AgentName,
        role: Option<AgentRole>,
    },
    Heartbeat {
        agent: AgentName,
        status: Option<AgentStatus>,
        task: Option<AgentTask>,
    },
    List {
        channel: ChannelName,
        limit: Option<usize>,
    },
    Channels,
    Agents,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerResponse {
    Ok {
        message: String,
    },
    Error {
        message: String,
    },
    Messages {
        channel: String,
        events: Vec<MessageEvent>,
    },
    ChannelList {
        channels: Vec<String>,
    },
    Agents {
        agents: Vec<AgentPresence>,
    },
}

pub async fn send_message(
    config: AppConfig,
    agent: impl Into<AgentName>,
    channel: impl Into<ChannelName>,
    message: String,
) -> Result<()> {
    let agent = agent.into();
    let channel = channel.into();
    let response = send_request(
        &config,
        &ClientRequest::Send {
            agent,
            channel,
            message,
        },
    )
    .await?;

    handle_simple_response(response)
}

pub async fn send_message_quiet(
    config: &AppConfig,
    agent: &str,
    channel: &str,
    message: &str,
) -> Result<()> {
    let response = send_request(
        config,
        &ClientRequest::Send {
            agent: AgentName::from(agent),
            channel: ChannelName::from(channel),
            message: message.to_string(),
        },
    )
    .await?;

    expect_simple_ok(response)?;
    Ok(())
}

pub async fn join_agent(
    config: AppConfig,
    agent: impl Into<AgentName>,
    role: Option<impl Into<AgentRole>>,
) -> Result<()> {
    let agent = agent.into();
    let role = role.map(Into::into);
    let response = send_request(&config, &ClientRequest::Join { agent, role }).await?;
    handle_simple_response(response)
}

pub async fn heartbeat_agent(
    config: AppConfig,
    agent: impl Into<AgentName>,
    status: Option<impl Into<AgentStatus>>,
    task: Option<impl Into<AgentTask>>,
) -> Result<()> {
    let agent = agent.into();
    let status = status.map(Into::into);
    let task = task.map(Into::into);
    let response = send_request(
        &config,
        &ClientRequest::Heartbeat {
            agent,
            status,
            task,
        },
    )
    .await?;
    handle_simple_response(response)
}

pub async fn print_agents(config: AppConfig) -> Result<()> {
    let response = send_request(&config, &ClientRequest::Agents).await?;
    match response {
        ServerResponse::Agents { agents } => {
            if agents.is_empty() {
                println!("no agents connected");
                return Ok(());
            }

            for agent in agents {
                println!(
                    "{} role={:?} status={} task={:?} last_seen={}",
                    agent.name, agent.role, agent.status, agent.task, agent.last_seen_epoch
                );
            }
            Ok(())
        }
        ServerResponse::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

pub async fn list_messages(
    config: AppConfig,
    channel: impl Into<ChannelName>,
    limit: Option<usize>,
) -> Result<()> {
    let channel = channel.into();
    let response = send_request(&config, &ClientRequest::List { channel, limit }).await?;
    match response {
        ServerResponse::Messages { channel, events } => {
            if events.is_empty() {
                println!("no messages in #{channel}");
                return Ok(());
            }
            for event in &events {
                println!("[{}] {}: {}", event.timestamp, event.agent, event.message);
            }
            Ok(())
        }
        ServerResponse::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

pub async fn print_channels(config: AppConfig) -> Result<()> {
    let response = send_request(&config, &ClientRequest::Channels).await?;
    match response {
        ServerResponse::ChannelList { channels } => {
            if channels.is_empty() {
                println!("no channels");
                return Ok(());
            }
            for channel in &channels {
                println!("#{}", channel);
            }
            Ok(())
        }
        ServerResponse::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

pub async fn send_request(config: &AppConfig, request: &ClientRequest) -> Result<ServerResponse> {
    let mut stream = UnixStream::connect(&config.socket_path)
        .await
        .with_context(|| {
            format!(
                "failed to connect to relay unix socket at {}",
                config.socket_path.display()
            )
        })?;

    let payload = serde_json::to_string(request)?;
    stream.write_all(payload.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut response_line = String::new();
    let mut reader = BufReader::new(stream);
    let bytes = reader.read_line(&mut response_line).await?;
    if bytes == 0 {
        bail!("relay server closed connection without response");
    }

    let parsed = serde_json::from_str::<ServerResponse>(response_line.trim())
        .context("failed to parse server response")?;
    Ok(parsed)
}

fn handle_simple_response(response: ServerResponse) -> Result<()> {
    let message = expect_simple_ok(response)?;
    println!("{message}");
    Ok(())
}

fn expect_simple_ok(response: ServerResponse) -> Result<String> {
    match response {
        ServerResponse::Ok { message } => Ok(message),
        ServerResponse::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}
