use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct ProxiesResponse {
    pub proxies: HashMap<String, ProxyInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub proxy_type: String,
    #[serde(default)]
    pub now: Option<String>,
    #[serde(default)]
    pub all: Option<Vec<String>>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub udp: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub time: String,
    pub delay: u64,
}

#[derive(Debug, Deserialize)]
pub struct ConfigResponse {
    pub mode: String,
    #[serde(rename = "mixed-port")]
    pub mixed_port: Option<u16>,
    #[serde(rename = "log-level")]
    pub log_level: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "tun")]
    pub tun: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TrafficEntry {
    pub up: u64,
    pub down: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionsResponse {
    #[serde(rename = "downloadTotal")]
    pub download_total: u64,
    #[serde(rename = "uploadTotal")]
    pub upload_total: u64,
    pub connections: Option<Vec<ConnectionInfo>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub metadata: ConnectionMetadata,
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    pub chains: Vec<String>,
    pub rule: String,
    #[serde(rename = "rulePayload")]
    pub rule_payload: String,
    pub start: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionMetadata {
    #[serde(rename = "destinationIP")]
    pub destination_ip: String,
    #[serde(rename = "destinationPort")]
    pub destination_port: String,
    pub host: String,
    #[serde(rename = "type")]
    pub conn_type: String,
    pub network: String,
    #[serde(rename = "sourceIP")]
    pub source_ip: String,
    #[serde(rename = "sourcePort")]
    pub source_port: String,
}

#[derive(Debug, Deserialize)]
pub struct LogEntry {
    #[serde(rename = "type")]
    pub level: String,
    pub payload: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct DelayRequest {
    pub url: String,
    pub timeout: u64,
}

#[derive(Debug, Deserialize)]
pub struct DelayResponse {
    pub delay: u64,
}

#[derive(Debug, Deserialize)]
pub struct RulesResponse {
    pub rules: Vec<RuleInfo>,
}

#[derive(Debug, Deserialize)]
pub struct RuleInfo {
    #[serde(rename = "type")]
    pub rule_type: String,
    pub payload: String,
    pub proxy: String,
}

#[derive(Debug, Deserialize)]
pub struct VersionResponse {
    pub version: String,
}

#[cfg(test)]
#[allow(clippy::needless_raw_string_hashes)]
mod tests {
    use super::*;

    #[test]
    fn parse_proxies_response() {
        let json = r#"{
            "proxies": {
                "DIRECT": {
                    "name": "DIRECT",
                    "type": "Direct",
                    "history": [],
                    "udp": true
                },
                "Proxy": {
                    "name": "Proxy",
                    "type": "Selector",
                    "now": "auto-select",
                    "all": ["node1", "node2"],
                    "history": [],
                    "udp": false
                }
            }
        }"#;
        let resp: ProxiesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.proxies.len(), 2);

        let direct = &resp.proxies["DIRECT"];
        assert_eq!(direct.name, "DIRECT");
        assert_eq!(direct.proxy_type, "Direct");
        assert!(direct.udp);
        assert!(direct.now.is_none());
        assert!(direct.all.is_none());

        let proxy = &resp.proxies["Proxy"];
        assert_eq!(proxy.now.as_deref(), Some("auto-select"));
        assert_eq!(proxy.all.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn parse_config_response() {
        let json = r#"{
            "mode": "rule",
            "mixed-port": 7897,
            "log-level": "info",
            "tun": {"enable": false}
        }"#;
        let resp: ConfigResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.mode, "rule");
        assert_eq!(resp.mixed_port, Some(7897));
        assert_eq!(resp.log_level.as_deref(), Some("info"));
    }

    #[test]
    fn parse_config_response_minimal() {
        let json = r#"{"mode": "global"}"#;
        let resp: ConfigResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.mode, "global");
        assert_eq!(resp.mixed_port, None);
        assert_eq!(resp.log_level, None);
    }

    #[test]
    fn parse_traffic_entry() {
        let json = r#"{"up": 1024, "down": 2048}"#;
        let entry: TrafficEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.up, 1024);
        assert_eq!(entry.down, 2048);
    }

    #[test]
    fn parse_connections_response() {
        let json = r#"{
            "downloadTotal": 100000,
            "uploadTotal": 50000,
            "connections": [{
                "id": "abc123",
                "metadata": {
                    "destinationIP": "1.2.3.4",
                    "destinationPort": "443",
                    "host": "example.com",
                    "type": "HTTP",
                    "network": "tcp",
                    "sourceIP": "192.168.1.1",
                    "sourcePort": "54321"
                },
                "upload": 1000,
                "download": 5000,
                "chains": ["Proxy", "node1"],
                "rule": "DOMAIN-SUFFIX",
                "rulePayload": "example.com",
                "start": "2024-01-01T00:00:00Z"
            }]
        }"#;
        let resp: ConnectionsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.download_total, 100_000);
        assert_eq!(resp.upload_total, 50_000);

        let conns = resp.connections.as_ref().unwrap();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].id, "abc123");
        assert_eq!(conns[0].metadata.host, "example.com");
        assert_eq!(conns[0].metadata.destination_port, "443");
        assert_eq!(conns[0].chains, vec!["Proxy", "node1"]);
        assert_eq!(conns[0].rule, "DOMAIN-SUFFIX");
    }

    #[test]
    fn parse_connections_response_null_connections() {
        let json = r#"{"downloadTotal": 0, "uploadTotal": 0, "connections": null}"#;
        let resp: ConnectionsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.connections.is_none());
    }

    #[test]
    fn parse_log_entry() {
        let json = r#"{"type": "info", "payload": "DNS resolve failed"}"#;
        let entry: LogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.level, "info");
        assert_eq!(entry.payload, "DNS resolve failed");
    }

    #[test]
    fn parse_delay_response() {
        let json = r#"{"delay": 150}"#;
        let resp: DelayResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.delay, 150);
    }

    #[test]
    fn parse_rules_response() {
        let json = r#"{
            "rules": [
                {"type": "DOMAIN-SUFFIX", "payload": "google.com", "proxy": "Proxy"},
                {"type": "MATCH", "payload": "", "proxy": "DIRECT"}
            ]
        }"#;
        let resp: RulesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.rules.len(), 2);
        assert_eq!(resp.rules[0].rule_type, "DOMAIN-SUFFIX");
        assert_eq!(resp.rules[0].payload, "google.com");
        assert_eq!(resp.rules[0].proxy, "Proxy");
        assert_eq!(resp.rules[1].rule_type, "MATCH");
    }

    #[test]
    fn parse_version_response() {
        let json = r#"{"version": "Mihomo Meta alpha-29c39a6"}"#;
        let resp: VersionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.version.contains("Mihomo"));
    }

    #[test]
    fn proxy_info_with_history() {
        let json = r#"{
            "name": "HK-01",
            "type": "Shadowsocks",
            "history": [
                {"time": "2024-01-01T00:00:00Z", "delay": 120},
                {"time": "2024-01-01T00:05:00Z", "delay": 95}
            ],
            "udp": true
        }"#;
        let info: ProxyInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "HK-01");
        assert_eq!(info.history.len(), 2);
        assert_eq!(info.history[0].delay, 120);
        assert_eq!(info.history[1].delay, 95);
    }
}
