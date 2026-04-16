use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "relay", version, about = "Agent chat and dashboard server")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start relay server
    Serve,
    /// Open local dashboard watcher
    Watch,
    /// Send a message to a channel
    Send {
        #[arg(short, long, default_value = "hermes")]
        agent: String,
        #[arg(short, long, default_value = "general")]
        channel: String,
        #[arg(short, long)]
        message: String,
    },
    /// Register an agent as online
    Join {
        #[arg(short, long)]
        agent: String,
        #[arg(long)]
        role: Option<String>,
    },
    /// Send heartbeat/status update
    Heartbeat {
        #[arg(short, long)]
        agent: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        task: Option<String>,
    },
    /// List recent messages in a channel
    List {
        #[arg(short, long, default_value = "general")]
        channel: String,
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// List available channels
    Channels,
    /// List known agent statuses
    Agents,
    /// Initialize local relay data layout
    Init,
}
