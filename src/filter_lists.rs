use adblock::{Engine, FilterSet, request::Request};
use adblock::lists::ParseOptions;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Notify};
use tracing::{info, warn};

/// Message sent to the engine worker thread
pub enum EngineMsg {
    IsBlocked {
        domain: String,
        reply_tx: oneshot::Sender<bool>,
    },
    Rebuild,
}

/// Manages filter lists and owns the adblock engine in a background thread.
/// The engine is NOT Send+Sync (uses Rc internally), so it lives exclusively
/// in the worker thread. All queries go through the MPSC channel.
pub struct FilterListManager {
    pub sender: Arc<std::sync::Mutex<mpsc::Sender<EngineMsg>>>,
    pub lists_dir: PathBuf,
    notify: Arc<Notify>,
}

impl Clone for FilterListManager {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            lists_dir: self.lists_dir.clone(),
            notify: self.notify.clone(),
        }
    }
}

impl FilterListManager {
    /// Create a new manager that will download the given URLs into `lists_dir`.
    pub async fn new(lists_dir: PathBuf, urls: Vec<String>) -> Self {
        tokio::fs::create_dir_all(&lists_dir)
            .await
            .expect("Failed to create filter lists directory");

        let (sender, receiver) = mpsc::channel::<EngineMsg>(32);
        let sender_arc = Arc::new(std::sync::Mutex::new(sender));
        let notify = Arc::new(Notify::new());

        let worker_dir = lists_dir.clone();
        let init_notify = notify.clone();
        // Run the engine worker in a dedicated blocking thread — safe because
        // Engine is not Send/Sync, so it must never leave this thread.
        let worker_dir_clone = worker_dir.clone();
        tokio::task::spawn_blocking(move || {
            run_engine_worker(receiver, &worker_dir_clone, init_notify);
        });

        // Download all configured filter lists asynchronously
        let client = reqwest::Client::builder()
            .user_agent("dns-adblocker/0.1")
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        for url in urls {
            let dest = worker_dir.join(url_to_filename(&url));
            match Self::download_filter(&client, &url, &dest).await {
                Ok(bytes) => info!("Downloaded {} ({} bytes)", url, bytes),
                Err(e) => warn!("Failed to download {}: {}", url, e),
            }
        }

        Self {
            sender: sender_arc,
            lists_dir,
            notify,
        }
    }

    async fn download_filter(client: &reqwest::Client, url: &str, dest: &PathBuf) -> Result<usize> {
        let response = client.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("HTTP {} for {}", status, url);
        }
        let content = response.bytes().await?;
        let len = content.len();
        tokio::fs::write(dest, &content).await?;
        Ok(len)
    }

    /// Wait until the engine worker has built the initial filter set.
    pub async fn wait_initialized(&self) {
        self.notify.notified().await;
    }

    /// Check if a domain should be blocked by sending a message to the engine worker.
    /// Returns false if the worker is not yet ready.
    pub async fn is_blocked(&self, domain: &str) -> bool {
        // Wait for the worker to signal it's initialized
        self.notify.notified().await;

        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let tx = self.sender.lock().unwrap();
            let _ = tx.send(EngineMsg::IsBlocked {
                domain: domain.to_string(),
                reply_tx,
            });
        }
        reply_rx.await.unwrap_or(false)
    }

    /// Signal the worker to rebuild its engine from disk (called when lists are refreshed).
    pub fn request_rebuild(&self) {
        if let Ok(tx) = self.sender.lock() {
            let _ = tx.send(EngineMsg::Rebuild);
        }
    }
}

fn url_to_filename(url: &str) -> String {
    url.split('/').last()
        .filter(|s| !s.is_empty())
        .unwrap_or("filter.txt")
        .to_string()
}

/// Main event loop for the engine worker task. Runs on a dedicated blocking thread.
/// The Engine lives here exclusively — it is never moved between threads.
fn run_engine_worker(
    mut receiver: mpsc::Receiver<EngineMsg>,
    lists_dir: &PathBuf,
    notify: Arc<Notify>,
) {
    // Build initial engine from saved files (or empty if none exist)
    let mut engine = build_engine_from_dir(lists_dir).unwrap_or_else(|_| {
        Engine::new_with_filter_set(FilterSet::new(false))
    });

    info!("Engine worker started with {} rules",
        engine.get_debug_info().source_info.iter().map(|s| s.network_filter_count).sum::<u32>());

    // Signal that the worker is ready
    notify.notify_one();

    // Periodically refresh engine from disk (once per hour)
    let mut last_rebuild = std::time::Instant::now();

    while let Some(msg) = receiver.blocking_recv() {
        // Rebuild engine periodically to pick up any changes to filter files
        if last_rebuild.elapsed() > Duration::from_secs(3600) {
            if let Ok(new_engine) = build_engine_from_dir(lists_dir) {
                engine = new_engine;
                last_rebuild = std::time::Instant::now();
                info!("Engine auto-refreshed from disk");
            }
        }

        match msg {
            EngineMsg::IsBlocked { domain, reply_tx } => {
                let url = format!("http://{}", domain);
                // FIX: Use domain as source_url so the engine treats this as first-party navigation.
                // With empty source_url, third-party filters incorrectly match first-party domains,
                // causing google.com, youtube.com, brave.com etc. to be blocked.
                let request = Request::preparsed(&url, &domain, &domain, "document", false, "get");
                let result = engine.check_network_request(&request).should_block();
                let _ = reply_tx.send(result);
            }
            EngineMsg::Rebuild => {
                if let Ok(new_engine) = build_engine_from_dir(lists_dir) {
                    engine = new_engine;
                    info!("Engine rebuilt on demand");
                }
            }
        }
    }
}

/// Build an adblock Engine from all .txt filter list files in the given directory.
fn build_engine_from_dir(dir: &PathBuf) -> Result<Engine> {
    let mut filter_set = FilterSet::new(false);

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "txt") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    filter_set.add_filter_list(content, ParseOptions::default());
                }
            }
        }
    }

    Ok(Engine::new_with_filter_set(filter_set))
}
