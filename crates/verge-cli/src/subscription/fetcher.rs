use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

use crate::config::app_config::Subscription;

/// Fetch a subscription URL and cache the result to disk
pub async fn fetch_subscription(sub: &Subscription, cache_dir: &Path) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .user_agent("clash-verge/v2.2.3")
        .build()
        .context("failed to build HTTP client")?;

    let response = client
        .get(&sub.url)
        .send()
        .await
        .with_context(|| format!("failed to fetch subscription: {}", sub.name))?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("subscription '{}' returned HTTP {}", sub.name, status);
    }

    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read subscription body: {}", sub.name))?;

    // Validate it's valid YAML
    let _: serde_yaml_ng::Value = serde_yaml_ng::from_str(&body)
        .with_context(|| format!("subscription '{}' returned invalid YAML", sub.name))?;

    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("failed to create cache directory: {}", cache_dir.display()))?;

    let cache_path = cache_dir.join(format!("{}.yaml", sub.name));
    std::fs::write(&cache_path, &body)
        .with_context(|| format!("failed to cache subscription: {}", cache_path.display()))?;

    Ok(cache_path)
}

/// Check if a cached subscription exists
pub fn cached_path(sub_name: &str, cache_dir: &Path) -> PathBuf {
    cache_dir.join(format!("{}.yaml", sub_name))
}

/// Remove cached subscription
pub fn remove_cache(sub_name: &str, cache_dir: &Path) -> Result<()> {
    let path = cached_path(sub_name, cache_dir);
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove cache: {}", path.display()))?;
    }
    Ok(())
}
