use anyhow::Result;
use colored::Colorize as _;

use crate::mihomo::client::MihomoClient;

pub async fn match_domain(client: &MihomoClient, domain: &str) -> Result<()> {
    let rules = client.get_rules().await?;

    let domain_lower = domain.to_lowercase();

    for rule in &rules.rules {
        let matched = match rule.rule_type.as_str() {
            "DomainSuffix" => domain_lower.ends_with(&rule.payload.to_lowercase()),
            "DomainKeyword" => domain_lower.contains(&rule.payload.to_lowercase()),
            "Domain" => domain_lower == rule.payload.to_lowercase(),
            "Match" => true,
            _ => false,
        };

        if matched {
            println!("{} {} -> {}",
                format!("{},{}", rule.rule_type, rule.payload).bold(),
                "matches".green(),
                rule.proxy.cyan()
            );
            return Ok(());
        }
    }

    println!("{} No matching rule found for '{}'", "✗".red(), domain);
    Ok(())
}
