use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::config::app_config::AppConfig;

pub fn init() -> Result<()> {
    let config_path = AppConfig::default_config_path()?;

    if config_path.exists() {
        anyhow::bail!("config already exists at {}", config_path.display());
    }

    let config = AppConfig::default();
    config.save(&config_path)?;
    println!("{} Created config at {}", "✓".green(), config_path.display());
    Ok(())
}

pub fn show(config_path: Option<&std::path::Path>) -> Result<()> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };

    if !path.exists() {
        anyhow::bail!("config not found at {}\nRun 'verge-cli config init' to create one", path.display());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    println!("{}", content);
    Ok(())
}

pub fn edit(config_path: Option<&std::path::Path>) -> Result<()> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };

    if !path.exists() {
        anyhow::bail!("config not found at {}\nRun 'verge-cli config init' to create one", path.display());
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to launch editor: {}", editor))?;

    if !status.success() {
        anyhow::bail!("editor exited with non-zero status");
    }

    // Validate the edited config
    AppConfig::load(&path).context("edited config is invalid YAML")?;
    println!("{} Config saved and validated", "✓".green());
    Ok(())
}
