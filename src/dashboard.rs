use anyhow::Result;

use crate::config::AppConfig;

pub async fn watch(config: AppConfig) -> Result<()> {
    tracing::info!(
        data_dir = %config.data_dir.display(),
        "dashboard skeleton entrypoint"
    );
    Ok(())
}
