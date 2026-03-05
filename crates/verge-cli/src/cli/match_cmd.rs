use anyhow::Result;
use colored::Colorize as _;

use crate::mihomo::client::MihomoClient;

pub async fn match_domain(client: &MihomoClient, domain: &str) -> Result<()> {
    let rules = client.get_rules().await?;

    let domain_lower = domain.to_lowercase();

    for rule in &rules.rules {
        let type_lower = rule.rule_type.to_lowercase();
        let matched = match type_lower.as_str() {
            "domainsuffix" => domain_lower.ends_with(&rule.payload.to_lowercase()),
            "domainkeyword" => domain_lower.contains(&rule.payload.to_lowercase()),
            "domain" => domain_lower == rule.payload.to_lowercase(),
            "match" => true,
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
