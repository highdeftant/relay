use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::{config::AppConfig, storage::AgentPresence};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    Send {
        agent: String,
        channel: String,
        message: String,
    },
    Join {
        agent: String,
        role: Option<String>,
    },
    Heartbeat {
        agent: String,
        status: Option<String>,
        task: Option<String>,
    },
    Agents,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerResponse {
    Ok { message: String },
    Error { message: String },
    Agents { agents: Vec<AgentPresence> },
    Pong,
}

pub async fn send_message(
    config: AppConfig,
    agent: String,
    channel: String,
    message: String,
) -> Result<()> {
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

pub async fn join_agent(config: AppConfig, agent: String, role: Option<String>) -> Result<()> {
    let response = send_request(&config, &ClientRequest::Join { agent, role }).await?;
    handle_simple_response(response)
}

pub async fn heartbeat_agent(
    config: AppConfig,
    agent: String,
    status: Option<String>,
    task: Option<String>,
) -> Result<()> {
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
    match response {
        ServerResponse::Ok { message } => {
            println!("{message}");
            Ok(())
        }
        ServerResponse::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}
