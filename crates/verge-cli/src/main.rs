mod cli;
mod config;
mod generator;
mod mihomo;
mod model;
mod subscription;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Default, ValueEnum)]
pub enum ListFormat {
    #[default]
    Table,
    Comma,
    Newline,
}

#[derive(Clone, Default, ValueEnum)]
pub enum ListAddr {
    #[default]
    Socks5,
    Http,
}

#[derive(Parser)]
#[command(name = "verge-cli", about = "CLI for Clash Verge Rev / mihomo")]
pub struct Cli {
    /// Config file path
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Override mihomo socket path
    #[arg(short = 'S', long, global = true)]
    socket: Option<PathBuf>,

    /// JSON output
    #[arg(long, global = true)]
    json: bool,

    /// Disable colors
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage subscriptions
    Sub {
        #[command(subcommand)]
        action: SubAction,
    },
    /// Manage custom rules
    Rule {
        #[command(subcommand)]
        action: RuleAction,
    },
    /// Generate final mihomo config YAML
    Generate {
        /// Print to stdout instead of writing to file
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate config and hot-reload mihomo
    Apply {
        /// Force reload even if config unchanged
        #[arg(long)]
        force: bool,
    },
    /// Manage config backups
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Restore config from a backup and reload mihomo
    Rollback {
        /// Backup ID (epoch seconds). Uses latest if omitted.
        id: Option<String>,
    },
    /// Manage proxy groups and nodes
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },
    /// Test single node latency
    #[command(alias = "d")]
    Delay {
        /// Node name
        node: String,
        /// Test URL
        #[arg(default_value = "http://www.gstatic.com/generate_204")]
        url: String,
        /// Timeout in ms
        #[arg(default_value = "5000")]
        timeout: u64,
    },
    /// Test all nodes in a group
    #[command(alias = "t")]
    Test {
        /// Group name (default: first group)
        group: Option<String>,
        /// Test URL
        #[arg(long, default_value = "http://www.gstatic.com/generate_204")]
        url: String,
    },
    /// Get or set proxy mode
    #[command(alias = "m")]
    Mode {
        /// Mode to set (rule/global/direct)
        mode: Option<String>,
    },
    /// Real-time traffic monitor
    #[command(alias = "tr")]
    Traffic,
    /// Active connections
    #[command(alias = "c")]
    Conns,
    /// Close all connections
    #[command(alias = "ca")]
    Closeall,
    /// Stream logs
    #[command(alias = "l")]
    Log {
        /// Log level filter
        level: Option<String>,
    },
    /// Status overview
    #[command(alias = "st")]
    Status,
    /// Reload mihomo config
    #[command(alias = "r")]
    Reload,
    /// Check which rule matches a domain
    #[command(alias = "M")]
    Match {
        /// Domain to match
        domain: String,
    },
    /// Flush DNS cache
    FlushDns,
    /// Expose nodes as local HTTP/SOCKS5 proxies (for web3 airdrop, etc.)
    Expose {
        /// Filter by region: substring match on node names (e.g. 香港, 日本, Hong Kong). Comma-separated for multiple
        #[arg(long)]
        region: Option<String>,
        #[command(subcommand)]
        action: ExposeAction,
    },
}

#[derive(Subcommand)]
pub enum ExposeAction {
    /// List nodes and their expose ports
    List {
        /// Filter by region: substring match on node names (e.g. 香港, 日本). Comma-separated for multiple
        #[arg(long)]
        region: Option<String>,
        /// Filter by upstream protocol: ss, http, https, socks5. Comma-separated for multiple
        #[arg(long)]
        protocol: Option<String>,
        /// Output format: table (default), comma, newline. Use comma/newline for copy-paste
        #[arg(long)]
        format: Option<ListFormat>,
        /// When format is comma/newline: which address to output (socks5 or http)
        #[arg(long)]
        addr: Option<ListAddr>,
    },
    /// Start exposing nodes as local proxies
    Start {
        /// Base port (default 10000). Each node uses base+N*2 (socks5) and base+N*2+1 (http)
        #[arg(long, default_value = "10000")]
        base_port: u16,
        /// Comma-separated node names to expose (default: all)
        #[arg(long)]
        nodes: Option<String>,
        /// Filter by region: substring match on node names (e.g. 香港, 日本). Comma-separated for multiple
        #[arg(long)]
        region: Option<String>,
    },
    /// Stop all expose processes
    Stop {
        /// Filter by region: only stop matching nodes
        #[arg(long)]
        region: Option<String>,
    },
}

#[derive(Subcommand)]
enum BackupAction {
    /// List available backups with timestamps
    List,
    /// Print a backup's config.yaml
    Show {
        /// Backup ID (epoch seconds)
        id: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Create default config interactively
    Init,
    /// Print current config
    Show,
    /// Open config in $EDITOR
    Edit,
}

#[derive(Subcommand)]
enum SubAction {
    /// Add a subscription
    Add {
        /// Subscription name
        name: String,
        /// Subscription URL
        url: String,
    },
    /// Update subscriptions
    Update {
        /// Specific subscription name (updates all if omitted)
        name: Option<String>,
    },
    /// List all subscriptions
    List,
    /// Remove a subscription
    Remove {
        /// Subscription name
        name: String,
    },
    /// Show cached subscription details
    Show {
        /// Subscription name
        name: String,
    },
}

#[derive(Subcommand)]
enum RuleAction {
    /// Add a custom rule
    Add {
        /// Rule string (e.g. "DOMAIN-SUFFIX,openai.com,ChatGPT")
        rule: String,
    },
    /// List custom rules
    List,
    /// Remove a rule by index or pattern
    Remove {
        /// Index or pattern
        target: String,
    },
    /// Import rules from file
    Import {
        /// File path
        file: PathBuf,
    },
    /// Manage rule file references (rule sets)
    Set {
        #[command(subcommand)]
        action: RuleSetAction,
    },
}

#[derive(Subcommand)]
enum RuleSetAction {
    /// Create a rule file and add reference to config
    Add {
        /// Rule set name (maps to ~/.config/verge-cli/rules/<name>.rules)
        name: String,
        /// Default target proxy group for rules in this file
        target: String,
    },
    /// List all rule file references
    List,
    /// Remove a rule file reference from config
    Remove {
        /// Rule set name
        name: String,
    },
    /// Show contents of a rule file
    Show {
        /// Rule set name
        name: String,
    },
    /// Open rule file in $EDITOR
    Edit {
        /// Rule set name
        name: String,
    },
}

#[derive(Subcommand)]
enum ProxyAction {
    /// List proxy groups
    #[command(alias = "g")]
    Groups,
    /// List nodes in a group
    #[command(alias = "ls")]
    List {
        /// Group name (default: first group)
        group: Option<String>,
    },
    /// Show current selection
    #[command(alias = "n")]
    Now {
        /// Group name (default: first group)
        group: Option<String>,
    },
    /// Switch node in a group
    #[command(alias = "s")]
    Set {
        /// Group name
        group: String,
        /// Node name
        node: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.no_color {
        colored::control::set_override(false);
    }

    cli::run(cli).await
}
