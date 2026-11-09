use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub bind_port: u16,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    pub upstream_dns: Vec<String>,
    pub filter_lists_dir: PathBuf,
    #[serde(default = "default_blocked_ip")]
    pub blocked_domain_ip: Ipv4Addr,
    #[serde(default = "default_refresh_hours")]
    pub refresh_hours: u64,
}

impl Config {
    pub async fn load(path: &PathBuf) -> anyhow::Result<Self> {
        if path.exists() {
            let content = tokio::fs::read_to_string(path).await?;
            let config: Config = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            let default = Config::default();
            let dir = match path.parent() {
                Some(p) => p.to_path_buf(),
                None => PathBuf::from("/tmp"),
            };
            tokio::fs::create_dir_all(&dir).await?;
            let json = serde_json::to_string_pretty(&default)?;
            tokio::fs::write(path, json).await?;
            info!("Created default config at {:?}", path);
            Ok(default)
        }
    }

    pub fn default_upstream() -> Vec<String> {
        vec![
            "1.1.1.1:53".to_string(),
            "1.0.0.1:53".to_string(),
            "9.9.9.9:53".to_string(),
            "8.8.8.8:53".to_string(),
        ]
    }

    /// Returns the bind socket address configured in the config file.
    pub fn bind_socket(&self) -> SocketAddr {
        format!("{}:{}", self.bind_addr, self.bind_port).parse().expect("invalid bind address")
    }
}

impl Default for Config {
    fn default() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("/home"))
            .join("dns-adblocker");
        Config {
            bind_port: default_port(),
            bind_addr: default_bind_addr(),
            upstream_dns: Config::default_upstream(),
            filter_lists_dir: config_dir.join("filter_lists"),
            blocked_domain_ip: default_blocked_ip(),
            refresh_hours: default_refresh_hours(),
        }
    }
}

fn default_port() -> u16 { 53 }
fn default_bind_addr() -> String { "127.0.0.1".to_string() }
fn default_blocked_ip() -> Ipv4Addr { Ipv4Addr::new(0, 0, 0, 0) }
fn default_refresh_hours() -> u64 { 24 }
