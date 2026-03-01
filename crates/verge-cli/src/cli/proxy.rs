use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::mihomo::client::MihomoClient;

pub async fn groups(client: &MihomoClient, json_output: bool) -> Result<()> {
    let resp = client.get_proxies().await?;

    if json_output {
        let groups: Vec<&crate::mihomo::types::ProxyInfo> = resp.proxies.values()
            .filter(|p| is_group_type(&p.proxy_type))
            .collect();
        println!("{}", serde_json::to_string_pretty(&groups).context("failed to serialize")?);
        return Ok(());
    }

    let mut groups: Vec<_> = resp.proxies.values()
        .filter(|p| is_group_type(&p.proxy_type))
        .collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));

    for group in groups {
        let now = group.now.as_deref().unwrap_or("-");
        let count = group.all.as_ref().map(|a| a.len()).unwrap_or(0);
        println!("{} ({}) [{} nodes] -> {}",
            group.name.bold(),
            group.proxy_type.dimmed(),
            count,
            now.cyan()
        );
    }
    Ok(())
}

pub async fn list(client: &MihomoClient, group: Option<&str>, json_output: bool) -> Result<()> {
    let resp = client.get_proxies().await?;

    let group_name = match group {
        Some(g) => g.to_string(),
        None => {
            // Find first selector group
            resp.proxies.values()
                .find(|p| p.proxy_type == "Selector")
                .map(|p| p.name.clone())
                .context("no proxy groups found")?
        }
    };

    let group_info = resp.proxies.get(&group_name)
        .with_context(|| format!("group '{}' not found", group_name))?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(group_info).context("failed to serialize")?);
        return Ok(());
    }

    let now = group_info.now.as_deref().unwrap_or("");

    println!("{} ({}):", group_name.bold(), group_info.proxy_type.dimmed());
    if let Some(nodes) = &group_info.all {
        for node in nodes {
            let marker = if node == now { " ←".cyan().to_string() } else { String::new() };
            // Get delay info if available
            if let Some(node_info) = resp.proxies.get(node.as_str()) {
                let delay = node_info.history.last().map(|h| h.delay).unwrap_or(0);
                let delay_str = if delay > 0 {
                    format_delay(delay)
                } else {
                    "-".dimmed().to_string()
                };
                println!("  {} [{}]{}", node, delay_str, marker);
            } else {
                println!("  {}{}", node, marker);
            }
        }
    }
    Ok(())
}

pub async fn now(client: &MihomoClient, group: Option<&str>, json_output: bool) -> Result<()> {
    let resp = client.get_proxies().await?;

    let group_name = match group {
        Some(g) => g.to_string(),
        None => {
            resp.proxies.values()
                .find(|p| p.proxy_type == "Selector")
                .map(|p| p.name.clone())
                .context("no proxy groups found")?
        }
    };

    let group_info = resp.proxies.get(&group_name)
        .with_context(|| format!("group '{}' not found", group_name))?;

    let current = group_info.now.as_deref().unwrap_or("none");

    if json_output {
        println!("{}", serde_json::json!({"group": group_name, "now": current}));
    } else {
        println!("{} -> {}", group_name.bold(), current.cyan());
    }
    Ok(())
}

pub async fn set(client: &MihomoClient, group: &str, node: &str) -> Result<()> {
    client.set_proxy(group, node).await?;
    println!("{} {} -> {}", "✓".green(), group.bold(), node.cyan());
    Ok(())
}

fn is_group_type(t: &str) -> bool {
    matches!(t, "Selector" | "URLTest" | "Fallback" | "LoadBalance" | "Relay")
}

fn format_delay(delay: u64) -> String {
    if delay < 200 {
        format!("{}ms", delay).green().to_string()
    } else if delay < 500 {
        format!("{}ms", delay).yellow().to_string()
    } else {
        format!("{}ms", delay).red().to_string()
    }
}
