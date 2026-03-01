use anyhow::{Context as _, Result};
use std::path::Path;

use crate::model::clash::ClashConfig;

/// Parse a cached subscription file into ClashConfig
pub fn parse_subscription(path: &Path) -> Result<ClashConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read subscription: {}", path.display()))?;
    let config: ClashConfig = serde_yaml_ng::from_str(&content)
        .with_context(|| format!("failed to parse subscription YAML: {}", path.display()))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub.yaml");
        let yaml = r"
proxies:
  - name: hk-01
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: secret
  - name: jp-01
    type: vmess
    server: 5.6.7.8
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,DIRECT
";
        std::fs::write(&path, yaml).unwrap();

        let config = parse_subscription(&path).unwrap();
        assert_eq!(config.proxies.len(), 2);
        assert_eq!(ClashConfig::proxy_name(&config.proxies[0]), Some("hk-01"));
        assert_eq!(ClashConfig::proxy_name(&config.proxies[1]), Some("jp-01"));
        assert_eq!(config.rules.len(), 2);
    }

    #[test]
    fn parse_empty_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.yaml");
        std::fs::write(&path, "{}").unwrap();

        let config = parse_subscription(&path).unwrap();
        assert!(config.proxies.is_empty());
        assert!(config.rules.is_empty());
    }

    #[test]
    fn parse_nonexistent_file_errors() {
        let result = parse_subscription(Path::new("/nonexistent/sub.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_yaml_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.yaml");
        std::fs::write(&path, "not: [valid: yaml: {{").unwrap();

        let result = parse_subscription(&path);
        assert!(result.is_err());
    }

    #[test]
    fn parse_subscription_preserves_extra_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extra.yaml");
        let yaml = r"
mixed-port: 7897
allow-lan: true
proxies:
  - name: test
    type: ss
    server: 1.1.1.1
    port: 443
proxy-groups:
  - name: Auto
    type: url-test
    proxies: [test]
    url: http://test.com/204
    interval: 300
rules:
  - MATCH,DIRECT
";
        std::fs::write(&path, yaml).unwrap();

        let config = parse_subscription(&path).unwrap();
        assert_eq!(config.mixed_port, Some(7897));
        assert_eq!(config.proxy_groups.len(), 1);
        assert_eq!(config.proxy_groups[0].name, "Auto");
    }
}
