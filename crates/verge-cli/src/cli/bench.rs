use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use colored::Colorize as _;
use tokio::net::TcpStream;

use crate::config::app_config::AppConfig;
use crate::mihomo::client::MihomoClient;
use crate::model::clash::ClashConfig;
use crate::subscription::parser;

/// Per-node statistics accumulated across rounds.
struct NodeStats {
    /// Total RTT samples (0 = timeout, excluded from latency stats).
    total_samples: Vec<u64>,
    /// TCP connect times to proxy server (None = DNS/connect failed).
    leg1_samples: Vec<Option<u64>>,
}

impl NodeStats {
    fn new() -> Self {
        Self {
            total_samples: Vec::new(),
            leg1_samples: Vec::new(),
        }
    }

    fn push(&mut self, total_delay: u64, leg1: Option<u64>) {
        self.total_samples.push(total_delay);
        self.leg1_samples.push(leg1);
    }

    fn rounds(&self) -> usize {
        self.total_samples.len()
    }

    fn loss_count(&self) -> usize {
        self.total_samples.iter().filter(|&&d| d == 0).count()
    }

    fn loss_pct(&self) -> f64 {
        if self.rounds() == 0 {
            return 100.0;
        }
        (self.loss_count() as f64 / self.rounds() as f64) * 100.0
    }

    /// Successful delay samples (non-zero).
    fn ok_delays(&self) -> Vec<u64> {
        self.total_samples.iter().copied().filter(|&d| d > 0).collect()
    }

    fn avg(&self) -> Option<u64> {
        let ok = self.ok_delays();
        if ok.is_empty() {
            return None;
        }
        Some(ok.iter().sum::<u64>() / ok.len() as u64)
    }

    fn min(&self) -> Option<u64> {
        self.ok_delays().into_iter().min()
    }

    fn max(&self) -> Option<u64> {
        self.ok_delays().into_iter().max()
    }

    fn p95(&self) -> Option<u64> {
        let mut ok = self.ok_delays();
        if ok.is_empty() {
            return None;
        }
        ok.sort();
        let idx = ((ok.len() as f64 * 0.95).ceil() as usize).min(ok.len()) - 1;
        Some(ok[idx])
    }

    fn jitter(&self) -> Option<u64> {
        let ok = self.ok_delays();
        if ok.len() < 2 {
            return None;
        }
        let mean = ok.iter().sum::<u64>() as f64 / ok.len() as f64;
        let variance = ok.iter().map(|&d| (d as f64 - mean).powi(2)).sum::<f64>() / ok.len() as f64;
        Some(variance.sqrt() as u64)
    }

    fn avg_leg1(&self) -> Option<u64> {
        let ok: Vec<u64> = self.leg1_samples.iter().filter_map(|&v| v).collect();
        if ok.is_empty() {
            return None;
        }
        Some(ok.iter().sum::<u64>() / ok.len() as u64)
    }

    /// Composite score: lower is better.
    /// score = avg + loss% * 10 + jitter * 2
    fn score(&self) -> f64 {
        let avg = self.avg().unwrap_or(9999) as f64;
        let loss = self.loss_pct();
        let jitter = self.jitter().unwrap_or(0) as f64;
        avg + loss * 10.0 + jitter * 2.0
    }
}

/// Measure TCP connect latency to `host:port`. Returns ms or None on failure.
async fn tcp_connect_ms(host: &str, port: u16, timeout_ms: u64) -> Option<u64> {
    let addr = format!("{}:{}", host, port);
    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        TcpStream::connect(&addr),
    )
    .await;
    match result {
        Ok(Ok(_stream)) => Some(start.elapsed().as_millis() as u64),
        _ => None,
    }
}

/// Extract server and port from a YAML proxy mapping.
fn extract_server_port_yaml(proxy: &serde_yaml_ng::Value) -> Option<(String, u16)> {
    let m = proxy.as_mapping()?;
    let server = m.get("server")?.as_str()?;
    let port = m
        .get("port")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })?
        as u16;
    if server.is_empty() || port == 0 {
        return None;
    }
    Some((server.to_string(), port))
}

/// Build a name → (server, port) map by scanning all cached subscription YAML files.
fn build_server_map_from_subscriptions(
    node_names: &[String],
) -> Result<HashMap<String, (String, u16)>> {
    let cache_dir = AppConfig::subscriptions_dir()?;
    let mut server_map: HashMap<String, (String, u16)> = HashMap::new();

    let need: std::collections::HashSet<&str> =
        node_names.iter().map(|s| s.as_str()).collect();

    if !cache_dir.exists() {
        return Ok(server_map);
    }

    let entries = std::fs::read_dir(&cache_dir)
        .with_context(|| format!("failed to read subscriptions dir: {}", cache_dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let parsed = match parser::parse_subscription(&path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for proxy in &parsed.proxies {
            if let Some(name) = ClashConfig::proxy_name(proxy) {
                if need.contains(name) && !server_map.contains_key(name) {
                    if let Some(sp) = extract_server_port_yaml(proxy) {
                        server_map.insert(name.to_string(), sp);
                    }
                }
            }
        }
        // Early exit if we found all nodes.
        if server_map.len() >= need.len() {
            break;
        }
    }

    Ok(server_map)
}

pub async fn bench(
    client: &MihomoClient,
    group: Option<&str>,
    url: &str,
    timeout: u64,
    rounds: u32,
    interval: u64,
) -> Result<()> {
    let proxies = client.get_proxies().await?;

    let group_name = match group {
        Some(g) => g.to_string(),
        None => proxies
            .proxies
            .values()
            .find(|p| p.proxy_type == "Selector")
            .map(|p| p.name.clone())
            .context("no proxy groups found")?,
    };

    // Get node list from the group.
    let group_info = proxies
        .proxies
        .get(&group_name)
        .context("group not found")?;
    let node_names: Vec<String> = group_info
        .all
        .as_ref()
        .context("group has no nodes")?
        .clone();

    println!(
        "Benchmarking group '{}' - {} nodes, {} rounds, {}s interval\n",
        group_name.bold(),
        node_names.len(),
        rounds,
        interval,
    );

    // Build server:port map from cached subscription YAML files.
    // Mihomo API does not expose server/port, so we read from the subscription cache.
    let server_map = build_server_map_from_subscriptions(&node_names)?;

    let mut stats: HashMap<String, NodeStats> = HashMap::new();
    for name in &node_names {
        stats.insert(name.clone(), NodeStats::new());
    }

    for round in 1..=rounds {
        // Run mihomo group delay and TCP connect probes in parallel.
        let group_delay_fut = client.get_group_delay(&group_name, url, timeout);

        // TCP connect probes: all nodes in parallel.
        let tcp_handles: Vec<_> = node_names
            .iter()
            .map(|name| {
                let sp = server_map.get(name).cloned();
                let t = timeout;
                let n = name.clone();
                tokio::spawn(async move {
                    let leg1 = match sp {
                        Some((host, port)) => tcp_connect_ms(&host, port, t).await,
                        None => None,
                    };
                    (n, leg1)
                })
            })
            .collect();

        let (delay_result, tcp_results) = tokio::join!(
            group_delay_fut,
            futures_util::future::join_all(tcp_handles)
        );

        // Collect TCP results.
        let mut leg1_map: HashMap<String, Option<u64>> = HashMap::new();
        for res in tcp_results {
            if let Ok((name, leg1)) = res {
                leg1_map.insert(name, leg1);
            }
        }

        // Collect delay results.
        let delay_obj = match delay_result {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  [{}] round failed: {}", round, e);
                // Record timeouts for all nodes this round.
                for name in &node_names {
                    if let Some(s) = stats.get_mut(name) {
                        let leg1 = leg1_map.get(name).copied().flatten();
                        s.push(0, leg1);
                    }
                }
                if round < rounds {
                    tokio::time::sleep(Duration::from_secs(interval)).await;
                }
                continue;
            }
        };

        let mut ok_count = 0u32;
        let mut timeout_count = 0u32;

        if let Some(obj) = delay_obj.as_object() {
            for name in &node_names {
                let total_delay = obj
                    .get(name)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let leg1 = leg1_map.get(name).copied().flatten();

                if let Some(s) = stats.get_mut(name) {
                    s.push(total_delay, leg1);
                }

                if total_delay > 0 {
                    ok_count += 1;
                } else {
                    timeout_count += 1;
                }
            }
        }

        print!(
            "\r  [{}/{}] {} ok, {} timeout",
            round, rounds, ok_count, timeout_count
        );

        if round < rounds {
            // Show countdown.
            for remaining in (1..=interval).rev() {
                print!(
                    "\r  [{}/{}] {} ok, {} timeout - next in {}s  ",
                    round, rounds, ok_count, timeout_count, remaining
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    println!("\r  Done.{:40}", ""); // clear countdown residue.
    println!();

    // Sort by composite score.
    let mut ranked: Vec<(&String, &NodeStats)> = stats.iter().collect();
    ranked.sort_by(|a, b| a.1.score().partial_cmp(&b.1.score()).unwrap());

    // Determine column widths.
    let name_width = ranked
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(20)
        .max(4);

    // Print header.
    println!(
        "  {:<width$}  {:>5}  {:>5}  {:>5}  {:>5}  {:>6}  {:>9}  {:>9}  {:>5}",
        "Node",
        "Avg",
        "Min",
        "Max",
        "P95",
        "Jitter",
        "Local>Prx",
        "Prx>Dest",
        "Loss",
        width = name_width,
    );
    println!(
        "  {:-<width$}  -----  -----  -----  -----  ------  ---------  ---------  -----",
        "",
        width = name_width,
    );

    for (name, st) in &ranked {
        let avg = st.avg();
        let loss = st.loss_pct();

        let indicator = if loss >= 50.0 {
            "X".red().to_string()
        } else if loss > 0.0 || avg.unwrap_or(9999) >= 500 {
            "!".yellow().to_string()
        } else if avg.unwrap_or(9999) < 200 {
            "*".green().to_string()
        } else {
            " ".to_string()
        };

        let fmt_ms = |v: Option<u64>| -> String {
            match v {
                Some(ms) => format!("{}ms", ms),
                None => "-".dimmed().to_string(),
            }
        };

        let leg1_avg = st.avg_leg1();
        // When total < leg1, it means the node uses relay/CDN entry — traffic
        // doesn't go through the IP we TCP-probed. Show "relay" instead of ≈0ms.
        let leg2_str = match (avg, leg1_avg) {
            (Some(total), Some(l1)) if total >= l1 => format!("\u{2248}{}ms", total - l1),
            (Some(_), Some(_)) => "relay".dimmed().to_string(),
            _ => "-".dimmed().to_string(),
        };

        println!(
            "{} {:<width$}  {:>5}  {:>5}  {:>5}  {:>5}  {:>6}  {:>9}  {:>9}  {:>5}",
            indicator,
            name,
            fmt_ms(avg),
            fmt_ms(st.min()),
            fmt_ms(st.max()),
            fmt_ms(st.p95()),
            fmt_ms(st.jitter()),
            fmt_ms(leg1_avg),
            leg2_str,
            format!("{:.0}%", loss),
            width = name_width,
        );
    }

    // Best node recommendation.
    if let Some((best_name, best_st)) = ranked.first() {
        println!();
        let avg_str = best_st.avg().map(|v| format!("{}ms", v)).unwrap_or("-".to_string());
        let loss_str = format!("{:.0}%", best_st.loss_pct());
        let jitter_str = best_st.jitter().map(|v| format!("{}ms", v)).unwrap_or("-".to_string());
        println!(
            "  Best: {} (avg {}, loss {}, jitter {})",
            best_name.bold().green(),
            avg_str,
            loss_str,
            jitter_str,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_stats_empty() {
        let s = NodeStats::new();
        assert_eq!(s.rounds(), 0);
        assert_eq!(s.loss_count(), 0);
        assert!(s.avg().is_none());
        assert!(s.min().is_none());
        assert!(s.max().is_none());
        assert!(s.p95().is_none());
        assert!(s.jitter().is_none());
        assert!(s.avg_leg1().is_none());
    }

    #[test]
    fn node_stats_all_timeouts() {
        let mut s = NodeStats::new();
        s.push(0, None);
        s.push(0, None);
        s.push(0, None);
        assert_eq!(s.rounds(), 3);
        assert_eq!(s.loss_count(), 3);
        assert!((s.loss_pct() - 100.0).abs() < f64::EPSILON);
        assert!(s.avg().is_none());
    }

    #[test]
    fn node_stats_basic() {
        let mut s = NodeStats::new();
        s.push(100, Some(30));
        s.push(120, Some(35));
        s.push(0, Some(32)); // timeout
        s.push(150, Some(28));
        s.push(110, Some(31));

        assert_eq!(s.rounds(), 5);
        assert_eq!(s.loss_count(), 1);
        assert!((s.loss_pct() - 20.0).abs() < f64::EPSILON);
        // avg of [100, 120, 150, 110] = 120
        assert_eq!(s.avg(), Some(120));
        assert_eq!(s.min(), Some(100));
        assert_eq!(s.max(), Some(150));
        // p95 of sorted [100, 110, 120, 150]: ceil(4 * 0.95) - 1 = 3 → 150
        assert_eq!(s.p95(), Some(150));
        // jitter: stddev of [100, 120, 150, 110]
        assert!(s.jitter().is_some());
        // avg leg1: (30+35+32+28+31)/5 = 31.2 → 31
        assert_eq!(s.avg_leg1(), Some(31));
    }

    #[test]
    fn node_stats_single_sample() {
        let mut s = NodeStats::new();
        s.push(85, Some(20));
        assert_eq!(s.avg(), Some(85));
        assert_eq!(s.min(), Some(85));
        assert_eq!(s.max(), Some(85));
        assert_eq!(s.p95(), Some(85));
        assert!(s.jitter().is_none()); // need >= 2 samples
        assert_eq!(s.avg_leg1(), Some(20));
    }

    #[test]
    fn node_stats_score_no_loss_low_latency() {
        let mut s = NodeStats::new();
        for _ in 0..5 {
            s.push(80, Some(20));
        }
        // avg=80, loss=0%, jitter=0 → score = 80
        assert!((s.score() - 80.0).abs() < 1.0);
    }

    #[test]
    fn node_stats_score_high_loss() {
        let mut s = NodeStats::new();
        s.push(100, Some(20));
        s.push(0, None);
        s.push(0, None);
        s.push(0, None);
        s.push(0, None);
        // avg=100, loss=80%, jitter=0 → score = 100 + 800 + 0 = 900
        assert!(s.score() > 800.0);
    }

    fn yaml_proxy(yaml_str: &str) -> serde_yaml_ng::Value {
        serde_yaml_ng::from_str(yaml_str).unwrap()
    }

    #[test]
    fn extract_server_port_valid() {
        let proxy = yaml_proxy("name: HK-01\ntype: ss\nserver: hk1.example.com\nport: 443");
        let result = extract_server_port_yaml(&proxy);
        assert_eq!(result, Some(("hk1.example.com".to_string(), 443)));
    }

    #[test]
    fn extract_server_port_string_port() {
        let proxy = yaml_proxy("server: sg.example.com\nport: '8080'");
        let result = extract_server_port_yaml(&proxy);
        assert_eq!(result, Some(("sg.example.com".to_string(), 8080)));
    }

    #[test]
    fn extract_server_port_missing_server() {
        let proxy = yaml_proxy("port: 443");
        assert!(extract_server_port_yaml(&proxy).is_none());
    }

    #[test]
    fn extract_server_port_missing_port() {
        let proxy = yaml_proxy("server: x.com");
        assert!(extract_server_port_yaml(&proxy).is_none());
    }

    #[test]
    fn extract_server_port_empty_server() {
        let proxy = yaml_proxy("server: ''\nport: 443");
        assert!(extract_server_port_yaml(&proxy).is_none());
    }

    #[test]
    fn extract_server_port_zero_port() {
        let proxy = yaml_proxy("server: x.com\nport: 0");
        assert!(extract_server_port_yaml(&proxy).is_none());
    }

    #[tokio::test]
    async fn tcp_connect_unreachable_returns_none() {
        // RFC 5737 TEST-NET: should be unreachable, fast timeout.
        let result = tcp_connect_ms("192.0.2.1", 1, 200).await;
        assert!(result.is_none());
    }
}
