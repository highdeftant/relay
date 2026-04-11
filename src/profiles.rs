use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub role: String,
    pub created: String,
    pub bio: String,
    pub skills: Vec<String>,
    pub color: String,
    pub avatar: String,
    pub avatar_file: Option<String>,
}
