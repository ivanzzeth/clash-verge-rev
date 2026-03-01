use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::config::app_config::AppConfig;

pub fn add(config: &AppConfig, config_path: Option<&std::path::Path>, rule: &str) -> Result<()> {
    // Validate rule format: TYPE,PAYLOAD,TARGET (basic check)
    if rule.splitn(3, ',').count() < 2 {
        anyhow::bail!("invalid rule format, expected TYPE,PAYLOAD,TARGET (e.g. DOMAIN-SUFFIX,example.com,Proxy)");
    }

    let mut config = config.clone();
    config.rules.push(rule.to_string());

    let save_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };
    config.save(&save_path)?;
    println!("{} Added rule: {}", "✓".green(), rule);
    Ok(())
}

pub fn list(config: &AppConfig) {
    if config.rules.is_empty() {
        println!("No custom rules configured");
        return;
    }

    println!("{}", "Custom rules:".bold());
    for (i, rule) in config.rules.iter().enumerate() {
        println!("  {}: {}", i, rule);
    }
}

pub fn remove(config: &AppConfig, config_path: Option<&std::path::Path>, target: &str) -> Result<()> {
    let mut config = config.clone();

    // Try to parse as index first
    if let Ok(idx) = target.parse::<usize>() {
        if idx >= config.rules.len() {
            anyhow::bail!("rule index {} out of range (0..{})", idx, config.rules.len());
        }
        let removed = config.rules.remove(idx);
        let save_path = match config_path {
            Some(p) => p.to_path_buf(),
            None => AppConfig::default_config_path()?,
        };
        config.save(&save_path)?;
        println!("{} Removed rule: {}", "✓".green(), removed);
    } else {
        // Remove by pattern match
        let before_len = config.rules.len();
        config.rules.retain(|r| !r.contains(target));
        let removed = before_len - config.rules.len();

        if removed == 0 {
            anyhow::bail!("no rules matching '{}'", target);
        }

        let save_path = match config_path {
            Some(p) => p.to_path_buf(),
            None => AppConfig::default_config_path()?,
        };
        config.save(&save_path)?;
        println!("{} Removed {} rule(s) matching '{}'", "✓".green(), removed, target);
    }
    Ok(())
}

pub fn import(config: &AppConfig, config_path: Option<&std::path::Path>, file: &std::path::Path) -> Result<()> {
    let content = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read file: {}", file.display()))?;

    let mut config = config.clone();
    let mut count = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        config.rules.push(line.to_string());
        count += 1;
    }

    let save_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };
    config.save(&save_path)?;
    println!("{} Imported {} rules from {}", "✓".green(), count, file.display());
    Ok(())
}
