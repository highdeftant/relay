use std::{collections::HashMap, fs, io::Write, path::Path, time::SystemTime};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{config::AppConfig, profiles::AgentProfile};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub agent: String,
    pub channel: String,
    pub message: String,
    pub timestamp: String,
}

impl MessageEvent {
    pub fn new(
        agent: impl Into<String>,
        channel: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            agent: agent.into(),
            channel: channel.into(),
            message: message.into(),
            timestamp: unix_timestamp_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPresence {
    pub name: String,
    pub role: Option<String>,
    pub status: String,
    pub task: Option<String>,
    pub last_seen_epoch: u64,
}

impl AgentPresence {
    pub fn new(name: String, role: Option<String>, status: String, task: Option<String>) -> Self {
        Self {
            name,
            role,
            status,
            task,
            last_seen_epoch: unix_timestamp_secs(),
        }
    }

    pub fn heartbeat(&mut self, status: Option<String>, task: Option<String>) {
        if let Some(new_status) = status {
            self.status = new_status;
        }
        self.task = task;
        self.last_seen_epoch = unix_timestamp_secs();
    }
}

pub fn init_layout(config: &AppConfig) -> Result<()> {
    fs::create_dir_all(&config.data_dir)?;
    fs::create_dir_all(&config.channels_dir)?;
    fs::create_dir_all(&config.files_dir)?;
    fs::create_dir_all(&config.logs_dir)?;

    if !config.profiles_file.exists() {
        let defaults = default_profiles();
        let content = serde_json::to_string_pretty(&defaults)?;
        fs::write(&config.profiles_file, content)?;
    }

    if !config.agents_file.exists() {
        fs::write(&config.agents_file, "[]\n")?;
    }

    Ok(())
}

pub fn append_event(config: &AppConfig, event: &MessageEvent) -> Result<()> {
    let file_path = config.channels_dir.join(format!("{}.jsonl", event.channel));
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)?;

    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;

    Ok(())
}

pub fn load_agents(config: &AppConfig) -> Result<HashMap<String, AgentPresence>> {
    if !config.agents_file.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&config.agents_file)?;
    if content.trim().is_empty() {
        return Ok(HashMap::new());
    }

    let rows: Vec<AgentPresence> = serde_json::from_str(&content)?;
    let map = rows
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect::<HashMap<String, AgentPresence>>();

    Ok(map)
}

pub fn save_agents(config: &AppConfig, agents: &HashMap<String, AgentPresence>) -> Result<()> {
    ensure_parent(&config.agents_file)?;

    let mut rows = agents.values().cloned().collect::<Vec<AgentPresence>>();
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let content = serde_json::to_string_pretty(&rows)?;
    fs::write(&config.agents_file, format!("{content}\n"))?;
    Ok(())
}

pub fn load_channel_events(
    config: &AppConfig,
    channel: &str,
    limit: usize,
) -> Result<Vec<MessageEvent>> {
    let path = config.channels_dir.join(format!("{channel}.jsonl"));
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path)?;
    let mut events = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<MessageEvent>(trimmed) {
            events.push(event);
        }
    }

    if events.len() > limit {
        let keep_from = events.len().saturating_sub(limit);
        events.drain(0..keep_from);
    }

    Ok(events)
}

pub fn list_channels(config: &AppConfig) -> Result<Vec<String>> {
    let mut channels = vec![
        "general".to_string(),
        "ops".to_string(),
        "dev".to_string(),
        "review".to_string(),
        "alerts".to_string(),
        "research".to_string(),
    ];

    if config.channels_dir.exists() {
        for row in fs::read_dir(&config.channels_dir)? {
            let row = row?;
            if !row.file_type()?.is_file() {
                continue;
            }
            let name = row.file_name().to_string_lossy().to_string();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let channel = name.trim_end_matches(".jsonl").to_string();
            if channel.is_empty() {
                continue;
            }
            channels.push(channel);
        }
    }

    channels.sort();
    channels.dedup();
    Ok(channels)
}

fn default_profiles() -> Vec<AgentProfile> {
    vec![AgentProfile {
        name: "Hermes".to_string(),
        role: "coordinator".to_string(),
        created: "2026-04-10".to_string(),
        bio: "I route tasks, break ties, and pretend I know what's going on.".to_string(),
        skills: vec![
            "coordination".to_string(),
            "infrastructure".to_string(),
            "research".to_string(),
            "code review".to_string(),
        ],
        color: "cyan".to_string(),
        avatar: "default".to_string(),
        avatar_file: None,
    }]
}

fn unix_timestamp_secs() -> u64 {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn unix_timestamp_string() -> String {
    unix_timestamp_secs().to_string()
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
