use anyhow::Result;
use colored::Colorize as _;
use futures_util::StreamExt as _;

use crate::mihomo::client::MihomoClient;

pub async fn traffic(client: &MihomoClient) -> Result<()> {
    println!("{}", "Real-time traffic (Ctrl+C to stop):".bold());
    let mut stream = client.stream_traffic().await?;
    while let Some(entry) = stream.next().await {
        match entry {
            Ok(t) => {
                print!("\r  \u{2191} {}/s  \u{2193} {}/s    ",
                    format_bytes(t.up).green(),
                    format_bytes(t.down).cyan()
                );
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
            }
            Err(e) => {
                eprintln!("\nStream error: {}", e);
                break;
            }
        }
    }
    println!();
    Ok(())
}

pub async fn conns(client: &MihomoClient, json_output: bool) -> Result<()> {
    let resp = client.get_connections().await?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    println!("Upload: {}  Download: {}",
        format_bytes(resp.upload_total).green(),
        format_bytes(resp.download_total).cyan()
    );

    if let Some(conns) = &resp.connections {
        println!("\n{} active connections:", conns.len());
        for conn in conns {
            let host = if conn.metadata.host.is_empty() {
                format!("{}:{}", conn.metadata.destination_ip, conn.metadata.destination_port)
            } else {
                format!("{}:{}", conn.metadata.host, conn.metadata.destination_port)
            };

            let chain = conn.chains.join(" -> ");
            println!("  {} {} [{}] ({})",
                host.bold(),
                conn.metadata.network.dimmed(),
                conn.rule.yellow(),
                chain.dimmed()
            );
        }
    }
    Ok(())
}

pub async fn closeall(client: &MihomoClient) -> Result<()> {
    client.close_all_connections().await?;
    println!("{} All connections closed", "✓".green());
    Ok(())
}

pub async fn log_stream(client: &MihomoClient, level: Option<&str>) -> Result<()> {
    println!("{}", "Streaming logs (Ctrl+C to stop):".bold());
    let mut stream = client.stream_logs(level).await?;
    while let Some(entry) = stream.next().await {
        match entry {
            Ok(log) => {
                let level_str = match log.level.as_str() {
                    "error" => "ERR".red().to_string(),
                    "warning" => "WRN".yellow().to_string(),
                    "info" => "INF".green().to_string(),
                    "debug" => "DBG".dimmed().to_string(),
                    other => other.to_string(),
                };
                println!("[{}] {}", level_str, log.payload);
            }
            Err(e) => {
                eprintln!("\nStream error: {}", e);
                break;
            }
        }
    }
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
