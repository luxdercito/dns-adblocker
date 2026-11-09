use adblock::{Engine, FilterSet, request::Request};
use adblock::lists::ParseOptions;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{info, warn};

const DEFAULT_FILTER_LISTS: &[(&str, &str)] = &[
    ("EasyList", "https://easylist-downloads.adblockplus.org/easylist.txt"),
    ("EasyPrivacy", "https://easylist-downloads.adblockplus.org/easyprivacy.txt"),
    ("EasyListCookie", "https://easylist-downloads.adblockplus.org/easylistcookie.txt"),
    ("FanboyAnnoyances", "https://easylist-downloads.adblockplus.org/fanboy-annoyance.txt"),
    ("FanboyAntiNewsletter", "https://easylist-downloads.adblockplus.org/fanboy-annoyance.txt"),
    ("FanboySocial", "https://easylist-downloads.adblockplus.org/fanboy-social.txt"),
    ("FanboyMobileNotifications", "https://easylist-downloads.adblockplus.org/fanboy-annoyance.txt"),
];

/// Message sent to the engine worker thread
pub enum EngineMsg {
    IsBlocked {
        domain: String,
        reply_tx: oneshot::Sender<bool>,
    },
}

/// Manages filter lists and owns the adblock engine in a background thread.
/// The engine is NOT Send+Sync (uses Rc internally), so it lives exclusively
/// in the worker thread. All queries go through the MPSC channel.
pub struct FilterListManager {
    pub sender: Arc<Mutex<mpsc::Sender<EngineMsg>>>,
    pub lists_dir: PathBuf,
    pub initialized: Arc<AtomicBool>,
}

impl Clone for FilterListManager {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            lists_dir: self.lists_dir.clone(),
            initialized: self.initialized.clone(),
        }
    }
}

impl FilterListManager {
    pub async fn new(lists_dir: PathBuf) -> Self {
        tokio::fs::create_dir_all(&lists_dir)
            .await
            .expect("Failed to create filter lists directory");

        let (sender, receiver) = std::sync::mpsc::channel::<EngineMsg>();
        let sender_arc = Arc::new(Mutex::new(sender));
        let initialized = Arc::new(AtomicBool::new(false));

        let worker_dir = lists_dir.clone();
        let init_flag = initialized.clone();
        // Run the engine worker in a dedicated blocking thread — safe because
        // Engine is not Send/Sync, so it must never leave this thread.
        tokio::task::spawn_blocking(move || {
            run_engine_worker(receiver, &worker_dir, init_flag);
        });

        Self {
            sender: sender_arc,
            lists_dir,
            initialized,
        }
    }

    /// Download default filter lists. Called from main() before starting server.
    pub async fn initialize(&self) {
        info!("Downloading default filter lists...");
        let client = reqwest::Client::builder()
            .user_agent("dns-adblocker/0.1")
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        for (name, url) in DEFAULT_FILTER_LISTS {
            let dest = self.lists_dir.join(format!("{}.txt", name));
            match Self::download_filter(&client, url, &dest).await {
                Ok(bytes) => info!("Downloaded {} ({} bytes)", name, bytes),
                Err(e) => warn!("Failed to download {}: {}", name, e),
            }
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

    /// Check if a domain should be blocked by sending a message to the engine worker.
    /// Blocks until the worker responds. Safe to call from any context.
    pub fn is_blocked(&self, domain: &str) -> bool {
        while !self.initialized.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let tx = self.sender.lock().unwrap();
            let _ = tx.send(EngineMsg::IsBlocked {
                domain: domain.to_string(),
                reply_tx,
            });
        }
        reply_rx.blocking_recv().unwrap_or(false)
    }
}

/// Main event loop for the engine worker task. Runs on a dedicated blocking thread.
/// The Engine lives here exclusively — it is never moved between threads.
fn run_engine_worker(
    receiver: mpsc::Receiver<EngineMsg>,
    lists_dir: &PathBuf,
    init_flag: Arc<AtomicBool>,
) {
    // Build initial engine from saved files (or empty if none exist)
    let mut engine = build_engine_from_dir(lists_dir).unwrap_or_else(|_| {
        Engine::new_with_filter_set(FilterSet::new(false))
    });

    // Signal that the worker is ready
    init_flag.store(true, Ordering::SeqCst);
    info!("Engine worker started with {} rules", 
        engine.get_debug_info().source_info.iter().map(|s| s.network_filter_count).sum::<u32>());

    // Periodically refresh engine from disk (once per hour)
    let mut last_rebuild = std::time::Instant::now();

    while let Ok(msg) = receiver.recv() {
        // Rebuild engine periodically to pick up any changes to filter files
        if last_rebuild.elapsed() > Duration::from_secs(3600) {
            if let Ok(new_engine) = build_engine_from_dir(lists_dir) {
                engine = new_engine;
                last_rebuild = std::time::Instant::now();
                info!("Engine refreshed from disk");
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
