use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::config::app_config::{AppConfig, RuleFileRef};

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

pub fn ruleset_add(
    config: &AppConfig,
    config_path: Option<&std::path::Path>,
    name: &str,
    target: &str,
) -> Result<()> {
    // Check for duplicate name
    if config.rule_files.iter().any(|rf| {
        let existing_name = std::path::Path::new(&rf.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        existing_name == name
    }) {
        anyhow::bail!("rule set '{}' already exists in config", name);
    }

    let rules_dir = AppConfig::rules_dir()?;
    std::fs::create_dir_all(&rules_dir)
        .with_context(|| format!("failed to create rules directory: {}", rules_dir.display()))?;

    let rule_file = rules_dir.join(format!("{}.rules", name));
    if !rule_file.exists() {
        let header = format!("# {} rules\n# One rule payload per line. Target: {}\n# Examples:\n# DOMAIN-SUFFIX,example.com\n# DOMAIN-KEYWORD,example\n# IP-CIDR,10.0.0.0/8\n", name, target);
        std::fs::write(&rule_file, &header)
            .with_context(|| format!("failed to create rule file: {}", rule_file.display()))?;
    }

    let mut config = config.clone();
    config.rule_files.push(RuleFileRef {
        path: format!("~/.config/verge-cli/rules/{}.rules", name),
        target: target.to_string(),
    });

    let save_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };
    config.save(&save_path)?;
    println!(
        "{} Created rule set '{}' -> {} ({})",
        "✓".green(),
        name,
        target,
        rule_file.display()
    );
    Ok(())
}

pub fn ruleset_list(config: &AppConfig) {
    if config.rule_files.is_empty() {
        println!("No rule file references configured");
        return;
    }

    println!("{}", "Rule sets:".bold());
    for rf in &config.rule_files {
        let name = std::path::Path::new(&rf.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&rf.path);

        let line_count = crate::config::app_config::expand_tilde(&rf.path)
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|content| {
                content
                    .lines()
                    .filter(|l| {
                        let l = l.trim();
                        !l.is_empty() && !l.starts_with('#')
                    })
                    .count()
            })
            .unwrap_or(0);

        println!(
            "  {} -> {} ({} rules)",
            name.bold(),
            rf.target.cyan(),
            line_count
        );
    }
}

pub fn ruleset_remove(
    config: &AppConfig,
    config_path: Option<&std::path::Path>,
    name: &str,
) -> Result<()> {
    let mut config = config.clone();
    let before_len = config.rule_files.len();
    config.rule_files.retain(|rf| {
        let existing_name = std::path::Path::new(&rf.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        existing_name != name
    });

    if config.rule_files.len() == before_len {
        anyhow::bail!("rule set '{}' not found in config", name);
    }

    let save_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => AppConfig::default_config_path()?,
    };
    config.save(&save_path)?;

    // Check if file exists and inform user
    let rules_dir = AppConfig::rules_dir()?;
    let rule_file = rules_dir.join(format!("{}.rules", name));
    if rule_file.exists() {
        println!(
            "{} Removed rule set '{}' from config (file kept at {})",
            "✓".green(),
            name,
            rule_file.display()
        );
    } else {
        println!("{} Removed rule set '{}' from config", "✓".green(), name);
    }
    Ok(())
}

pub fn ruleset_show(name: &str) -> Result<()> {
    let rules_dir = AppConfig::rules_dir()?;
    let rule_file = rules_dir.join(format!("{}.rules", name));

    let content = std::fs::read_to_string(&rule_file)
        .with_context(|| format!("failed to read rule file: {}", rule_file.display()))?;

    println!("{}", format!("Rule set: {}", name).bold());
    print!("{}", content);
    Ok(())
}

pub fn ruleset_edit(name: &str) -> Result<()> {
    let rules_dir = AppConfig::rules_dir()?;
    let rule_file = rules_dir.join(format!("{}.rules", name));

    if !rule_file.exists() {
        anyhow::bail!(
            "rule file not found: {} (run 'rule set add {} <target>' first)",
            rule_file.display(),
            name
        );
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&rule_file)
        .status()
        .with_context(|| format!("failed to open editor: {}", editor))?;

    if !status.success() {
        anyhow::bail!("editor exited with non-zero status");
    }
    Ok(())
}
