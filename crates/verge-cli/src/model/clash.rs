use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Full Clash/mihomo configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClashConfig {
    #[serde(default, rename = "mixed-port", skip_serializing_if = "Option::is_none")]
    pub mixed_port: Option<u16>,
    #[serde(default, rename = "allow-lan", skip_serializing_if = "Option::is_none")]
    pub allow_lan: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, rename = "log-level", skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(
        default,
        rename = "external-controller-unix",
        skip_serializing_if = "Option::is_none"
    )]
    pub external_controller_unix: Option<String>,
    #[serde(
        default,
        rename = "external-controller",
        skip_serializing_if = "Option::is_none"
    )]
    pub external_controller: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<serde_yaml_ng::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tun: Option<serde_yaml_ng::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxies: Vec<serde_yaml_ng::Value>,
    #[serde(default, rename = "proxy-groups", skip_serializing_if = "Vec::is_empty")]
    pub proxy_groups: Vec<ProxyGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    /// Catch-all for extra fields we don't explicitly model
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

/// A proxy group in the config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyGroup {
    pub name: String,
    #[serde(rename = "type")]
    pub group_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Extra fields (tolerance, lazy, etc.)
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_yaml_ng::Value>,
}

impl ClashConfig {
    /// Get proxy name from a proxy Value (mapping with "name" key)
    pub fn proxy_name(proxy: &serde_yaml_ng::Value) -> Option<&str> {
        proxy.as_mapping()?.get("name")?.as_str()
    }
    /// Get server host from a proxy Value (mapping with "server" key)
    pub fn proxy_server(proxy: &serde_yaml_ng::Value) -> Option<&str> {
        proxy.as_mapping()?.get("server")?.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_name_from_mapping() {
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "name: my-proxy\ntype: ss\nserver: 1.2.3.4\nport: 8388",
        )
        .unwrap();
        assert_eq!(ClashConfig::proxy_name(&proxy), Some("my-proxy"));
    }

    #[test]
    fn proxy_name_missing_name_field() {
        let proxy: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("type: ss\nserver: 1.2.3.4").unwrap();
        assert_eq!(ClashConfig::proxy_name(&proxy), None);
    }

    #[test]
    fn proxy_name_not_mapping() {
        let proxy = serde_yaml_ng::Value::String("just-a-string".to_string());
        assert_eq!(ClashConfig::proxy_name(&proxy), None);
    }

    #[test]
    fn proxy_name_numeric_name() {
        let proxy: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("name: 12345\ntype: ss").unwrap();
        // Numeric YAML values become integers, not strings
        assert_eq!(ClashConfig::proxy_name(&proxy), None);
    }

    #[test]
    fn clash_config_roundtrip_yaml() {
        let yaml = r"
mixed-port: 7897
allow-lan: true
mode: rule
log-level: info
proxies:
  - name: test-ss
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: secret
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - test-ss
      - DIRECT
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,DIRECT
";
        let config: ClashConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.mixed_port, Some(7897));
        assert_eq!(config.allow_lan, Some(true));
        assert_eq!(config.mode.as_deref(), Some("rule"));
        assert_eq!(config.log_level.as_deref(), Some("info"));
        assert_eq!(config.proxies.len(), 1);
        assert_eq!(
            ClashConfig::proxy_name(&config.proxies[0]),
            Some("test-ss")
        );
        assert_eq!(config.proxy_groups.len(), 1);
        assert_eq!(config.proxy_groups[0].name, "Proxy");
        assert_eq!(config.proxy_groups[0].group_type, "select");
        assert_eq!(config.proxy_groups[0].proxies, vec!["test-ss", "DIRECT"]);
        assert_eq!(config.rules.len(), 2);

        // Roundtrip: serialize and deserialize
        let serialized = serde_yaml_ng::to_string(&config).unwrap();
        let config2: ClashConfig = serde_yaml_ng::from_str(&serialized).unwrap();
        assert_eq!(config2.mixed_port, config.mixed_port);
        assert_eq!(config2.rules, config.rules);
    }

    #[test]
    fn clash_config_empty_default() {
        let config = ClashConfig::default();
        assert_eq!(config.mixed_port, None);
        assert!(config.proxies.is_empty());
        assert!(config.proxy_groups.is_empty());
        assert!(config.rules.is_empty());
    }

    #[test]
    fn clash_config_preserves_extra_fields() {
        let yaml = r"
mixed-port: 7897
some-custom-field: hello
another-field: 42
proxies: []
rules: []
";
        let config: ClashConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.mixed_port, Some(7897));
        assert_eq!(
            config.extra.get("some-custom-field"),
            Some(&serde_yaml_ng::Value::String("hello".to_string()))
        );

        // Roundtrip preserves extra fields
        let serialized = serde_yaml_ng::to_string(&config).unwrap();
        assert!(serialized.contains("some-custom-field"));
    }

    #[test]
    fn proxy_group_with_filter() {
        let yaml = "
name: HK-Auto
type: url-test
filter: 香港
url: http://www.gstatic.com/generate_204
interval: 300
";
        let group: ProxyGroup = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(group.name, "HK-Auto");
        assert_eq!(group.group_type, "url-test");
        assert_eq!(group.filter.as_deref(), Some("香港"));
        assert_eq!(
            group.url.as_deref(),
            Some("http://www.gstatic.com/generate_204")
        );
        assert_eq!(group.interval, Some(300));
        assert!(group.proxies.is_empty());
    }

    #[test]
    fn proxy_group_extra_fields_preserved() {
        let yaml = r"
name: Test
type: url-test
proxies: [a, b]
tolerance: 150
lazy: true
";
        let group: ProxyGroup = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(group.name, "Test");
        assert!(group.extra.contains_key("tolerance"));
        assert!(group.extra.contains_key("lazy"));
    }
}
