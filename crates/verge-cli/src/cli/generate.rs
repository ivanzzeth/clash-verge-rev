use anyhow::Result;
use colored::Colorize as _;

use crate::config::app_config::AppConfig;
use crate::generator::merge;

pub fn generate_cmd(config: &AppConfig, dry_run: bool) -> Result<()> {
    let final_config = merge::generate(config)?;

    if dry_run {
        let yaml = serde_yaml_ng::to_string(&final_config)?;
        println!("{}", yaml);
        return Ok(());
    }

    let output_path = config.resolved_output_path()?;
    merge::write_config(&final_config, &output_path)?;

    // Print summary
    println!("{} Generated config at {}", "✓".green(), output_path.display());
    println!("  Proxies:       {}", final_config.proxies.len());
    println!("  Proxy Groups:  {}", final_config.proxy_groups.len());
    println!("  Rules:         {}", final_config.rules.len());

    Ok(())
}
