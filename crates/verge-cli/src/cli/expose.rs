//! Expose nodes as local HTTP/SOCKS5 proxies (Rust local proxy, no GOST).
//!
//! Each node gets two local ports: SOCKS5 and HTTP (randomly assigned to avoid conflicts).
//! State is stored in ~/.config/verge-cli/expose/state.json.

use anyhow::{Context as _, Result};
use base64::Engine;
use colored::Colorize as _;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Stdio;

use crate::config::app_config::AppConfig;
use crate::generator::merge;
use crate::ip_type;
use crate::model::clash::ClashConfig;

#[derive(Debug, Serialize, Deserialize)]
struct ExposeState {
    /// node_name -> (socks5_port, http_port, pid)
    nodes: HashMap<String, (u16, u16, u32)>,
    #[serde(default)]
    base_port: u16,
}

/// Check if process is still alive (Unix: kill -0).
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        pid > 0
    }
}

/// Find an unused port on 127.0.0.1, excluding given ports.
fn find_unused_port(exclude: &HashSet<u16>) -> Result<u16> {
    for _ in 0..100 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .context("bind for port discovery")?;
        let port = listener.local_addr().context("local_addr")?.port();
        drop(listener);
        if !exclude.contains(&port) {
            return Ok(port);
        }
    }
    anyhow::bail!("could not find unused port after 100 attempts")
}

/// Remove dead processes from state. Returns true if any were pruned.
fn prune_dead_nodes(state: &mut ExposeState) -> bool {
    let dead: Vec<String> = state
        .nodes
        .iter()
        .filter(|(_, (_, _, pid))| !is_process_alive(*pid))
        .map(|(k, _)| k.clone())
        .collect();
    for name in &dead {
        state.nodes.remove(name);
    }
    !dead.is_empty()
}

fn expose_state_path() -> Result<PathBuf> {
    let dir = AppConfig::config_dir()?.join("expose");
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir.join("state.json"))
}

fn expose_log_dir() -> Result<PathBuf> {
    let dir = AppConfig::config_dir()?.join("expose").join("logs");
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Sanitize node name for use as log filename (no path separators, limited length).
fn sanitize_node_name_for_log(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' => c,
            _ => '_',
        })
        .take(64)
        .collect();
    if s.is_empty() {
        "node".to_string()
    } else {
        s.trim().to_string()
    }
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

/// Convert Clash proxy YAML to upstream URI (ss://, http://, etc.). Returns None if unsupported.
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

/// Spawn verge-cli expose-proxy in a new session (Unix: setsid) so it does not receive SIGHUP.
/// Stderr is wired to the given Stdio (e.g. log file).
fn spawn_local_proxy_detached(
    listen_socks: &str,
    listen_http: &str,
    upstream: &str,
    stderr: Stdio,
) -> Result<tokio::process::Child> {
    let exe = std::env::current_exe().context("current executable path")?;
    let args = [
        "expose-proxy",
        "--listen-socks",
        listen_socks,
        "--listen-http",
        listen_http,
        "--upstream",
        upstream,
    ];
    #[cfg(unix)]
    let mut cmd = if which::which("setsid").is_ok() {
        let mut c = tokio::process::Command::new("setsid");
        c.arg(&exe).args(&args);
        c
    } else {
        let mut c = tokio::process::Command::new(&exe);
        c.args(&args);
        c
    };
    #[cfg(not(unix))]
    let mut cmd = {
        let mut c = tokio::process::Command::new(&exe);
        c.args(&args);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .context("spawn local proxy")
}

pub async fn list(
    config: &AppConfig,
    region: Option<&str>,
    protocol: Option<&str>,
    format: &str,
    addr: &str,
) -> Result<()> {
    let clash = merge::generate(config).await?;
    let mut state = load_state()?;
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
            let server = ClashConfig::proxy_server(p).map(String::from);
            let (socks_port, http_port, pid) = state
                .as_ref()
                .and_then(|s| s.nodes.get(&name).copied())
                .unwrap_or((0, 0, 0));
            let alive = is_process_alive(pid);
            Some((name, server, uri, socks_port, http_port, pid, alive))
        })
        .collect();

    // Prune dead nodes from state and persist
    if let Some(ref mut s) = state {
        if prune_dead_nodes(s) {
            let _ = save_state(s);
        }
    }

    if nodes_to_show.is_empty() {
        println!("No exposable nodes (ss/http/https/socks5 only). Run 'sub update' first.");
        return Ok(());
    }

    match format {
        "comma" | "newline" => {
            let sep = if format == "comma" { "," } else { "\n" };
            let addrs: Vec<String> = nodes_to_show
                .iter()
                .filter(|(_, _, _, _socks_port, _http_port, _pid, alive)| *alive)
                .map(|(_, _, _, socks_port, http_port, _, _)| {
                    let port = if addr == "http" { *http_port } else { *socks_port };
                    format!("127.0.0.1:{}", port)
                })
                .collect();
            println!("{}", addrs.join(sep));
        }
        _ => {
            let config_dir = AppConfig::config_dir()?;
            let (mut ip_type_cache, ip_type_cache_path) = ip_type::load_ip_type_cache(&config_dir)?;
            let client = reqwest::Client::new();
            let unique_servers: HashSet<String> = nodes_to_show
                .iter()
                .filter_map(|(_, server, ..)| server.as_ref().cloned())
                .collect();
            let mut server_type_map: HashMap<String, String> = HashMap::new();
            for server in unique_servers {
                let t = ip_type::get_ip_type(
                    &server,
                    &mut ip_type_cache,
                    &client,
                    &ip_type_cache_path,
                    ip_type::CACHE_TTL_SECS,
                )
                .await;
                server_type_map.insert(server, t);
            }
            println!(
                "{:<24} {:<8} {:<6} {:<12} {:<12} {}",
                "NODE".bold(),
                "TYPE".bold(),
                "IP".bold(),
                "SOCKS5".bold(),
                "HTTP".bold(),
                "STATUS".bold()
            );
            for (name, server_opt, uri, socks_port, http_port, pid, alive) in &nodes_to_show {
                let ty = uri.split("://").next().unwrap_or("?").to_uppercase();
                let ip_label = server_opt
                    .as_ref()
                    .and_then(|s| server_type_map.get(s))
                    .map(|t| ip_type::ip_type_label(t))
                    .unwrap_or("-");
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
                let status = if *alive {
                    "running".green().to_string()
                } else if *pid > 0 {
                    "dead".red().to_string()
                } else {
                    "stopped".dimmed().to_string()
                };
                println!(
                    "{:<24} {:<8} {:<6} {:<12} {:<12} {}",
                    name, ty, ip_label, socks, http, status
                );
            }
        }
    }
    Ok(())
}

pub async fn start(
    config: &AppConfig,
    nodes_filter: Option<&str>,
    region: Option<&str>,
) -> Result<()> {
    let clash = merge::generate(config).await?;
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
        if !uri.trim().to_lowercase().starts_with("ss://") {
            eprintln!(
                "{} Skipping '{}' (local proxy supports ss only for now)",
                "!".yellow(),
                name
            );
            continue;
        }
        to_expose.push((name, uri));
    }

    if to_expose.is_empty() {
        anyhow::bail!("No nodes to expose. Check --nodes or run 'sub update'.");
    }

    // Load state, prune dead nodes, stop nodes we're about to replace
    let mut new_state = if let Some(mut state) = load_state()? {
        prune_dead_nodes(&mut state);
        for (name, _) in &to_expose {
            if let Some((_, _, pid)) = state.nodes.remove(name) {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
            }
        }
        state
    } else {
        ExposeState {
            nodes: HashMap::new(),
            base_port: 0,
        }
    };

    let mut used_ports: HashSet<u16> = new_state
        .nodes
        .values()
        .flat_map(|(s, h, _)| [*s, *h])
        .collect();

    let log_dir = expose_log_dir()?;

    for (name, uri) in &to_expose {
        let socks_port = find_unused_port(&used_ports)?;
        used_ports.insert(socks_port);
        let http_port = find_unused_port(&used_ports)?;
        used_ports.insert(http_port);

        let listen_socks = format!("127.0.0.1:{}", socks_port);
        let listen_http = format!("127.0.0.1:{}", http_port);

        let log_path = log_dir.join(format!("{}.log", sanitize_node_name_for_log(name)));
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(&log_path)
            .with_context(|| format!("open log {}", log_path.display()))?;

        let stderr = Stdio::from(log_file);

        let child = spawn_local_proxy_detached(&listen_socks, &listen_http, uri, stderr)
            .with_context(|| format!("failed to start local proxy for {}", name))?;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let pid = child.id().context("local proxy process has no pid")?;
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
    println!(
        "\n{} {} node(s) exposed. Use 'verge-cli expose stop' to stop.",
        "✓".green(),
        new_state.nodes.len()
    );
    println!("  Logs: {}", log_dir.display());
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

    prune_dead_nodes(&mut state);

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
