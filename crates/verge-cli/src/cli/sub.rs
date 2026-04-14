use anyhow::{Context as _, Result};
use colored::Colorize as _;
use qrcode::QrCode;

use crate::config::app_config::{AppConfig, Subscription};
use crate::model::clash::ClashConfig;
use crate::subscription::{fetcher, parser};

pub fn add(config: &AppConfig, config_path: Option<&std::path::Path>, name: &str, url: &str) -> Result<()> {
    let mut config = config.clone();

    // Check for duplicate
    if config.subscriptions.iter().any(|s| s.name == name) {
        anyhow::bail!("subscription '{}' already exists", name);
    }

    config.subscriptions.push(Subscription {
        name: name.to_string(),
        url: url.to_string(),
        enabled: true,
    });

    let save_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };
    config.save(&save_path)?;
    println!("{} Added subscription '{}'", "✓".green(), name);
    Ok(())
}

pub async fn update(config: &AppConfig, name: Option<&str>) -> Result<()> {
    let cache_dir = AppConfig::subscriptions_dir()?;

    let subs_to_update: Vec<&Subscription> = match name {
        Some(n) => {
            let sub = config.subscriptions.iter().find(|s| s.name == n)
                .with_context(|| format!("subscription '{}' not found", n))?;
            vec![sub]
        }
        None => config.subscriptions.iter().filter(|s| s.enabled).collect(),
    };

    if subs_to_update.is_empty() {
        println!("No subscriptions to update");
        return Ok(());
    }

    for sub in subs_to_update {
        print!("Updating '{}'... ", sub.name);
        match fetcher::fetch_subscription(sub, &cache_dir).await {
            Ok(path) => {
                // Parse to show stats
                match parser::parse_subscription(&path) {
                    Ok(parsed) => {
                        println!("{} ({} proxies, {} rules)",
                            "OK".green(),
                            parsed.proxies.len(),
                            parsed.rules.len()
                        );
                    }
                    Err(_) => println!("{}", "OK (cached)".green()),
                }
            }
            Err(e) => println!("{}: {}", "FAILED".red(), e),
        }
    }
    Ok(())
}

pub fn list(config: &AppConfig) -> Result<()> {
    if config.subscriptions.is_empty() {
        println!("No subscriptions configured");
        return Ok(());
    }

    let cache_dir = AppConfig::subscriptions_dir()?;

    println!("{:<20} {:<8} {}", "NAME".bold(), "STATUS".bold(), "URL".bold());
    for sub in &config.subscriptions {
        let cached = fetcher::cached_path(&sub.name, &cache_dir).exists();
        let status = if !sub.enabled {
            "disabled".yellow().to_string()
        } else if cached {
            "cached".green().to_string()
        } else {
            "pending".red().to_string()
        };
        println!("{:<20} {:<8} {}", sub.name, status, sub.url);
    }
    Ok(())
}

pub fn remove(config: &AppConfig, config_path: Option<&std::path::Path>, name: &str) -> Result<()> {
    let mut config = config.clone();
    let idx = config.subscriptions.iter().position(|s| s.name == name)
        .with_context(|| format!("subscription '{}' not found", name))?;
    config.subscriptions.remove(idx);

    // Remove cache
    let cache_dir = AppConfig::subscriptions_dir()?;
    fetcher::remove_cache(name, &cache_dir)?;

    let save_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };
    config.save(&save_path)?;
    println!("{} Removed subscription '{}'", "✓".green(), name);
    Ok(())
}

pub fn show(config: &AppConfig, name: &str) -> Result<()> {
    let _sub = config.subscriptions.iter().find(|s| s.name == name)
        .with_context(|| format!("subscription '{}' not found", name))?;

    let cache_dir = AppConfig::subscriptions_dir()?;
    let cache_path = fetcher::cached_path(name, &cache_dir);

    if !cache_path.exists() {
        anyhow::bail!("subscription '{}' not cached, run 'sub update' first", name);
    }

    let parsed = parser::parse_subscription(&cache_path)?;
    println!("{}: {}", "Subscription".bold(), name);
    println!("{}: {}", "Proxies".bold(), parsed.proxies.len());

    for proxy in &parsed.proxies {
        if let Some(pname) = ClashConfig::proxy_name(proxy) {
            let ptype = proxy.as_mapping()
                .and_then(|m| m.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!("  {} ({})", pname, ptype);
        }
    }

    println!("{}: {}", "Rules".bold(), parsed.rules.len());
    Ok(())
}

pub fn qr(config: &AppConfig, name: Option<&str>) -> Result<()> {
    let subs: Vec<&Subscription> = match name {
        Some(n) => {
            let sub = config.subscriptions.iter().find(|s| s.name == n)
                .with_context(|| format!("subscription '{}' not found", n))?;
            vec![sub]
        }
        None => config.subscriptions.iter().collect(),
    };

    if subs.is_empty() {
        println!("No subscriptions configured");
        return Ok(());
    }

    for (i, sub) in subs.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}: {}", "Subscription".bold(), sub.name);
        println!("{}: {}", "URL".bold(), sub.url);
        println!();

        let code = QrCode::new(sub.url.as_bytes())
            .with_context(|| format!("failed to encode URL for '{}' as QR code", sub.name))?;

        let string = code.render::<char>()
            .quiet_zone(true)
            .module_dimensions(2, 1)
            .build();

        println!("{}", string);
    }
    Ok(())
}
