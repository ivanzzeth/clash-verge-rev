use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::config::app_config::AppConfig;
use crate::mihomo::client::MihomoClient;

pub async fn reload(client: &MihomoClient, config: &AppConfig) -> Result<()> {
    let output_path = config.resolved_output_path()?;
    let path_str = output_path.to_str().context("output path is not valid UTF-8")?;
    client.reload_config(path_str, true).await?;
    client.flush_dns().await?;
    println!("{} Config reloaded and DNS flushed", "✓".green());
    Ok(())
}
