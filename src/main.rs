mod config;
mod filter_lists;
mod dns_server;

use config::Config;
use filter_lists::FilterListManager;
use dns_server::DnsAdBlockerServer;

use anyhow::Result;
use std::path::PathBuf;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("dns_adblocker=info".parse()?)
        )
        .with_target(false)
        .init();

    info!("Starting DNS AdBlocker");

    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("/home"))
                .join("dns-adblocker")
                .join("config.json")
        });

    let config = Config::load(&config_path).await?;

    info!("Configuration loaded: bind_port={}, upstream={:?}",
          config.bind_port, config.upstream_dns);

    let filter_manager = FilterListManager::new(config.filter_lists_dir.clone()).await;
    filter_manager.initialize().await;

    let server = DnsAdBlockerServer::new(&config);

    info!("DNS AdBlocker running on port {}. Press Ctrl+C to stop.", config.bind_port);
    server.run(filter_manager).await?;

    info!("DNS AdBlocker stopped cleanly");
    Ok(())
}
