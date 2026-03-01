use anyhow::Result;
use colored::Colorize as _;

use crate::mihomo::client::MihomoClient;

pub async fn status(client: &MihomoClient, json_output: bool) -> Result<()> {
    let version = client.get_version().await?;
    let config = client.get_config().await?;
    let conns = client.get_connections().await?;

    if json_output {
        let status = serde_json::json!({
            "version": version.version,
            "mode": config.mode,
            "mixed_port": config.mixed_port,
            "log_level": config.log_level,
            "connections": conns.connections.as_ref().map(|c| c.len()).unwrap_or(0),
            "upload_total": conns.upload_total,
            "download_total": conns.download_total,
        });
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("{}", "mihomo status".bold());
    println!("  Version:     {}", version.version.cyan());
    println!("  Mode:        {}", config.mode.cyan());
    if let Some(port) = config.mixed_port {
        println!("  Mixed Port:  {}", port.to_string().cyan());
    }
    if let Some(level) = &config.log_level {
        println!("  Log Level:   {}", level.cyan());
    }
    println!("  Connections: {}", conns.connections.as_ref().map(|c| c.len()).unwrap_or(0));
    println!("  Upload:      {}", format_bytes(conns.upload_total));
    println!("  Download:    {}", format_bytes(conns.download_total));

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
