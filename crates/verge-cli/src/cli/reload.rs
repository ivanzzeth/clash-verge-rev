use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::cli::apply;
use crate::config::app_config::AppConfig;
use crate::mihomo::client::MihomoClient;

pub async fn reload(client: &MihomoClient, config: &AppConfig) -> Result<()> {
    let output_path = config.resolved_output_path()?;
    let reload_path = apply::resolve_reload_path(&output_path)?;
    let path_str = reload_path.to_str().context("reload path is not valid UTF-8")?;
    client.reload_config(path_str, true).await?;
    match client.flush_dns().await {
        Ok(()) => println!("{} Config reloaded and DNS flushed", "✓".green()),
        Err(_) => println!("{} Config reloaded (DNS flush not supported)", "✓".green()),
    }
    Ok(())
}
