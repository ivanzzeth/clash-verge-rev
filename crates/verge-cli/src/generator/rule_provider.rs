//! Rule-provider support for RULE-SET and SUB-RULE expansion.
//!
//! Fetches rule content from HTTP URLs or local files, parses by behavior
//! (classical, domain, ipcidr), and converts to full Clash rules with target.

use anyhow::{Context as _, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// Rule-provider config from subscription YAML (rule-providers: name: {...})
#[derive(Debug, Clone, Deserialize)]
pub struct RuleProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(default)]
    pub behavior: String,
    pub url: Option<String>,
    pub path: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // reserved for future cache TTL
    pub interval: Option<u64>,
}

/// Extract rule-providers from raw YAML (subscription or config).
/// Returns name -> config map.
pub fn parse_rule_providers_from_yaml(yaml: &str) -> Result<IndexMap<String, RuleProviderConfig>> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(yaml).context("parse YAML for rule-providers")?;
    let mapping = value.as_mapping().context("root must be mapping")?;
    let rp = mapping.get("rule-providers");
    let Some(rp) = rp else {
        return Ok(IndexMap::new());
    };
    let rp_map = rp.as_mapping().context("rule-providers must be mapping")?;
    let mut out = IndexMap::new();
    for (k, v) in rp_map {
        let name = k.as_str().context("rule-provider key must be string")?;
        let config: RuleProviderConfig =
            serde_yaml_ng::from_value(v.clone()).with_context(|| {
                format!("parse rule-provider '{}'", name)
            })?;
        out.insert(name.to_string(), config);
    }
    Ok(out)
}

/// Fetch rule content: HTTP URL or local file.
/// For file type, path is relative to config_dir (subscription cache dir).
pub async fn fetch_rule_content(
    config: &RuleProviderConfig,
    config_dir: &Path,
) -> Result<String> {
    match config.provider_type.to_lowercase().as_str() {
        "http" | "https" => {
            let url = config
                .url
                .as_deref()
                .context("http rule-provider must have url")?;
            let client = reqwest::Client::builder()
                .user_agent("verge-cli/0.1")
                .build()
                .context("build HTTP client")?;
            let resp = client
                .get(url)
                .send()
                .await
                .with_context(|| format!("fetch rule-provider from {}", url))?;
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("rule-provider fetch {} returned HTTP {}", url, status);
            }
            resp.text()
                .await
                .with_context(|| format!("read body from {}", url))
        }
        "file" => {
            let path_str = config
                .path
                .as_deref()
                .context("file rule-provider must have path")?;
            let path = Path::new(path_str);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                config_dir.join(path)
            };
            std::fs::read_to_string(&resolved)
                .with_context(|| format!("read rule-provider file {}", resolved.display()))
        }
        _ => anyhow::bail!(
            "unsupported rule-provider type: {}",
            config.provider_type
        ),
    }
}

/// Parse rule content by behavior and convert to full rules with target.
/// - classical: payload is TYPE,PAYLOAD (e.g. DOMAIN-SUFFIX,google.com) -> append ,target
/// - domain: payload is domain pattern -> DOMAIN-SUFFIX or DOMAIN
/// - ipcidr: payload is CIDR -> IP-CIDR,payload,target
pub fn parse_rules_from_content(
    content: &str,
    behavior: &str,
    target: &str,
) -> Vec<String> {
    let behavior = behavior.to_lowercase();
    let mut rules = Vec::new();

    // Try YAML payload first
    if let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(content) {
        if let Some(mapping) = value.as_mapping() {
            if let Some(payload) = mapping.get("payload") {
                if let Some(arr) = payload.as_sequence() {
                    for item in arr {
                        let s = item.as_str().unwrap_or_default().trim();
                        if s.is_empty() || s.starts_with('#') {
                            continue;
                        }
                        if let Some(rule) = convert_payload_line(s, &behavior, target) {
                            rules.push(rule);
                        }
                    }
                    return rules;
                }
            }
        }
    }

    // Fallback: plain text lines
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rule) = convert_payload_line(line, &behavior, target) {
            rules.push(rule);
        }
    }
    rules
}

fn convert_payload_line(line: &str, behavior: &str, target: &str) -> Option<String> {
    match behavior {
        "classical" => {
            // Payload is TYPE,PAYLOAD or TYPE,PAYLOAD,no-resolve - insert target before no-resolve
            let parts: Vec<&str> = line.splitn(4, ',').map(|s| s.trim()).collect();
            if parts.len() >= 3 {
                // Already has 3+ parts: could be TYPE,PAYLOAD,TARGET or TYPE,PAYLOAD,no-resolve
                if parts[2].eq_ignore_ascii_case("no-resolve") {
                    Some(format!("{},{},{},no-resolve", parts[0], parts[1], target))
                } else {
                    // Assume already has target, use as-is
                    Some(line.to_string())
                }
            } else if parts.len() == 2 {
                let (a, b) = (parts[0], parts[1]);
                if b.eq_ignore_ascii_case("no-resolve") {
                    None
                } else {
                    Some(format!("{},{},{}", a, b, target))
                }
            } else if parts.len() == 1 && !parts[0].is_empty() {
                Some(format!("{},{}", line, target))
            } else {
                None
            }
        }
        "domain" => {
            let domain = line.trim_matches('"').trim_matches('\'');
            if domain.is_empty() {
                return None;
            }
            let rule_type = if domain.starts_with('.') {
                format!("DOMAIN-SUFFIX,{},{}", domain.trim_start_matches('.'), target)
            } else {
                format!("DOMAIN,{},{}", domain, target)
            };
            Some(rule_type)
        }
        "ipcidr" => {
            let cidr = line.trim_matches('"').trim_matches('\'');
            if cidr.is_empty() {
                return None;
            }
            Some(format!("IP-CIDR,{},{}", cidr, target))
        }
        _ => None,
    }
}

/// Parse sub-rules from raw YAML (sub-rules: name: [rule1, rule2, ...])
pub fn parse_sub_rules_from_yaml(yaml: &str) -> Result<IndexMap<String, Vec<String>>> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(yaml).context("parse YAML for sub-rules")?;
    let mapping = value.as_mapping().context("root must be mapping")?;
    let sr = mapping.get("sub-rules");
    let Some(sr) = sr else {
        return Ok(IndexMap::new());
    };
    let sr_map = sr.as_mapping().context("sub-rules must be mapping")?;
    let mut out = IndexMap::new();
    for (k, v) in sr_map {
        let name = k.as_str().context("sub-rule key must be string")?;
        let rules: Vec<String> = serde_yaml_ng::from_value(v.clone())
            .with_context(|| format!("parse sub-rule '{}'", name))?;
        out.insert(name.to_string(), rules);
    }
    Ok(out)
}

/// Parse SUB-RULE,(condition),name into (condition, name).
/// Returns None if format is invalid.
pub fn parse_sub_rule_parts(rule: &str) -> Option<(&str, &str)> {
    let rest = rule.strip_prefix("SUB-RULE,")?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut end = 0usize;
    for (i, c) in rest.chars().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return None;
    }
    let condition = &rest[1..end];
    let after = rest[end + 1..].trim_start();
    let name = after.strip_prefix(',')?.trim();
    if name.is_empty() {
        return None;
    }
    Some((condition, name))
}

/// Expand SUB-RULE,(condition),name into AND rules.
/// Format: SUB-RULE,(NETWORK,tcp),rule1 -> for each rule in sub-rules[rule1]: AND,((NETWORK,tcp),(payload)),target
pub fn expand_sub_rule(
    condition: &str,
    sub_rule_name: &str,
    sub_rules: &IndexMap<String, Vec<String>>,
    valid_targets: &HashSet<String>,
) -> Vec<String> {
    let Some(rules) = sub_rules.get(sub_rule_name) else {
        eprintln!(
            "warning: sub-rule '{}' not found, skipping SUB-RULE",
            sub_rule_name
        );
        return Vec::new();
    };
    let mut out = Vec::new();
    for rule in rules {
        let rule = rule.trim();
        if rule.is_empty() || rule.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = rule.split(',').map(str::trim).collect();
        if parts.len() < 2 {
            continue;
        }
        let target = parts[parts.len() - 1];
        if !valid_targets.contains(target) {
            continue;
        }
        if parts[0] == "MATCH" {
            out.push(format!("AND,(({}),(MATCH)),{}", condition, target));
            continue;
        }
        // payload: first N-1 parts joined by comma (TYPE,PAYLOAD,...)
        let payload = parts[..parts.len() - 1].join(",");
        out.push(format!("AND,(({}),({})),{}", condition, payload, target));
    }
    out
}

/// Expand RULE-SET,name,target into concrete rules.
/// Returns empty vec if provider not found or fetch fails (logs warning).
pub async fn expand_rule_set(
    provider_name: &str,
    target: &str,
    providers: &IndexMap<String, RuleProviderConfig>,
    config_dir: &Path,
) -> Vec<String> {
    let Some(provider) = providers.get(provider_name) else {
        eprintln!(
            "warning: rule-provider '{}' not found, skipping RULE-SET",
            provider_name
        );
        return Vec::new();
    };
    let content = match fetch_rule_content(provider, config_dir).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "warning: failed to fetch rule-provider '{}': {}",
                provider_name, e
            );
            return Vec::new();
        }
    };
    let behavior = if provider.behavior.is_empty() {
        "classical"
    } else {
        &provider.behavior
    };
    parse_rules_from_content(&content, behavior, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_classical_text() {
        let content = "DOMAIN-SUFFIX,google.com\nDOMAIN-KEYWORD,google\n";
        let rules = parse_rules_from_content(content, "classical", "Proxy");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "DOMAIN-SUFFIX,google.com,Proxy");
        assert_eq!(rules[1], "DOMAIN-KEYWORD,google,Proxy");
    }

    #[test]
    fn parse_classical_yaml() {
        let content = r"
payload:
  - DOMAIN-SUFFIX,google.com
  - IP-CIDR,127.0.0.0/8
";
        let rules = parse_rules_from_content(content, "classical", "DIRECT");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "DOMAIN-SUFFIX,google.com,DIRECT");
        assert_eq!(rules[1], "IP-CIDR,127.0.0.0/8,DIRECT");
    }

    #[test]
    fn parse_domain_behavior() {
        let content = ".blogger.com\n*.*.microsoft.com\n";
        let rules = parse_rules_from_content(content, "domain", "Proxy");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "DOMAIN-SUFFIX,blogger.com,Proxy");
        assert_eq!(rules[1], "DOMAIN,*.*.microsoft.com,Proxy");
    }

    #[test]
    fn parse_ipcidr_behavior() {
        let content = "192.168.1.0/24\n10.0.0.0/8\n";
        let rules = parse_rules_from_content(content, "ipcidr", "DIRECT");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "IP-CIDR,192.168.1.0/24,DIRECT");
        assert_eq!(rules[1], "IP-CIDR,10.0.0.0/8,DIRECT");
    }

    #[test]
    fn parse_sub_rules_from_yaml_works() {
        let yaml = r"
sub-rules:
  tcp-proxy:
    - DOMAIN-SUFFIX,baidu.com,DIRECT
    - MATCH,Proxy
  udp-only:
    - IP-CIDR,192.168.0.0/16,DIRECT
";
        let sr = parse_sub_rules_from_yaml(yaml).unwrap();
        assert_eq!(sr.len(), 2);
        assert_eq!(sr["tcp-proxy"].len(), 2);
        assert_eq!(sr["tcp-proxy"][0], "DOMAIN-SUFFIX,baidu.com,DIRECT");
        assert_eq!(sr["tcp-proxy"][1], "MATCH,Proxy");
    }

    #[test]
    fn parse_sub_rule_parts_works() {
        let (cond, name) = parse_sub_rule_parts("SUB-RULE,(NETWORK,tcp),tcp-proxy").unwrap();
        assert_eq!(cond, "NETWORK,tcp");
        assert_eq!(name, "tcp-proxy");
    }

    #[test]
    fn expand_sub_rule_works() {
        use std::collections::HashSet;
        let mut sub_rules = IndexMap::new();
        sub_rules.insert(
            "tcp-proxy".to_string(),
            vec![
                "DOMAIN-SUFFIX,baidu.com,DIRECT".to_string(),
                "MATCH,Proxy".to_string(),
            ],
        );
        let valid: HashSet<String> = ["DIRECT", "Proxy"].iter().map(|s| (*s).to_string()).collect();
        let out = expand_sub_rule("NETWORK,tcp", "tcp-proxy", &sub_rules, &valid);
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"AND,((NETWORK,tcp),(DOMAIN-SUFFIX,baidu.com)),DIRECT".to_string()));
        assert!(out.contains(&"AND,((NETWORK,tcp),(MATCH)),Proxy".to_string()));
    }

    #[test]
    fn parse_rule_providers_from_yaml_works() {
        let yaml = r"
rule-providers:
  SteamCN:
    type: http
    behavior: classical
    url: https://example.com/rules.yml
  Local:
    type: file
    path: ./local.yaml
";
        let providers = parse_rule_providers_from_yaml(yaml).unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers["SteamCN"].provider_type, "http");
        assert_eq!(providers["SteamCN"].behavior, "classical");
        assert_eq!(providers["Local"].provider_type, "file");
    }
}
