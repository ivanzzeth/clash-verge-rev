use anyhow::Result;
use colored::Colorize as _;

use crate::mihomo::client::MihomoClient;

pub async fn mode(client: &MihomoClient, mode: Option<&str>, json_output: bool) -> Result<()> {
    match mode {
        Some(m) => {
            // Validate mode
            let valid = ["rule", "global", "direct"];
            if !valid.contains(&m) {
                anyhow::bail!("invalid mode '{}', expected one of: {}", m, valid.join(", "));
            }
            client.set_mode(m).await?;
            println!("{} Mode set to {}", "✓".green(), m.cyan());
        }
        None => {
            let config = client.get_config().await?;
            if json_output {
                println!("{}", serde_json::json!({"mode": config.mode}));
            } else {
                println!("Current mode: {}", config.mode.cyan());
            }
        }
    }
    Ok(())
}
