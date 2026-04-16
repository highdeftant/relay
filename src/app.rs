//! Application state for the TUI dashboard.

use crate::{
    gateway_health::ProfileHealth,
    hermes::HermesSnapshot,
    storage::{AgentPresence, MessageEvent},
    types::UnixEpochSecs,
};

/// Which tab is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chat,
    Agents,
    Knowledge,
    Memory,
    System,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Chat,
        Tab::Agents,
        Tab::Knowledge,
        Tab::Memory,
        Tab::System,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Chat => "CHAT",
            Tab::Agents => "AGENTS",
            Tab::Knowledge => "KNOWLEDGE",
            Tab::Memory => "MEMORY",
            Tab::System => "SYSTEM",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Chat => 0,
            Tab::Agents => 1,
            Tab::Knowledge => 2,
            Tab::Memory => 3,
            Tab::System => 4,
        }
    }

    pub fn from_index(i: usize) -> Tab {
        match i {
            0 => Tab::Chat,
            1 => Tab::Agents,
            2 => Tab::Knowledge,
            3 => Tab::Memory,
            4 => Tab::System,
            _ => Tab::Chat,
        }
    }

    pub fn next(self) -> Tab {
        Tab::from_index((self.index() + 1) % 5)
    }

    pub fn prev(self) -> Tab {
        Tab::from_index((self.index() + 4) % 5)
    }
}

/// Shared application state.
#[derive(Debug)]
pub struct AppState {
    pub active_tab: Tab,
    pub agents: Vec<AgentPresence>,
    pub selected_agent: usize,
    pub channels: Vec<String>,
    pub active_channel: String,
    pub messages: Vec<MessageEvent>,
    pub chat_input: String,
    pub chat_agent: String,
    pub logs: Vec<String>,
    pub hermes_snapshot: HermesSnapshot,
    pub gateway_health: Vec<ProfileHealth>,
    pub should_quit: bool,
    pub last_refresh_unix: UnixEpochSecs,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            active_tab: Tab::Chat,
            agents: Vec::new(),
            selected_agent: 0,
            channels: vec!["general".into()],
            active_channel: "general".into(),
            messages: Vec::new(),
            chat_input: String::new(),
            chat_agent: "hermes".into(),
            logs: Vec::new(),
            hermes_snapshot: HermesSnapshot::default(),
            gateway_health: Vec::new(),
            should_quit: false,
            last_refresh_unix: 0,
        }
    }

    pub fn open_dm_with_selected(&mut self) {
        let Some(agent) = self.selected_agent_ref() else {
            return;
        };

        let sender = self.chat_agent.trim().to_string();
        let recipient = agent.name.trim().to_string();
        if sender.is_empty() || recipient.is_empty() || sender == recipient {
            return;
        }

        let mut parts = [sender, recipient];
        parts.sort();
        let channel = format!("dm-{}__{}", parts[0], parts[1]);

        if !self.channels.iter().any(|existing| existing == &channel) {
            self.channels.push(channel.clone());
            self.channels.sort();
        }

        self.active_channel = channel;
        self.active_tab = Tab::Chat;
    }

    pub fn clamp_selection(&mut self) {
        if self.agents.is_empty() {
            self.selected_agent = 0;
            return;
        }
        if self.selected_agent >= self.agents.len() {
            self.selected_agent = self.agents.len().saturating_sub(1);
        }
    }

    pub fn set_channels(&mut self, mut channels: Vec<String>) {
        if channels.is_empty() {
            channels.push("general".to_string());
        }
        channels.sort();
        channels.dedup();

        self.channels = channels;
        if !self.channels.iter().any(|c| c == &self.active_channel) {
            self.active_channel = self.channels.first().cloned().unwrap_or_default();
        }
    }

    pub fn select_next_channel(&mut self) {
        if self.channels.is_empty() {
            return;
        }
        let idx = self
            .channels
            .iter()
            .position(|c| c == &self.active_channel)
            .unwrap_or(0);
        let next_idx = (idx + 1) % self.channels.len();
        if let Some(next) = self.channels.get(next_idx) {
            self.active_channel = next.clone();
        }
    }

    pub fn select_prev_channel(&mut self) {
        if self.channels.is_empty() {
            return;
        }
        let idx = self
            .channels
            .iter()
            .position(|c| c == &self.active_channel)
            .unwrap_or(0);
        let prev_idx = (idx + self.channels.len() - 1) % self.channels.len();
        if let Some(prev) = self.channels.get(prev_idx) {
            self.active_channel = prev.clone();
        }
    }

    pub fn select_next_agent(&mut self) {
        if self.agents.is_empty() {
            return;
        }
        self.selected_agent = (self.selected_agent + 1) % self.agents.len();
    }

    pub fn select_prev_agent(&mut self) {
        if self.agents.is_empty() {
            return;
        }
        self.selected_agent = (self.selected_agent + self.agents.len() - 1) % self.agents.len();
    }

    pub fn selected_agent_ref(&self) -> Option<&AgentPresence> {
        self.agents.get(self.selected_agent)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppState, Tab};
    use crate::storage::AgentPresence;
    use crate::types::{AgentName, AgentRole, AgentStatus};

    #[test]
    fn default_state_starts_in_chat_tab() {
        let state = AppState::new();
        assert_eq!(state.active_tab, Tab::Chat);
    }

    #[test]
    fn open_dm_with_selected_creates_stable_channel_and_switches_to_chat() {
        let mut state = AppState::new();
        state.chat_agent = "hermes".to_string();
        state.agents = vec![AgentPresence::new(
            AgentName::from("codex"),
            Some(AgentRole::from("coder")),
            AgentStatus::from("online"),
            None,
        )];
        state.selected_agent = 0;

        state.open_dm_with_selected();

        assert_eq!(state.active_channel, "dm-codex__hermes");
        assert!(state.channels.iter().any(|c| c == "dm-codex__hermes"));
        assert_eq!(state.active_tab, Tab::Chat);
    }

    #[test]
    fn channel_cycle_wraps_forward_and_back() {
        let mut state = AppState::new();
        state.channels = vec![
            "general".to_string(),
            "dev".to_string(),
            "review".to_string(),
        ];
        state.active_channel = "general".to_string();

        state.select_next_channel();
        assert_eq!(state.active_channel, "dev");

        state.select_next_channel();
        assert_eq!(state.active_channel, "review");

        state.select_next_channel();
        assert_eq!(state.active_channel, "general");

        state.select_prev_channel();
        assert_eq!(state.active_channel, "review");
    }
}
