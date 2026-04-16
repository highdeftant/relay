use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixListener},
    sync::RwLock,
};

use crate::{
    config::AppConfig,
    profiles::{load_hermes_admission_allowlist, normalize_agent_name},
    protocol::{ClientRequest, ServerResponse},
    storage::{AgentPresence, MessageEvent},
    types::{AgentName, AgentStatus},
};

#[derive(Clone)]
struct SharedState {
    config: AppConfig,
    agents: Arc<RwLock<HashMap<AgentName, AgentPresence>>>,
    allowed_agents: Arc<HashSet<String>>,
}

pub async fn serve(config: AppConfig) -> Result<()> {
    crate::storage::init_layout(&config)?;

    if config.socket_path.exists() {
        std::fs::remove_file(&config.socket_path)?;
    }

    let known_agents = crate::storage::load_agents(&config)?;
    let allowed_agents = Arc::new(load_allowed_agents(&config));

    let state = SharedState {
        config: config.clone(),
        agents: Arc::new(RwLock::new(known_agents)),
        allowed_agents,
    };

    let unix_listener = UnixListener::bind(&config.socket_path)?;
    let tcp_listener = TcpListener::bind(("127.0.0.1", config.tcp_port)).await?;

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
                            if let Err(error) = handle_stream(stream, state_clone).await {
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
                            if let Err(error) = handle_stream(stream, state_clone).await {
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
            if let Some(response) = ensure_agent_allowed(state, &agent) {
                return response;
            }

            if let Err(policy_error) = validate_channel_policy(&channel, &message) {
                return ServerResponse::Error {
                    message: policy_error,
                };
            }

            let event = MessageEvent::new(agent.clone(), channel, message);
            if let Err(error) = crate::storage::append_event(&state.config, &event) {
                return ServerResponse::Error {
                    message: format!("failed to append event: {error}"),
                };
            }

            let mut agents = state.agents.write().await;
            let entry = agents.entry(agent.clone()).or_insert_with(|| {
                AgentPresence::new(agent.clone(), None, AgentStatus::from("idle"), None)
            });
            entry.heartbeat(Some(AgentStatus::from("idle")), None);

            if let Some(response) = save_agents_or_error(&state.config, &agents) {
                return response;
            }

            ServerResponse::Ok {
                message: "message accepted".to_string(),
            }
        }
        ClientRequest::Join { agent, role } => {
            if let Some(response) = ensure_agent_allowed(state, &agent) {
                return response;
            }

            let mut agents = state.agents.write().await;
            let status = AgentStatus::from("online");
            let entry = agents.entry(agent.clone()).or_insert_with(|| {
                AgentPresence::new(agent.clone(), role.clone(), status.clone(), None)
            });
            entry.role = role;
            entry.heartbeat(Some(status), None);

            if let Some(response) = save_agents_or_error(&state.config, &agents) {
                return response;
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
            if let Some(response) = ensure_agent_allowed(state, &agent) {
                return response;
            }

            let mut agents = state.agents.write().await;
            let entry = agents.entry(agent.clone()).or_insert_with(|| {
                AgentPresence::new(agent.clone(), None, AgentStatus::from("online"), None)
            });
            entry.heartbeat(status, task);

            if let Some(response) = save_agents_or_error(&state.config, &agents) {
                return response;
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
        ClientRequest::List { channel, limit } => {
            let max = limit.unwrap_or(50);
            match crate::storage::load_channel_events(&state.config, &channel, max) {
                Ok(events) => ServerResponse::Messages {
                    channel: channel.into(),
                    events,
                },
                Err(error) => ServerResponse::Error {
                    message: format!("list failed: {error}"),
                },
            }
        }
        ClientRequest::Channels => match crate::storage::list_channels(&state.config) {
            Ok(channels) => ServerResponse::ChannelList { channels },
            Err(error) => ServerResponse::Error {
                message: format!("channels failed: {error}"),
            },
        },
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

fn load_allowed_agents(_config: &AppConfig) -> HashSet<String> {
    load_hermes_admission_allowlist()
}

fn is_agent_allowed(state: &SharedState, agent: &str) -> bool {
    let normalized = normalize_agent_name(agent);
    !normalized.is_empty() && state.allowed_agents.contains(&normalized)
}

fn ensure_agent_allowed(state: &SharedState, agent: &str) -> Option<ServerResponse> {
    if is_agent_allowed(state, agent) {
        return None;
    }

    Some(ServerResponse::Error {
        message: format!(
            "agent '{}' is not allowed (Relay allows only local Hermes profiles)",
            agent
        ),
    })
}

fn save_agents_or_error(
    config: &AppConfig,
    agents: &HashMap<AgentName, AgentPresence>,
) -> Option<ServerResponse> {
    match crate::storage::save_agents(config, agents) {
        Ok(()) => None,
        Err(error) => Some(ServerResponse::Error {
            message: format!("failed to persist agents: {error}"),
        }),
    }
}

fn validate_channel_policy(channel: &str, message: &str) -> Result<(), String> {
    if is_dm_channel(channel) {
        return Ok(());
    }

    let normalized_channel = channel.trim().to_lowercase();
    let normalized_message = message.trim().to_lowercase();

    let is_failure = contains_any(
        &normalized_message,
        &[
            "fail", "error", "panic", "down", "critical", "incident", "timeout",
        ],
    );
    let is_review = contains_any(
        &normalized_message,
        &[
            "review",
            "lgtm",
            "approve",
            "requested changes",
            "code review",
        ],
    );
    let is_status = contains_any(
        &normalized_message,
        &[
            "status", "handoff", "starting", "started", "online", "offline", "idle", "working",
        ],
    );

    if is_failure && normalized_channel != "alerts" {
        return Err("policy: failure/error messages must go to #alerts".to_string());
    }
    if is_review && normalized_channel != "review" {
        return Err("policy: review messages must go to #review".to_string());
    }
    if is_status && normalized_channel != "general" {
        return Err("policy: status/handoff messages must go to #general".to_string());
    }

    if normalized_channel == "alerts" && !is_failure {
        return Err("policy: #alerts is failures only".to_string());
    }

    Ok(())
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn is_dm_channel(channel: &str) -> bool {
    channel.trim().to_lowercase().starts_with("dm-")
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn policy_rejects_failure_outside_alerts() {
        let result = super::validate_channel_policy("dev", "build failed with error");
        assert!(result.is_err());
    }

    #[test]
    fn policy_accepts_failure_in_alerts() {
        let result = super::validate_channel_policy("alerts", "build failed with error");
        assert!(result.is_ok());
    }

    #[test]
    fn policy_rejects_review_outside_review_channel() {
        let result = super::validate_channel_policy("general", "code review requested");
        assert!(result.is_err());
    }

    #[test]
    fn policy_accepts_dm_anything() {
        let result = super::validate_channel_policy("dm-codex__hermes", "status update here");
        assert!(result.is_ok());
    }

    #[test]
    fn profile_allowlist_reads_and_normalizes_names() {
        let root =
            std::env::temp_dir().join(format!("relay-allowlist-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let profiles_path = root.join("profiles.json");

        let payload = r#"[
            {"name":"Hermes","role":"coordinator","created":"2026-01-01","bio":"","skills":[],"color":"cyan","avatar":"default","avatar_file":null},
            {"name":" Codex ","role":"coder","created":"2026-01-01","bio":"","skills":[],"color":"green","avatar":"default","avatar_file":null}
        ]"#;
        assert!(fs::write(&profiles_path, payload).is_ok());

        let allowed = crate::profiles::load_profile_allowlist(&profiles_path);
        assert!(allowed.contains("hermes"));
        assert!(allowed.contains("codex"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_agent_name_is_case_insensitive() {
        assert_eq!(crate::profiles::normalize_agent_name(" Hermes "), "hermes");
        assert_eq!(crate::profiles::normalize_agent_name("CoDeX"), "codex");
    }
}
