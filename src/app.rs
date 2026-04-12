//! Application state for the TUI dashboard.

use crate::storage::AgentPresence;

/// Which tab is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chat,
    Agents,
    Files,
    Logs,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Chat, Tab::Agents, Tab::Files, Tab::Logs];

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Chat => "CHAT",
            Tab::Agents => "AGENTS",
            Tab::Files => "FILES",
            Tab::Logs => "LOGS",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Chat => 0,
            Tab::Agents => 1,
            Tab::Files => 2,
            Tab::Logs => 3,
        }
    }

    pub fn from_index(i: usize) -> Tab {
        match i {
            0 => Tab::Chat,
            1 => Tab::Agents,
            2 => Tab::Files,
            3 => Tab::Logs,
            _ => Tab::Chat,
        }
    }

    pub fn next(self) -> Tab {
        Tab::from_index((self.index() + 1) % 4)
    }

    pub fn prev(self) -> Tab {
        Tab::from_index((self.index() + 3) % 4)
    }
}

/// Shared application state.
#[derive(Debug)]
pub struct AppState {
    pub active_tab: Tab,
    pub agents: Vec<AgentPresence>,
    pub channels: Vec<String>,
    pub messages: Vec<String>,
    pub logs: Vec<String>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            active_tab: Tab::Agents,
            agents: Vec::new(),
            channels: vec!["general".into()],
            messages: Vec::new(),
            logs: Vec::new(),
            should_quit: false,
        }
    }
}
