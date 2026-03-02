use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::cli::backup;
use crate::config::app_config::AppConfig;
use crate::generator::merge;
use crate::mihomo::client::MihomoClient;

pub async fn apply(config: &AppConfig, client: &MihomoClient, force: bool) -> Result<()> {
    // Step 0: Backup current state before making changes
    backup::create_backup(config)?;

    // Step 1: Generate config
    println!("Generating config...");
    let final_config = merge::generate(config)?;
    let output_path = config.resolved_output_path()?;
    merge::write_config(&final_config, &output_path)?;
    println!("  {} Written to {}", "✓".green(), output_path.display());
    println!(
        "  Proxies: {}  Groups: {}  Rules: {}",
        final_config.proxies.len(),
        final_config.proxy_groups.len(),
        final_config.rules.len()
    );

    // Step 2: Copy to mihomo's safe path if needed
    // mihomo restricts config loading to its data directory
    let reload_path = resolve_reload_path(&output_path)?;
    if reload_path != output_path {
        std::fs::copy(&output_path, &reload_path).with_context(|| {
            format!(
                "failed to copy config to mihomo safe path: {}",
                reload_path.display()
            )
        })?;
        println!(
            "  {} Copied to {}",
            "✓".green(),
            reload_path.display()
        );
    }

    // Step 3: Reload mihomo
    println!("Reloading mihomo...");
    let path_str = reload_path
        .to_str()
        .context("output path is not valid UTF-8")?;
    client.reload_config(path_str, force).await?;

    // Step 4: Flush DNS (best-effort, not all mihomo versions support this)
    match client.flush_dns().await {
        Ok(()) => println!("  {} Config reloaded and DNS flushed", "✓".green()),
        Err(_) => println!("  {} Config reloaded (DNS flush not supported)", "✓".green()),
    }

    Ok(())
}

/// Resolve the path where mihomo can load the config from.
/// Clash Verge Rev restricts config paths to its data directory.
pub(crate) fn resolve_reload_path(output_path: &std::path::Path) -> Result<std::path::PathBuf> {
    // Check common mihomo data directories
    let data_dirs = [
        dirs::data_dir().map(|d| {
            d.join("io.github.clash-verge-rev.clash-verge-rev")
        }),
        dirs::data_local_dir().map(|d| {
            d.join("io.github.clash-verge-rev.clash-verge-rev")
        }),
    ];

    for dir in data_dirs.into_iter().flatten() {
        if dir.join("profiles").is_dir() {
            // If output_path is already under this directory, use it as-is
            if output_path.starts_with(&dir) {
                return Ok(output_path.to_path_buf());
            }
            // Otherwise, copy to profiles/verge-cli-generated.yaml
            return Ok(dir.join("profiles").join("verge-cli-generated.yaml"));
        }
    }

    // Fallback: use the output_path directly and hope mihomo accepts it
    Ok(output_path.to_path_buf())
}
