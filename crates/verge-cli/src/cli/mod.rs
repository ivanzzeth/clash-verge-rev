pub mod apply;
pub mod config_cmd;
pub mod delay;
pub mod generate;
pub mod match_cmd;
pub mod mode;
pub mod monitor;
pub mod proxy;
pub mod reload;
pub mod rule;
pub mod status;
pub mod sub;

use anyhow::Result;

use crate::config::app_config::AppConfig;
use crate::mihomo::client::MihomoClient;
use crate::{Cli, Commands, ConfigAction, ProxyAction, RuleAction, RuleSetAction, SubAction};

fn build_client_from_config(
    socket_override: Option<&std::path::Path>,
    config: &AppConfig,
) -> Result<MihomoClient> {
    let socket_path = if let Some(s) = socket_override {
        Some(s.to_path_buf())
    } else {
        Some(config.resolved_socket_path()?)
    };

    let api_addr = config.mihomo.api_addr.clone();
    let secret = config.mihomo.secret.clone();

    MihomoClient::new(socket_path, api_addr, secret)
}

pub async fn run(cli: Cli) -> Result<()> {
    let config_path = cli.config.as_deref();
    let config = AppConfig::load_or_default(config_path)?;
    let json_output = cli.json;
    let socket_override = cli.socket.as_deref();

    let client = || build_client_from_config(socket_override, &config);

    match cli.command {
        Commands::Config { action } => match action {
            ConfigAction::Init => config_cmd::init(),
            ConfigAction::Show => config_cmd::show(config_path),
            ConfigAction::Edit => config_cmd::edit(config_path),
        },
        Commands::Sub { action } => match action {
            SubAction::Add { name, url } => sub::add(&config, config_path, &name, &url),
            SubAction::Update { name } => sub::update(&config, name.as_deref()).await,
            SubAction::List => sub::list(&config),
            SubAction::Remove { name } => sub::remove(&config, config_path, &name),
            SubAction::Show { name } => sub::show(&config, &name),
        },
        Commands::Rule { action } => match action {
            RuleAction::Add { rule } => rule::add(&config, config_path, &rule),
            RuleAction::List => {
                rule::list(&config);
                Ok(())
            }
            RuleAction::Remove { target } => rule::remove(&config, config_path, &target),
            RuleAction::Import { file } => rule::import(&config, config_path, &file),
            RuleAction::Set { action } => match action {
                RuleSetAction::Add { name, target } => {
                    rule::ruleset_add(&config, config_path, &name, &target)
                }
                RuleSetAction::List => {
                    rule::ruleset_list(&config);
                    Ok(())
                }
                RuleSetAction::Remove { name } => {
                    rule::ruleset_remove(&config, config_path, &name)
                }
                RuleSetAction::Show { name } => rule::ruleset_show(&name),
                RuleSetAction::Edit { name } => rule::ruleset_edit(&name),
            },
        },
        Commands::Generate { dry_run } => generate::generate_cmd(&config, dry_run),
        Commands::Apply { force } => apply::apply(&config, &client()?, force).await,
        Commands::Proxy { action } => {
            let c = client()?;
            match action {
                ProxyAction::Groups => proxy::groups(&c, json_output).await,
                ProxyAction::List { group } => {
                    proxy::list(&c, group.as_deref(), json_output).await
                }
                ProxyAction::Now { group } => {
                    proxy::now(&c, group.as_deref(), json_output).await
                }
                ProxyAction::Set { group, node } => proxy::set(&c, &group, &node).await,
            }
        }
        Commands::Delay { node, url, timeout } => {
            delay::delay(&client()?, &node, &url, timeout).await
        }
        Commands::Test { group, url } => {
            delay::test(&client()?, group.as_deref(), &url).await
        }
        Commands::Mode { mode } => mode::mode(&client()?, mode.as_deref(), json_output).await,
        Commands::Traffic => monitor::traffic(&client()?).await,
        Commands::Conns => monitor::conns(&client()?, json_output).await,
        Commands::Closeall => monitor::closeall(&client()?).await,
        Commands::Log { level } => monitor::log_stream(&client()?, level.as_deref()).await,
        Commands::Status => status::status(&client()?, json_output).await,
        Commands::Reload => reload::reload(&client()?, &config).await,
        Commands::Match { domain } => match_cmd::match_domain(&client()?, &domain).await,
        Commands::FlushDns => {
            client()?.flush_dns().await?;
            println!("DNS cache flushed");
            Ok(())
        }
    }
}
