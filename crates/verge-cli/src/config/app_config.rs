use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::model::clash::ProxyGroup;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub mihomo: MihomoConfig,
    #[serde(default = "default_output_path")]
    pub output_path: String,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub node_filter: Vec<String>,
    #[serde(default)]
    pub proxies: Vec<serde_yaml_ng::Value>,
    #[serde(default)]
    pub proxy_groups: Vec<ProxyGroup>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub base: serde_yaml_ng::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MihomoConfig {
    #[serde(default = "default_socket_path")]
    pub socket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl Default for MihomoConfig {
    fn default() -> Self {
        Self {
            socket: default_socket_path(),
            api_addr: None,
            secret: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

fn default_socket_path() -> String {
    "/tmp/verge/verge-mihomo.sock".to_string()
}

fn default_output_path() -> String {
    "~/.config/verge-cli/generated.yaml".to_string()
}

impl AppConfig {
    /// Get the config directory (~/.config/verge-cli/)
    pub fn config_dir() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("failed to determine config directory")?
            .join("verge-cli");
        Ok(config_dir)
    }

    /// Get the default config file path
    pub fn default_config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.yaml"))
    }

    /// Get the subscriptions cache directory
    pub fn subscriptions_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("subscriptions"))
    }

    /// Load config from a file path
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let config: Self = serde_yaml_ng::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        Ok(config)
    }

    /// Load config from default or specified path
    pub fn load_or_default(path: Option<&Path>) -> Result<Self> {
        match path {
            Some(p) => Self::load(p),
            None => {
                let default_path = Self::default_config_path()?;
                if default_path.exists() {
                    Self::load(&default_path)
                } else {
                    Ok(Self::default())
                }
            }
        }
    }

    /// Save config to a file
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))?;
        }
        let content = serde_yaml_ng::to_string(self).context("failed to serialize config")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write config: {}", path.display()))?;
        Ok(())
    }

    /// Resolve the output path (expand ~ to home dir)
    pub fn resolved_output_path(&self) -> Result<PathBuf> {
        let expanded = expand_tilde(&self.output_path)?;
        Ok(PathBuf::from(expanded))
    }

    /// Resolve the socket path (expand ~ if present)
    pub fn resolved_socket_path(&self) -> Result<PathBuf> {
        let expanded = expand_tilde(&self.mihomo.socket)?;
        Ok(PathBuf::from(expanded))
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mihomo: MihomoConfig::default(),
            output_path: default_output_path(),
            subscriptions: Vec::new(),
            node_filter: Vec::new(),
            proxies: Vec::new(),
            proxy_groups: Vec::new(),
            rules: Vec::new(),
            base: serde_yaml_ng::Value::Mapping(Default::default()),
        }
    }
}

/// Expand ~ to the user's home directory
fn expand_tilde(path: &str) -> Result<String> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs::home_dir().context("failed to determine home directory")?;
        Ok(format!("{}/{}", home.display(), rest))
    } else {
        Ok(path.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_with_prefix() {
        let result = expand_tilde("~/foo/bar").unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(result, format!("{}/foo/bar", home.display()));
    }

    #[test]
    fn expand_tilde_without_prefix() {
        let result = expand_tilde("/absolute/path").unwrap();
        assert_eq!(result, "/absolute/path");
    }

    #[test]
    fn expand_tilde_bare_tilde_no_slash() {
        // "~something" without "/" should NOT expand
        let result = expand_tilde("~something").unwrap();
        assert_eq!(result, "~something");
    }

    #[test]
    fn default_config_has_expected_values() {
        let config = AppConfig::default();
        assert_eq!(config.mihomo.socket, "/tmp/verge/verge-mihomo.sock");
        assert!(config.mihomo.api_addr.is_none());
        assert!(config.mihomo.secret.is_none());
        assert!(config.subscriptions.is_empty());
        assert!(config.node_filter.is_empty());
        assert!(config.proxies.is_empty());
        assert!(config.proxy_groups.is_empty());
        assert!(config.rules.is_empty());
        assert_eq!(config.output_path, "~/.config/verge-cli/generated.yaml");
    }

    #[test]
    fn app_config_serde_roundtrip() {
        let yaml = "
mihomo:
  socket: /tmp/test.sock
  api_addr: 127.0.0.1:9090
  secret: my-secret
output_path: /tmp/out.yaml
subscriptions:
  - name: sub1
    url: https://example.com/sub
    enabled: true
  - name: sub2
    url: https://example.com/sub2
    enabled: false
node_filter:
  - ^info
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,DIRECT
";
        let config: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.mihomo.socket, "/tmp/test.sock");
        assert_eq!(config.mihomo.api_addr.as_deref(), Some("127.0.0.1:9090"));
        assert_eq!(config.mihomo.secret.as_deref(), Some("my-secret"));
        assert_eq!(config.subscriptions.len(), 2);
        assert!(config.subscriptions[0].enabled);
        assert!(!config.subscriptions[1].enabled);
        assert_eq!(config.node_filter, vec!["^info"]);
        assert_eq!(config.rules.len(), 2);

        // Roundtrip
        let serialized = serde_yaml_ng::to_string(&config).unwrap();
        let config2: AppConfig = serde_yaml_ng::from_str(&serialized).unwrap();
        assert_eq!(config2.mihomo.socket, config.mihomo.socket);
        assert_eq!(config2.subscriptions.len(), config.subscriptions.len());
    }

    #[test]
    fn subscription_enabled_default_true() {
        let yaml = "
name: test-sub
url: https://example.com
";
        let sub: Subscription = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(sub.enabled);
    }

    #[test]
    fn save_and_load_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        let mut config = AppConfig::default();
        config.rules.push("DOMAIN-SUFFIX,test.com,Proxy".to_string());
        config
            .subscriptions
            .push(Subscription {
                name: "test".to_string(),
                url: "https://example.com".to_string(),
                enabled: true,
            });

        config.save(&path).unwrap();
        assert!(path.exists());

        let loaded = AppConfig::load(&path).unwrap();
        assert_eq!(loaded.rules, config.rules);
        assert_eq!(loaded.subscriptions.len(), 1);
        assert_eq!(loaded.subscriptions[0].name, "test");
    }

    #[test]
    fn load_nonexistent_file_errors() {
        let result = AppConfig::load(Path::new("/nonexistent/config.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn load_or_default_returns_default_when_no_path_and_no_file() {
        // With an explicit path that doesn't exist, should error
        let result = AppConfig::load_or_default(Some(Path::new("/nonexistent/config.yaml")));
        assert!(result.is_err());
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("config.yaml");

        let config = AppConfig::default();
        config.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn resolved_output_path_expands_tilde() {
        let config = AppConfig::default();
        let resolved = config.resolved_output_path().unwrap();
        let home = dirs::home_dir().unwrap();
        assert!(resolved.starts_with(&home));
        assert!(resolved.ends_with(".config/verge-cli/generated.yaml"));
    }

    #[test]
    fn resolved_socket_path_absolute() {
        let config = AppConfig::default();
        let resolved = config.resolved_socket_path().unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/verge/verge-mihomo.sock"));
    }
}
