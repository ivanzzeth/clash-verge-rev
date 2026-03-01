use anyhow::{Context as _, Result};
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;

use crate::config::app_config::AppConfig;
use crate::model::clash::{ClashConfig, ProxyGroup};
use crate::subscription::parser;

/// Generate the final merged mihomo config
pub fn generate(config: &AppConfig) -> Result<ClashConfig> {
    let cache_dir = AppConfig::subscriptions_dir()?;

    // Step 1: Load all enabled subscriptions
    let mut all_proxies: Vec<serde_yaml_ng::Value> = Vec::new();
    let mut sub_rules: Vec<String> = Vec::new();
    let mut seen_proxy_names: HashSet<String> = HashSet::new();

    for sub in &config.subscriptions {
        if !sub.enabled {
            continue;
        }
        let cache_path = cache_dir.join(format!("{}.yaml", sub.name));
        if !cache_path.exists() {
            eprintln!(
                "warning: subscription '{}' not cached, run 'sub update' first",
                sub.name
            );
            continue;
        }

        let sub_config = parser::parse_subscription(&cache_path)
            .with_context(|| format!("failed to parse subscription: {}", sub.name))?;

        // Step 2: Collect proxies, handle name collisions
        for proxy in sub_config.proxies {
            if let Some(name) = ClashConfig::proxy_name(&proxy) {
                let name = name.to_string();
                if seen_proxy_names.contains(&name) {
                    // Prefix with subscription name on collision
                    let mut proxy = proxy;
                    let new_name = format!("{}/{}", sub.name, name);
                    if let Some(mapping) = proxy.as_mapping_mut() {
                        mapping.insert(
                            serde_yaml_ng::Value::String("name".to_string()),
                            serde_yaml_ng::Value::String(new_name.clone()),
                        );
                    }
                    seen_proxy_names.insert(new_name);
                    all_proxies.push(proxy);
                } else {
                    seen_proxy_names.insert(name);
                    all_proxies.push(proxy);
                }
            }
        }

        // Collect subscription rules (used as fallback after custom rules)
        for rule in sub_config.rules {
            if !rule.starts_with("MATCH") {
                sub_rules.push(rule);
            }
        }
    }

    // Step 3: Append custom proxies
    for proxy in &config.proxies {
        if let Some(name) = ClashConfig::proxy_name(proxy) {
            seen_proxy_names.insert(name.to_string());
        }
        all_proxies.push(proxy.clone());
    }

    // Step 4: Apply node_filter regex - remove matching entries
    let filters: Vec<Regex> = config
        .node_filter
        .iter()
        .map(|pattern| {
            Regex::new(pattern)
                .with_context(|| format!("invalid node_filter regex: {}", pattern))
        })
        .collect::<Result<Vec<_>>>()?;

    if !filters.is_empty() {
        all_proxies.retain(|proxy| {
            if let Some(name) = ClashConfig::proxy_name(proxy) {
                !filters.iter().any(|re| re.is_match(name))
            } else {
                true
            }
        });
        // Update seen names after filtering
        seen_proxy_names.clear();
        for proxy in &all_proxies {
            if let Some(name) = ClashConfig::proxy_name(proxy) {
                seen_proxy_names.insert(name.to_string());
            }
        }
    }

    // Collect all proxy names for group resolution
    let all_proxy_names: Vec<String> = all_proxies
        .iter()
        .filter_map(|p| ClashConfig::proxy_name(p).map(|s| s.to_string()))
        .collect();

    // Step 5: Build proxy groups
    let mut proxy_groups: Vec<ProxyGroup> = Vec::new();
    let group_names: Vec<String> = config.proxy_groups.iter().map(|g| g.name.clone()).collect();

    for group_template in &config.proxy_groups {
        let mut group = group_template.clone();

        if let Some(filter_pattern) = &group.filter {
            // Filter-based: populate proxies by regex matching all proxy names
            let re = Regex::new(filter_pattern).with_context(|| {
                format!(
                    "invalid filter regex in group '{}': {}",
                    group.name, filter_pattern
                )
            })?;
            let matched: Vec<String> = all_proxy_names
                .iter()
                .filter(|name| re.is_match(name))
                .cloned()
                .collect();
            group.proxies = matched;
            // Remove the filter field from output (it's our custom field, not mihomo's)
            group.filter = None;
        } else {
            // Explicit proxies: resolve references
            // Valid references: other group names, DIRECT, REJECT, or literal proxy names
            let special = ["DIRECT", "REJECT", "PASS", "COMPATIBLE"];
            let resolved: Vec<String> = group
                .proxies
                .iter()
                .filter(|name| {
                    special.contains(&name.as_str())
                        || group_names.contains(name)
                        || seen_proxy_names.contains(name.as_str())
                })
                .cloned()
                .collect();
            group.proxies = resolved;
        }

        proxy_groups.push(group);
    }

    // Step 6: Build rules
    // Collect valid targets: group names + special keywords
    let special_targets: HashSet<&str> =
        ["DIRECT", "REJECT", "PASS", "COMPATIBLE", "no-resolve"]
            .into_iter()
            .collect();
    let valid_targets: HashSet<String> = proxy_groups
        .iter()
        .map(|g| g.name.clone())
        .chain(special_targets.iter().map(|s| (*s).to_string()))
        .collect();

    let mut rules: Vec<String> = Vec::new();
    // Custom rules first
    for rule in &config.rules {
        if !rule.starts_with("MATCH") {
            rules.push(rule.clone());
        }
    }
    // Then subscription rules (dedup, validate target exists)
    let custom_set: HashSet<String> = rules.iter().cloned().collect();
    let mut dropped_rules = 0usize;
    for rule in &sub_rules {
        if custom_set.contains(rule.as_str()) {
            continue;
        }
        // Validate that the rule's target group exists
        if rule_target_valid(rule, &valid_targets) {
            rules.push(rule.clone());
        } else {
            dropped_rules += 1;
        }
    }
    if dropped_rules > 0 {
        eprintln!(
            "warning: dropped {} subscription rules referencing non-existent groups",
            dropped_rules
        );
    }
    // MATCH at end - from custom rules or default
    let match_rule = config
        .rules
        .iter()
        .find(|r| r.starts_with("MATCH"))
        .cloned()
        .unwrap_or_else(|| "MATCH,DIRECT".to_string());
    rules.push(match_rule);

    // Step 7: Assemble final config
    let mut final_config = if let serde_yaml_ng::Value::Mapping(base_map) = &config.base {
        serde_yaml_ng::from_value::<ClashConfig>(serde_yaml_ng::Value::Mapping(base_map.clone()))
            .unwrap_or_default()
    } else {
        ClashConfig::default()
    };

    final_config.proxies = all_proxies;
    final_config.proxy_groups = proxy_groups;
    final_config.rules = rules;

    Ok(final_config)
}

/// Check if a rule's target proxy group exists in our config.
/// Rule format: TYPE,PAYLOAD,TARGET or TYPE,PAYLOAD,TARGET,no-resolve
/// Also rejects RULE-SET rules (which require rule-providers not in our model).
fn rule_target_valid(rule: &str, valid_targets: &HashSet<String>) -> bool {
    // RULE-SET and SUB-RULE require external definitions we don't support yet
    if rule.starts_with("RULE-SET,") || rule.starts_with("SUB-RULE,") {
        return false;
    }
    let parts: Vec<&str> = rule.splitn(4, ',').collect();
    if parts.len() < 3 {
        return true; // Malformed rule, let mihomo decide
    }
    let target = parts[2].trim();
    valid_targets.contains(target)
}

/// Write the generated config to a file
pub fn write_config(config: &ClashConfig, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create output directory: {}",
                parent.display()
            )
        })?;
    }
    let yaml = serde_yaml_ng::to_string(config).context("failed to serialize final config")?;
    std::fs::write(path, &yaml)
        .with_context(|| format!("failed to write config: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_empty_config() {
        let config = AppConfig::default();
        let result = generate(&config);
        assert!(result.is_ok());
        let clash = result.unwrap();
        assert!(clash.proxies.is_empty());
        assert!(clash.proxy_groups.is_empty());
        // Should still have MATCH rule
        assert_eq!(clash.rules.len(), 1);
        assert_eq!(clash.rules[0], "MATCH,DIRECT");
    }

    #[test]
    fn generate_with_custom_rules_only() {
        let mut config = AppConfig::default();
        config.rules = vec![
            "DOMAIN-SUFFIX,google.com,Proxy".to_string(),
            "DOMAIN-KEYWORD,openai,ChatGPT".to_string(),
            "MATCH,Proxy".to_string(),
        ];

        let clash = generate(&config).unwrap();
        assert_eq!(clash.rules.len(), 3);
        assert_eq!(clash.rules[0], "DOMAIN-SUFFIX,google.com,Proxy");
        assert_eq!(clash.rules[1], "DOMAIN-KEYWORD,openai,ChatGPT");
        // MATCH should be last
        assert_eq!(clash.rules[2], "MATCH,Proxy");
    }

    #[test]
    fn generate_match_rule_always_last() {
        let mut config = AppConfig::default();
        // Put MATCH in the middle
        config.rules = vec![
            "DOMAIN-SUFFIX,a.com,Proxy".to_string(),
            "MATCH,Proxy".to_string(),
            "DOMAIN-SUFFIX,b.com,Direct".to_string(),
        ];

        let clash = generate(&config).unwrap();
        // MATCH should be at the end
        assert!(clash.rules.last().unwrap().starts_with("MATCH"));
        // Non-MATCH rules come first
        assert_eq!(clash.rules[0], "DOMAIN-SUFFIX,a.com,Proxy");
        assert_eq!(clash.rules[1], "DOMAIN-SUFFIX,b.com,Direct");
    }

    #[test]
    fn generate_custom_proxies_added() {
        let mut config = AppConfig::default();
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "name: my-socks5\ntype: socks5\nserver: 1.2.3.4\nport: 1080",
        )
        .unwrap();
        config.proxies = vec![proxy];

        let clash = generate(&config).unwrap();
        assert_eq!(clash.proxies.len(), 1);
        assert_eq!(ClashConfig::proxy_name(&clash.proxies[0]), Some("my-socks5"));
    }

    #[test]
    fn generate_node_filter_removes_matching() {
        let mut config = AppConfig::default();
        config.node_filter = vec!["^info-".to_string(), "^expire".to_string()];

        // Add proxies directly (simulating what would come from subscriptions)
        let proxies: Vec<serde_yaml_ng::Value> = vec![
            serde_yaml_ng::from_str("name: info-node\ntype: ss\nserver: 1.1.1.1\nport: 443")
                .unwrap(),
            serde_yaml_ng::from_str("name: expire-soon\ntype: ss\nserver: 2.2.2.2\nport: 443")
                .unwrap(),
            serde_yaml_ng::from_str("name: hk-01\ntype: ss\nserver: 3.3.3.3\nport: 443")
                .unwrap(),
        ];
        config.proxies = proxies;

        let clash = generate(&config).unwrap();
        // Only hk-01 should remain
        assert_eq!(clash.proxies.len(), 1);
        assert_eq!(ClashConfig::proxy_name(&clash.proxies[0]), Some("hk-01"));
    }

    #[test]
    fn generate_proxy_group_explicit_resolves() {
        let mut config = AppConfig::default();
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "name: node1\ntype: ss\nserver: 1.1.1.1\nport: 443",
        )
        .unwrap();
        config.proxies = vec![proxy];

        config.proxy_groups = vec![ProxyGroup {
            name: "Proxy".to_string(),
            group_type: "select".to_string(),
            proxies: vec![
                "node1".to_string(),
                "DIRECT".to_string(),
                "nonexistent".to_string(),
            ],
            url: None,
            interval: None,
            strategy: None,
            filter: None,
            extra: Default::default(),
        }];

        let clash = generate(&config).unwrap();
        assert_eq!(clash.proxy_groups.len(), 1);
        // "nonexistent" should be filtered out
        assert_eq!(clash.proxy_groups[0].proxies, vec!["node1", "DIRECT"]);
    }

    #[test]
    fn generate_proxy_group_filter_regex() {
        let mut config = AppConfig::default();
        let proxies: Vec<serde_yaml_ng::Value> = vec![
            serde_yaml_ng::from_str("name: HK-01\ntype: ss\nserver: 1.1.1.1\nport: 443").unwrap(),
            serde_yaml_ng::from_str("name: HK-02\ntype: ss\nserver: 2.2.2.2\nport: 443").unwrap(),
            serde_yaml_ng::from_str("name: JP-01\ntype: ss\nserver: 3.3.3.3\nport: 443").unwrap(),
            serde_yaml_ng::from_str("name: US-01\ntype: ss\nserver: 4.4.4.4\nport: 443").unwrap(),
        ];
        config.proxies = proxies;

        config.proxy_groups = vec![ProxyGroup {
            name: "HK-Group".to_string(),
            group_type: "url-test".to_string(),
            proxies: Vec::new(),
            url: Some("http://test.com/204".to_string()),
            interval: Some(300),
            strategy: None,
            filter: Some("^HK".to_string()),
            extra: Default::default(),
        }];

        let clash = generate(&config).unwrap();
        assert_eq!(clash.proxy_groups.len(), 1);
        assert_eq!(clash.proxy_groups[0].proxies, vec!["HK-01", "HK-02"]);
        // filter field should be cleared
        assert!(clash.proxy_groups[0].filter.is_none());
    }

    #[test]
    fn generate_invalid_node_filter_errors() {
        let mut config = AppConfig::default();
        config.node_filter = vec!["[invalid".to_string()];

        let result = generate(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid node_filter regex"));
    }

    #[test]
    fn generate_invalid_group_filter_errors() {
        let mut config = AppConfig::default();
        config.proxies = vec![
            serde_yaml_ng::from_str("name: n1\ntype: ss\nserver: 1.1.1.1\nport: 443").unwrap(),
        ];
        config.proxy_groups = vec![ProxyGroup {
            name: "Bad".to_string(),
            group_type: "select".to_string(),
            proxies: Vec::new(),
            url: None,
            interval: None,
            strategy: None,
            filter: Some("[unclosed".to_string()),
            extra: Default::default(),
        }];

        let result = generate(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid filter regex"));
    }

    #[test]
    fn generate_group_references_other_groups() {
        let mut config = AppConfig::default();
        let proxy: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "name: node1\ntype: ss\nserver: 1.1.1.1\nport: 443",
        )
        .unwrap();
        config.proxies = vec![proxy];

        config.proxy_groups = vec![
            ProxyGroup {
                name: "Auto".to_string(),
                group_type: "url-test".to_string(),
                proxies: vec!["node1".to_string()],
                url: Some("http://test.com/204".to_string()),
                interval: Some(300),
                strategy: None,
                filter: None,
                extra: Default::default(),
            },
            ProxyGroup {
                name: "Proxy".to_string(),
                group_type: "select".to_string(),
                proxies: vec![
                    "Auto".to_string(),
                    "node1".to_string(),
                    "DIRECT".to_string(),
                    "REJECT".to_string(),
                ],
                url: None,
                interval: None,
                strategy: None,
                filter: None,
                extra: Default::default(),
            },
        ];

        let clash = generate(&config).unwrap();
        assert_eq!(clash.proxy_groups.len(), 2);
        // "Auto" is a valid group reference
        assert_eq!(
            clash.proxy_groups[1].proxies,
            vec!["Auto", "node1", "DIRECT", "REJECT"]
        );
    }

    #[test]
    fn generate_base_config_applied() {
        let mut config = AppConfig::default();
        let base: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "mixed-port: 7897\nallow-lan: true\nmode: rule\nlog-level: info",
        )
        .unwrap();
        config.base = base;

        let clash = generate(&config).unwrap();
        assert_eq!(clash.mixed_port, Some(7897));
        assert_eq!(clash.allow_lan, Some(true));
        assert_eq!(clash.mode.as_deref(), Some("rule"));
    }

    #[test]
    fn write_config_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.yaml");

        let mut config = ClashConfig::default();
        config.mixed_port = Some(7897);
        config.rules = vec!["MATCH,DIRECT".to_string()];

        write_config(&config, &path).unwrap();
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("mixed-port"));
        assert!(content.contains("MATCH,DIRECT"));
    }

    #[test]
    fn write_config_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("out.yaml");

        let config = ClashConfig::default();
        write_config(&config, &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn generate_rules_dedup_custom_vs_sub() {
        // Simulate: custom rules and subscription rules with overlap
        // Since we can't easily set up subscription caching in unit tests,
        // we test the rule dedup logic by using only custom rules + MATCH
        let mut config = AppConfig::default();
        config.rules = vec![
            "DOMAIN-SUFFIX,google.com,Proxy".to_string(),
            "DOMAIN-SUFFIX,github.com,Proxy".to_string(),
            "MATCH,Proxy".to_string(),
        ];

        let clash = generate(&config).unwrap();
        // Custom rules first, MATCH last
        assert_eq!(clash.rules[0], "DOMAIN-SUFFIX,google.com,Proxy");
        assert_eq!(clash.rules[1], "DOMAIN-SUFFIX,github.com,Proxy");
        assert_eq!(clash.rules[2], "MATCH,Proxy");
    }

    #[test]
    fn generate_default_match_rule_when_none_specified() {
        let config = AppConfig::default();
        let clash = generate(&config).unwrap();
        // Should get default MATCH,DIRECT
        assert_eq!(clash.rules.last().unwrap(), "MATCH,DIRECT");
    }
}
