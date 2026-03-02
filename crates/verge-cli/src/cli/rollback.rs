use anyhow::{bail, Context as _, Result};
use colored::Colorize as _;
use std::path::Path;

use crate::cli::apply::resolve_reload_path;
use crate::cli::backup;
use crate::config::app_config::AppConfig;
use crate::mihomo::client::MihomoClient;

/// Rollback to a previous backup. If no ID is given, uses the latest backup.
pub async fn rollback(
    config: &AppConfig,
    client: &MihomoClient,
    id: Option<&str>,
) -> Result<()> {
    let backups_dir = AppConfig::backups_dir()?;

    let backup_id = match id {
        Some(id) => id.to_string(),
        None => {
            let latest = backup::latest_backup_id()?
                .context("no backups found; run 'verge-cli apply' first to create one")?;
            latest.to_string()
        }
    };

    let backup_dir = backups_dir.join(&backup_id);
    if !backup_dir.is_dir() {
        bail!("backup '{}' not found", backup_id);
    }

    println!("Rolling back to backup {}...", backup_id);

    // Step 1: Restore config.yaml
    let backup_config = backup_dir.join("config.yaml");
    if backup_config.exists() {
        let config_path = AppConfig::default_config_path()?;
        std::fs::copy(&backup_config, &config_path).with_context(|| {
            format!("failed to restore config.yaml to {}", config_path.display())
        })?;
        println!("  {} Restored config.yaml", "✓".green());
    }

    // Step 2: Restore rules directory
    let backup_rules = backup_dir.join("rules");
    if backup_rules.is_dir() {
        let rules_dir = AppConfig::rules_dir()?;
        // Remove current rules and replace with backup
        if rules_dir.is_dir() {
            std::fs::remove_dir_all(&rules_dir)
                .with_context(|| format!("failed to remove rules dir: {}", rules_dir.display()))?;
        }
        std::fs::create_dir_all(&rules_dir)
            .with_context(|| format!("failed to create rules dir: {}", rules_dir.display()))?;
        copy_dir_contents(&backup_rules, &rules_dir)
            .context("failed to restore rules directory")?;
        println!("  {} Restored rules/", "✓".green());
    }

    // Step 3: Restore generated.yaml
    let backup_generated = backup_dir.join("generated.yaml");
    if backup_generated.exists() {
        let output_path = config.resolved_output_path()?;
        std::fs::copy(&backup_generated, &output_path).with_context(|| {
            format!(
                "failed to restore generated.yaml to {}",
                output_path.display()
            )
        })?;
        println!(
            "  {} Restored generated.yaml to {}",
            "✓".green(),
            output_path.display()
        );

        // Copy to mihomo's safe path if needed
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

        // Step 4: Reload mihomo
        println!("Reloading mihomo...");
        let path_str = reload_path
            .to_str()
            .context("output path is not valid UTF-8")?;
        client.reload_config(path_str, true).await?;

        match client.flush_dns().await {
            Ok(()) => println!("  {} Config reloaded and DNS flushed", "✓".green()),
            Err(_) => println!(
                "  {} Config reloaded (DNS flush not supported)",
                "✓".green()
            ),
        }
    } else {
        println!(
            "  {} No generated.yaml in backup, skipping mihomo reload",
            "!".yellow()
        );
    }

    println!(
        "\n{} Rolled back to backup {}",
        "Done!".green().bold(),
        backup_id
    );

    Ok(())
}

/// Copy all files from src directory to dst directory (files only, non-recursive).
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).context("failed to read source directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let dest_file = dst.join(entry.file_name());
            std::fs::copy(&path, &dest_file).with_context(|| {
                format!(
                    "failed to copy {} -> {}",
                    path.display(),
                    dest_file.display()
                )
            })?;
        }
    }
    Ok(())
}
