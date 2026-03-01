use anyhow::{Context as _, Result};
use colored::Colorize as _;

use crate::mihomo::client::MihomoClient;

pub async fn delay(client: &MihomoClient, node: &str, url: &str, timeout: u64) -> Result<()> {
    let resp = client.get_proxy_delay(node, url, timeout).await?;
    println!("{}: {}", node.bold(), format_delay(resp.delay));
    Ok(())
}

pub async fn test(client: &MihomoClient, group: Option<&str>, url: &str) -> Result<()> {
    let proxies = client.get_proxies().await?;

    let group_name = match group {
        Some(g) => g.to_string(),
        None => {
            proxies.proxies.values()
                .find(|p| p.proxy_type == "Selector")
                .map(|p| p.name.clone())
                .context("no proxy groups found")?
        }
    };

    println!("Testing group '{}'...", group_name.bold());

    let result = client.get_group_delay(&group_name, url, 5000).await?;

    if let Some(obj) = result.as_object() {
        let mut entries: Vec<_> = obj.iter().collect();
        entries.sort_by(|a, b| {
            let da = a.1.as_u64().unwrap_or(u64::MAX);
            let db = b.1.as_u64().unwrap_or(u64::MAX);
            da.cmp(&db)
        });

        for (name, delay_val) in entries {
            let delay = delay_val.as_u64().unwrap_or(0);
            if delay > 0 {
                println!("  {:<40} {}", name, format_delay(delay));
            } else {
                println!("  {:<40} {}", name, "timeout".red());
            }
        }
    }
    Ok(())
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
