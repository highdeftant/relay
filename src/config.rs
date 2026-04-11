use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub data_dir: PathBuf,
    pub channels_dir: PathBuf,
    pub files_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub profiles_file: PathBuf,
    pub agents_file: PathBuf,
    pub socket_path: PathBuf,
    pub tcp_port: u16,
    pub http_port: u16,
}

impl AppConfig {
    pub fn from_default_paths() -> Result<Self> {
        let home = std::env::var("HOME")?;
        let data_dir = PathBuf::from(home).join(".relay");

        Ok(Self {
            channels_dir: data_dir.join("channels"),
            files_dir: data_dir.join("files"),
            logs_dir: data_dir.join("logs"),
            profiles_file: data_dir.join("profiles.json"),
            agents_file: data_dir.join("agents.json"),
            socket_path: data_dir.join("relay.sock"),
            data_dir,
            tcp_port: 7777,
            http_port: 7778,
        })
    }
}
