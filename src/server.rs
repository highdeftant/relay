use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Result, anyhow};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
    sync::RwLock,
};

use crate::{
    config::AppConfig,
    protocol::{ClientRequest, ServerResponse},
    storage::{AgentPresence, MessageEvent},
};

#[derive(Clone)]
struct SharedState {
    config: AppConfig,
    agents: Arc<RwLock<HashMap<String, AgentPresence>>>,
}

pub async fn serve(config: AppConfig) -> Result<()> {
    crate::storage::init_layout(&config)?;

    if socket_exists(&config.socket_path) {
        std::fs::remove_file(&config.socket_path)?;
    }

    let known_agents = crate::storage::load_agents(&config)?;
    let state = SharedState {
        config: config.clone(),
        agents: Arc::new(RwLock::new(known_agents)),
    };

    let unix_listener = UnixListener::bind(&config.socket_path)?;
    let tcp_listener = TcpListener::bind(("0.0.0.0", config.tcp_port)).await?;

    tracing::info!(
        unix_socket = %config.socket_path.display(),
        tcp_port = config.tcp_port,
        "relay server started"
    );

    loop {
        tokio::select! {
            unix_accept = unix_listener.accept() => {
                match unix_accept {
                    Ok((stream, _addr)) => {
                        let state_clone = state.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_unix_connection(stream, state_clone).await {
                                tracing::warn!("unix client error: {error}");
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!("unix accept error: {error}");
                    }
                }
            }
            tcp_accept = tcp_listener.accept() => {
                match tcp_accept {
                    Ok((stream, addr)) => {
                        let state_clone = state.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_tcp_connection(stream, state_clone).await {
                                tracing::warn!(peer = %addr, "tcp client error: {error}");
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!("tcp accept error: {error}");
                    }
                }
            }
        }
    }
}

async fn handle_unix_connection(stream: UnixStream, state: SharedState) -> Result<()> {
    handle_stream(stream, state).await
}

async fn handle_tcp_connection(stream: TcpStream, state: SharedState) -> Result<()> {
    handle_stream(stream, state).await
}

async fn handle_stream<T>(stream: T, state: SharedState) -> Result<()>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (reader_half, mut writer_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader_half).lines();

    while let Some(line) = lines.next_line().await? {
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<ClientRequest>(raw) {
            Ok(req) => req,
            Err(error) => {
                let response = ServerResponse::Error {
                    message: format!("invalid request json: {error}"),
                };
                write_response(&mut writer_half, &response).await?;
                continue;
            }
        };

        let response = process_request(&state, request).await;
        write_response(&mut writer_half, &response).await?;
    }

    Ok(())
}

async fn process_request(state: &SharedState, request: ClientRequest) -> ServerResponse {
    match request {
        ClientRequest::Send {
            agent,
            channel,
            message,
        } => {
            let event = MessageEvent::new(agent.clone(), channel, message);
            if let Err(error) = crate::storage::append_event(&state.config, &event) {
                return ServerResponse::Error {
                    message: format!("failed to append event: {error}"),
                };
            }

            let mut agents = state.agents.write().await;
            let entry = agents.entry(agent.clone()).or_insert_with(|| {
                AgentPresence::new(agent.clone(), None, "idle".to_string(), None)
            });
            entry.heartbeat(Some("idle".to_string()), None);

            if let Err(error) = crate::storage::save_agents(&state.config, &agents) {
                return ServerResponse::Error {
                    message: format!("failed to persist agents: {error}"),
                };
            }

            ServerResponse::Ok {
                message: "message accepted".to_string(),
            }
        }
        ClientRequest::Join { agent, role } => {
            let mut agents = state.agents.write().await;
            let status = "online".to_string();
            let entry = agents.entry(agent.clone()).or_insert_with(|| {
                AgentPresence::new(agent.clone(), role.clone(), status.clone(), None)
            });
            entry.role = role;
            entry.heartbeat(Some(status), None);

            if let Err(error) = crate::storage::save_agents(&state.config, &agents) {
                return ServerResponse::Error {
                    message: format!("failed to persist agents: {error}"),
                };
            }

            ServerResponse::Ok {
                message: "agent joined".to_string(),
            }
        }
        ClientRequest::Heartbeat {
            agent,
            status,
            task,
        } => {
            let mut agents = state.agents.write().await;
            let entry = agents.entry(agent.clone()).or_insert_with(|| {
                AgentPresence::new(agent.clone(), None, "online".to_string(), None)
            });
            entry.heartbeat(status, task);

            if let Err(error) = crate::storage::save_agents(&state.config, &agents) {
                return ServerResponse::Error {
                    message: format!("failed to persist agents: {error}"),
                };
            }

            ServerResponse::Ok {
                message: "heartbeat accepted".to_string(),
            }
        }
        ClientRequest::Agents => {
            let agents = state.agents.read().await;
            let mut rows = agents.values().cloned().collect::<Vec<AgentPresence>>();
            rows.sort_by(|a, b| a.name.cmp(&b.name));
            ServerResponse::Agents { agents: rows }
        }
        ClientRequest::Ping => ServerResponse::Pong,
    }
}

async fn write_response<W>(writer: &mut W, response: &ServerResponse) -> Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let payload = serde_json::to_string(response)?;
    writer.write_all(payload.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn socket_exists(path: &Path) -> bool {
    path.exists()
}

#[allow(dead_code)]
fn into_error_response(error: anyhow::Error) -> ServerResponse {
    ServerResponse::Error {
        message: format!("{error}"),
    }
}

#[allow(dead_code)]
fn to_anyhow(message: &str) -> anyhow::Error {
    anyhow!(message.to_string())
}
