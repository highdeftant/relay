use anyhow::Result;
use clap::Parser;
use relay::{
    cli::{Cli, Commands},
    config::AppConfig,
};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let config = AppConfig::from_default_paths()?;

    match cli.command {
        Commands::Serve => relay::server::serve(config).await?,
        Commands::Watch => relay::dashboard::watch(config).await?,
        Commands::Send {
            agent,
            channel,
            message,
        } => relay::protocol::send_message(config, agent, channel, message).await?,
        Commands::Join { agent, role } => relay::protocol::join_agent(config, agent, role).await?,
        Commands::Heartbeat {
            agent,
            status,
            task,
        } => relay::protocol::heartbeat_agent(config, agent, status, task).await?,
        Commands::Agents => relay::protocol::print_agents(config).await?,
        Commands::List { channel, limit } => {
            relay::protocol::list_messages(config, channel, limit).await?
        }
        Commands::Channels => relay::protocol::print_channels(config).await?,
        Commands::Init => relay::storage::init_layout(&config)?,
    }

    Ok(())
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .finish();

    if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("failed to initialize tracing: {error}");
    }
}
