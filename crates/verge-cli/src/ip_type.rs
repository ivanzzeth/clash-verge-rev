//! IP type lookup (datacenter / residential / mobile) with persistent cache.
//! Uses ip-api.com (free, no key). Cache TTL 7 days.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CACHE_TTL_SECS: u64 = 7 * 24 * 3600; // 7 days
const IP_API_FIELDS: &str = "status,hosting,mobile,proxy";
const RATE_LIMIT_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedEntry {
    #[serde(rename = "t")]
    pub ip_type: String,
    #[serde(rename = "ts")]
    pub cached_at: u64,
}

pub(crate) type Cache = HashMap<String, CachedEntry>;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path(config_dir: &Path) -> Result<std::path::PathBuf> {
    let dir = config_dir.join("expose");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir.join("ip-type-cache.json"))
}

fn load_cache(path: &Path) -> Result<Cache> {
    if !path.exists() {
        return Ok(Cache::new());
    }
    let s = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&s).with_context(|| "parse ip-type cache")
}

fn save_cache(path: &Path, cache: &Cache) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
    }
    let s = serde_json::to_string_pretty(cache).context("serialize ip-type cache")?;
    std::fs::write(path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Resolve server hostname to one IPv4 or IPv6 address.
async fn resolve_to_ip(server: &str) -> Result<String> {
    let mut addrs = tokio::net::lookup_host((server, 0))
        .await
        .with_context(|| format!("resolve {}", server))?;
    let ip = addrs
        .next()
        .with_context(|| format!("no address for {}", server))?
        .ip()
        .to_string();
    Ok(ip)
}

/// Fetch IP type from ip-api.com (hosting -> dc, mobile -> mobile, else -> res).
async fn fetch_ip_type(client: &reqwest::Client, ip: &str) -> Result<String> {
    let url = format!(
        "http://ip-api.com/json/{}?fields={}",
        ip, IP_API_FIELDS
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("fetch ip-api for {}", ip))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .with_context(|| "parse ip-api response")?;
    let hosting = json.get("hosting").and_then(|v| v.as_bool()).unwrap_or(false);
    let mobile = json.get("mobile").and_then(|v| v.as_bool()).unwrap_or(false);
    let t = if hosting {
        "dc"
    } else if mobile {
        "mobile"
    } else {
        "res"
    };
    Ok(t.to_string())
}

/// Get IP type for a server (hostname or IP). Uses cache; on miss calls ip-api.com and persists.
/// Rate-limits: 2s delay after each API call to stay under 45/min.
pub async fn get_ip_type(
    server: &str,
    cache: &mut Cache,
    client: &reqwest::Client,
    cache_path: &Path,
    ttl_secs: u64,
) -> String {
    let ip = match resolve_to_ip(server).await {
        Ok(ip) => ip,
        Err(_) => return "?".to_string(),
    };
    let now = now_secs();
    if let Some(entry) = cache.get(&ip) {
        if now.saturating_sub(entry.cached_at) < ttl_secs {
            return entry.ip_type.clone();
        }
    }
    let t = match fetch_ip_type(client, &ip).await {
        Ok(t) => t,
        Err(_) => "?".to_string(),
    };
    cache.insert(
        ip.clone(),
        CachedEntry {
            ip_type: t.clone(),
            cached_at: now,
        },
    );
    let _ = save_cache(cache_path, cache);
    tokio::time::sleep(RATE_LIMIT_DELAY).await;
    t
}

/// Load cache from config dir. Returns (cache, path).
pub fn load_ip_type_cache(config_dir: &Path) -> Result<(Cache, std::path::PathBuf)> {
    let path = cache_path(config_dir)?;
    let cache = load_cache(&path)?;
    Ok((cache, path))
}

/// Short label for table: dc -> DC, res -> RES, mobile -> MOB, ? -> -
pub fn ip_type_label(t: &str) -> &'static str {
    match t {
        "dc" => "DC",
        "res" => "RES",
        "mobile" => "MOB",
        _ => "-",
    }
}
