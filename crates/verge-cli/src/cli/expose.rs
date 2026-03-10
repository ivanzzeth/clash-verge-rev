//! Expose nodes as local HTTP/SOCKS5 proxies via GOST.
//!
//! Each node gets two local ports: SOCKS5 and HTTP.
//! State is stored in ~/.config/verge-cli/expose/state.json.

use anyhow::{Context as _, Result};
use base64::Engine;
use colored::Colorize as _;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;

use crate::config::app_config::AppConfig;
use crate::generator::merge;
use crate::model::clash::ClashConfig;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const GOST_RELEASE_URL: &str =
    "https://github.com/go-gost/gost/releases/download/v3.2.6/gost_3.2.6_linux_amd64.tar.gz";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const GOST_RELEASE_URL: &str =
    "https://github.com/go-gost/gost/releases/download/v3.2.6/gost_3.2.6_linux_arm64.tar.gz";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const GOST_RELEASE_URL: &str =
    "https://github.com/go-gost/gost/releases/download/v3.2.6/gost_3.2.6_darwin_amd64.tar.gz";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const GOST_RELEASE_URL: &str =
    "https://github.com/go-gost/gost/releases/download/v3.2.6/gost_3.2.6_darwin_arm64.tar.gz";
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
const GOST_RELEASE_URL: &str = "";

#[derive(Debug, Serialize, Deserialize)]
struct ExposeState {
    /// node_name -> (socks5_port, http_port, pid)
    nodes: HashMap<String, (u16, u16, u32)>,
    base_port: u16,
}

fn expose_state_path() -> Result<PathBuf> {
    let dir = AppConfig::config_dir()?.join("expose");
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir.join("state.json"))
}

fn load_state() -> Result<Option<ExposeState>> {
    let path = expose_state_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let state: ExposeState =
        serde_json::from_str(&content).with_context(|| "failed to parse expose state")?;
    Ok(Some(state))
}

fn save_state(state: &ExposeState) -> Result<()> {
    let path = expose_state_path()?;
    let content = serde_json::to_string_pretty(state).context("failed to serialize state")?;
    std::fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Parse --region "香港,日本" into list of substrings for matching
fn parse_region_filter(region: Option<&str>) -> Vec<String> {
    region
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Check if node name matches any region keyword (substring, case-insensitive for ASCII)
fn node_matches_region(name: &str, regions: &[String]) -> bool {
    if regions.is_empty() {
        return true;
    }
    let name_lower = name.to_lowercase();
    regions.iter().any(|r| {
        name.contains(r) || name_lower.contains(&r.to_lowercase())
    })
}

fn gost_binary_path() -> Result<PathBuf> {
    // Prefer PATH first
    if which::which("gost").is_ok() {
        return which::which("gost").context("gost in PATH");
    }
    // Then ~/.local/bin/gost
    let home = dirs::home_dir().context("no home dir")?;
    let local = home.join(".local").join("bin").join("gost");
    if local.exists() {
        return Ok(local);
    }
    anyhow::bail!(
        "gost not found. Run 'verge-cli expose start' to auto-install, or install manually:\n  \
         curl -fsSL https://github.com/go-gost/gost/raw/master/install.sh | bash -s -- --install"
    )
}

/// Convert Clash proxy YAML to GOST -F URI. Returns None if unsupported.
fn clash_proxy_to_gost_uri(proxy: &serde_yaml_ng::Value) -> Result<Option<String>> {
    let m = proxy
        .as_mapping()
        .context("proxy must be a mapping")?;
    let ty = m
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let server = m
        .get("server")
        .and_then(|v| v.as_str())
        .context("proxy missing 'server'")?;
    let port = m
        .get("port")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .context("proxy missing 'port'")? as u16;

    let uri = match ty.as_str() {
        "ss" => {
            let cipher = m
                .get("cipher")
                .or_else(|| m.get("method"))
                .and_then(|v| v.as_str())
                .unwrap_or("aes-256-gcm");
            let password = m
                .get("password")
                .and_then(|v| v.as_str())
                .context("ss proxy missing 'password'")?;
            let userinfo = format!("{}:{}", cipher, password);
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(userinfo.as_bytes());
            Some(format!("ss://{}@{}:{}", encoded, server, port))
        }
        "http" => {
            let username = m.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let password = m.get("password").and_then(|v| v.as_str()).unwrap_or("");
            let auth = if username.is_empty() && password.is_empty() {
                String::new()
            } else {
                format!("{}:{}@", username, password)
            };
            Some(format!("http://{}{}:{}", auth, server, port))
        }
        "https" => {
            let username = m.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let password = m.get("password").and_then(|v| v.as_str()).unwrap_or("");
            let auth = if username.is_empty() && password.is_empty() {
                String::new()
            } else {
                format!("{}:{}@", username, password)
            };
            Some(format!("https://{}{}:{}", auth, server, port))
        }
        "socks5" => {
            let username = m.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let password = m.get("password").and_then(|v| v.as_str()).unwrap_or("");
            let auth = if username.is_empty() && password.is_empty() {
                String::new()
            } else {
                format!("{}:{}@", username, password)
            };
            Some(format!("socks5://{}{}:{}", auth, server, port))
        }
        _ => None,
    };
    Ok(uri)
}

/// Ensure GOST is installed. If not, download to ~/.local/bin.
async fn ensure_gost_installed() -> Result<PathBuf> {
    if let Ok(p) = gost_binary_path() {
        return Ok(p);
    }

    if GOST_RELEASE_URL.is_empty() {
        anyhow::bail!(
            "GOST auto-install not supported on this platform. Install manually: \
             https://github.com/go-gost/gost/releases"
        );
    }

    eprintln!("{} GOST not found, installing to ~/.local/bin ...", "→".yellow());
    let home = dirs::home_dir().context("no home dir")?;
    let bin_dir = home.join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;

    let client = reqwest::Client::builder()
        .user_agent("verge-cli/0.1")
        .build()
        .context("failed to create HTTP client")?;

    let resp = client
        .get(GOST_RELEASE_URL)
        .send()
        .await
        .context("failed to download GOST")?;
    if !resp.status().is_success() {
        anyhow::bail!("GOST download failed: {}", resp.status());
    }
    let bytes = resp.bytes().await.context("failed to read GOST archive")?;

    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let gost_path = bin_dir.join("gost");
    for entry in archive.entries().context("invalid tar")? {
        let mut entry = entry.context("tar entry")?;
        let path = entry.path().context("entry path")?;
        if path.file_name().and_then(|n| n.to_str()) == Some("gost") {
            entry
                .unpack(&gost_path)
                .with_context(|| format!("failed to extract to {}", gost_path.display()))?;
            break;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&gost_path)
            .with_context(|| format!("metadata for {}", gost_path.display()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&gost_path, perms)
            .with_context(|| format!("chmod {}", gost_path.display()))?;
    }

    if !gost_path.exists() {
        anyhow::bail!("GOST extraction failed: binary not found");
    }
    eprintln!("{} Installed GOST to {}", "✓".green(), gost_path.display());
    Ok(gost_path)
}

/// Parse --protocol "ss,http" into set of upstream types (lowercase)
fn parse_protocol_filter(protocol: Option<&str>) -> HashSet<String> {
    protocol
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_lowercase())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Check if upstream URI type matches protocol filter
fn uri_matches_protocol(uri: &str, protocols: &HashSet<String>) -> bool {
    if protocols.is_empty() {
        return true;
    }
    let ty = uri.split("://").next().unwrap_or("").to_lowercase();
    protocols.contains(&ty)
}

pub async fn list(
    config: &AppConfig,
    region: Option<&str>,
    protocol: Option<&str>,
    format: &str,
    addr: &str,
) -> Result<()> {
    let clash = merge::generate(config)?;
    let state = load_state()?;
    let regions = parse_region_filter(region);
    let protocols = parse_protocol_filter(protocol);

    let nodes_to_show: Vec<_> = clash
        .proxies
        .iter()
        .filter_map(|p| {
            let name = ClashConfig::proxy_name(p)?.to_string();
            if !node_matches_region(&name, &regions) {
                return None;
            }
            let uri = clash_proxy_to_gost_uri(p).ok().flatten()?;
            if !uri_matches_protocol(&uri, &protocols) {
                return None;
            }
            let (socks_port, http_port, pid) = state
                .as_ref()
                .and_then(|s| s.nodes.get(&name).copied())
                .unwrap_or((0, 0, 0));
            Some((name, uri, socks_port, http_port, pid))
        })
        .collect();

    if nodes_to_show.is_empty() {
        println!("No exposable nodes (ss/http/https/socks5 only). Run 'sub update' first.");
        return Ok(());
    }

    match format {
        "comma" | "newline" => {
            let sep = if format == "comma" { "," } else { "\n" };
            let addrs: Vec<String> = nodes_to_show
                .iter()
                .filter(|(_, _, _socks_port, _http_port, pid)| *pid > 0)
                .map(|(_, _, socks_port, http_port, _)| {
                    let port = if addr == "http" { *http_port } else { *socks_port };
                    format!("127.0.0.1:{}", port)
                })
                .collect();
            println!("{}", addrs.join(sep));
        }
        _ => {
            println!(
                "{:<24} {:<8} {:<12} {:<12} {}",
                "NODE".bold(),
                "TYPE".bold(),
                "SOCKS5".bold(),
                "HTTP".bold(),
                "STATUS".bold()
            );
            for (name, uri, socks_port, http_port, pid) in &nodes_to_show {
                let ty = uri.split("://").next().unwrap_or("?").to_uppercase();
                let socks = if *socks_port > 0 {
                    format!("127.0.0.1:{}", socks_port)
                } else {
                    "-".to_string()
                };
                let http = if *http_port > 0 {
                    format!("127.0.0.1:{}", http_port)
                } else {
                    "-".to_string()
                };
                let status = if *pid > 0 {
                    "running".green().to_string()
                } else {
                    "stopped".dimmed().to_string()
                };
                println!("{:<24} {:<8} {:<12} {:<12} {}", name, ty, socks, http, status);
            }
        }
    }
    Ok(())
}

pub async fn start(
    config: &AppConfig,
    base_port: u16,
    nodes_filter: Option<&str>,
    region: Option<&str>,
) -> Result<()> {
    let gost_path = ensure_gost_installed().await?;

    let clash = merge::generate(config)?;
    let regions = parse_region_filter(region);

    let explicit_nodes: Option<std::collections::HashSet<String>> = nodes_filter.map(|s| {
        s.split(',')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect()
    });

    let mut to_expose: Vec<(String, String)> = Vec::new();
    for proxy in &clash.proxies {
        let name = match ClashConfig::proxy_name(proxy) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !node_matches_region(&name, &regions) {
            continue;
        }
        if let Some(ref explicit) = explicit_nodes {
            if !explicit.contains(&name) {
                continue;
            }
        }
        let uri = match clash_proxy_to_gost_uri(proxy)? {
            Some(u) => u,
            None => {
                eprintln!("{} Skipping '{}' (unsupported type)", "!".yellow(), name);
                continue;
            }
        };
        to_expose.push((name, uri));
    }

    if to_expose.is_empty() {
        anyhow::bail!("No nodes to expose. Check --nodes or run 'sub update'.");
    }

    // Stop existing nodes we're about to replace; merge with any other running nodes
    let mut new_state = if let Some(mut state) = load_state()? {
        for (name, _) in &to_expose {
            if let Some((_, _, pid)) = state.nodes.remove(name) {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
            }
        }
        state.base_port = base_port;
        state
    } else {
        ExposeState {
            nodes: HashMap::new(),
            base_port,
        }
    };

    let base_idx = new_state.nodes.len();
    for (i, (name, uri)) in to_expose.iter().enumerate() {
        let socks_port = base_port + (base_idx + i) as u16 * 2;
        let http_port = base_port + (base_idx + i) as u16 * 2 + 1;

        let child = tokio::process::Command::new(&gost_path)
            .args([
                "-L",
                &format!("socks5://127.0.0.1:{}", socks_port),
                "-L",
                &format!("http://127.0.0.1:{}", http_port),
                "-F",
                uri,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start gost for {}", name))?;

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let pid = child.id().context("gost process has no pid")?;
        new_state.nodes.insert(name.clone(), (socks_port, http_port, pid));

        println!(
            "{} {} -> socks5://127.0.0.1:{}  http://127.0.0.1:{}",
            "✓".green(),
            name,
            socks_port,
            http_port
        );
    }

    save_state(&new_state)?;
    println!("\n{} {} node(s) exposed. Use 'verge-cli expose stop' to stop.", "✓".green(), new_state.nodes.len());
    Ok(())
}

pub async fn stop(region: Option<&str>) -> Result<()> {
    let mut state = match load_state()? {
        Some(s) => s,
        None => {
            println!("No expose state found (nothing was started)");
            return Ok(());
        }
    };

    let regions = parse_region_filter(region);
    let to_stop: Vec<_> = state
        .nodes
        .iter()
        .filter(|(name, _)| node_matches_region(name, &regions))
        .map(|(name, (_, _, pid))| (name.clone(), *pid))
        .collect();

    for (name, pid) in &to_stop {
        state.nodes.remove(name);
        if std::process::Command::new("kill")
            .arg(pid.to_string())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            println!("{} Stopped {}", "✓".green(), name);
        }
    }

    if state.nodes.is_empty() {
        std::fs::remove_file(expose_state_path()?).ok();
    } else {
        save_state(&state)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clash_ss_to_gost_uri() {
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            r#"
name: test-ss
type: ss
server: 1.2.3.4
port: 8388
cipher: aes-256-gcm
password: secret
"#,
        )
        .unwrap();
        let uri = clash_proxy_to_gost_uri(&proxy).unwrap();
        assert!(uri.is_some());
        let u = uri.unwrap();
        assert!(u.starts_with("ss://"));
        assert!(u.contains("@1.2.3.4:8388"));
    }

    #[test]
    fn clash_http_to_gost_uri() {
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            r#"
name: test-http
type: http
server: proxy.example.com
port: 3128
username: user
password: pass
"#,
        )
        .unwrap();
        let uri = clash_proxy_to_gost_uri(&proxy).unwrap();
        assert_eq!(uri.as_deref(), Some("http://user:pass@proxy.example.com:3128"));
    }

    #[test]
    fn clash_https_no_auth_to_gost_uri() {
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            r#"
name: test-https
type: https
server: 10.0.0.1
port: 443
"#,
        )
        .unwrap();
        let uri = clash_proxy_to_gost_uri(&proxy).unwrap();
        assert_eq!(uri.as_deref(), Some("https://10.0.0.1:443"));
    }

    #[test]
    fn clash_vmess_unsupported_returns_none() {
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            r#"
name: test-vmess
type: vmess
server: 1.2.3.4
port: 443
uuid: xxx
"#,
        )
        .unwrap();
        let uri = clash_proxy_to_gost_uri(&proxy).unwrap();
        assert!(uri.is_none());
    }
}
