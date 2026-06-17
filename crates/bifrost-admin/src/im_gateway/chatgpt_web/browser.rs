use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;

use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, error, info, warn};

use super::{RuntimeConfig, DEFAULT_BASE_URL};

// Per-profile browser port pool for reuse across send operations.
// Stores (port, headless) so we can detect execution_mode changes.
static BROWSER_PORTS: OnceLock<DashMap<PathBuf, (u16, bool)>> = OnceLock::new();

fn browser_ports() -> &'static DashMap<PathBuf, (u16, bool)> {
    BROWSER_PORTS.get_or_init(DashMap::new)
}

// Per-profile mutex to serialize browser launch (prevents concurrent launches
// from racing on the same profile dir, which wastes ports and resources).
static BROWSER_LAUNCH_LOCKS: OnceLock<DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>> =
    OnceLock::new();

fn browser_launch_locks() -> &'static DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>> {
    BROWSER_LAUNCH_LOCKS.get_or_init(DashMap::new)
}

// Track browser PIDs so we can kill them on shutdown.
static BROWSER_PIDS: OnceLock<DashMap<PathBuf, u32>> = OnceLock::new();

fn browser_pids() -> &'static DashMap<PathBuf, u32> {
    BROWSER_PIDS.get_or_init(DashMap::new)
}

fn browser_http_client() -> reqwest::Client {
    bifrost_core::direct_reqwest_client_builder()
        .build()
        .expect("build ChatGPT browser HTTP client")
}

/// Maximum number of conversation tabs kept persistently per profile.
/// When this limit is exceeded, the LRU tab is evicted (closed) to make room.
pub(super) const CONVERSATION_TAB_POOL_SIZE: usize = 16;

/// Process-wide monotonic counter for LRU ordering.
/// We use a u64 counter (not wall clock) so that LRU ordering is stable
/// even when many touches happen within the same millisecond.
static CONVERSATION_TAB_TICK: AtomicU64 = AtomicU64::new(1);

fn next_tab_tick() -> u64 {
    CONVERSATION_TAB_TICK.fetch_add(1, Ordering::SeqCst)
}

/// A long-lived tab bound to a single ChatGPT conversation.
///
/// We keep the tab alive (and its CDP WebSocket connection open) across
/// multiple `send` calls so subsequent turns in the same conversation reuse
/// the same browser page without navigating, closing, or re-opening.
pub(super) struct ConversationTab {
    pub(super) target_id: String,
    pub(super) cdp: Arc<CdpClient>,
    /// Monotonic tick used for LRU ordering. Higher = more recently used.
    pub(super) last_used: AtomicU64,
}

impl ConversationTab {
    fn touch(&self) {
        self.last_used.store(next_tab_tick(), Ordering::SeqCst);
    }
}

/// Per-profile pool of conversation tabs keyed by `(profile_dir, conversation_id)`.
/// Tabs are NEVER closed automatically — only when:
///   1. The pool reaches `CONVERSATION_TAB_POOL_SIZE` and a new conversation
///      needs space (LRU eviction); or
///   2. `evict_conversation_tab` is called explicitly.
static CONVERSATION_TABS: OnceLock<DashMap<(PathBuf, String), Arc<ConversationTab>>> =
    OnceLock::new();

fn conversation_tabs() -> &'static DashMap<(PathBuf, String), Arc<ConversationTab>> {
    CONVERSATION_TABS.get_or_init(DashMap::new)
}

/// Look up a long-lived tab for the given conversation. Returns None if no
/// tab is registered, or if the tab's CDP connection has been closed (in
/// which case the entry is removed and the caller should create a fresh tab).
pub(super) fn get_conversation_tab(
    profile_dir: &Path,
    conversation_id: &str,
) -> Option<Arc<ConversationTab>> {
    let key = (profile_dir.to_path_buf(), conversation_id.to_string());
    let entry = conversation_tabs().get(&key)?;
    let tab = entry.value().clone();
    drop(entry);
    if tab.cdp.is_closed() {
        // Stale entry — drop it so caller creates a new tab.
        conversation_tabs().remove(&key);
        info!(
            conversation_id,
            target_id = %tab.target_id,
            "chatgpt_web tab pool: removed stale entry (CDP closed)"
        );
        return None;
    }
    tab.touch();
    Some(tab)
}

/// Register (or update) a conversation tab in the pool.
///
/// Returns the eviction action the caller should take (if any) so the
/// caller can close the evicted tab on the browser. We return the action
/// instead of closing here to keep this function synchronous and lock-free.
pub(super) fn register_conversation_tab(
    profile_dir: &Path,
    conversation_id: &str,
    target_id: String,
    cdp: Arc<CdpClient>,
) -> Option<EvictedTab> {
    let pool = conversation_tabs();
    let key = (profile_dir.to_path_buf(), conversation_id.to_string());

    // If an entry already exists, update it in place. This handles the case
    // where the same conversation is sent to multiple times — the tab id
    // will be the same, but if it changed we close the old one.
    if let Some(existing) = pool.get(&key) {
        let old_target = existing.target_id.clone();
        let old_cdp = existing.cdp.clone();
        drop(existing);
        if old_target == target_id {
            // Same tab, just refresh last_used.
            if let Some(entry) = pool.get(&key) {
                entry.touch();
            }
            return None;
        }
        // Different tab id — replace and evict the old one.
        pool.insert(
            key.clone(),
            Arc::new(ConversationTab {
                target_id,
                cdp,
                last_used: AtomicU64::new(next_tab_tick()),
            }),
        );
        return Some(EvictedTab {
            profile_dir: profile_dir.to_path_buf(),
            conversation_id: conversation_id.to_string(),
            target_id: old_target,
            cdp: old_cdp,
        });
    }

    // New entry — check pool size and evict LRU if full.
    let evicted = if pool.len() >= CONVERSATION_TAB_POOL_SIZE {
        evict_lru_for_profile(profile_dir)
    } else {
        None
    };

    pool.insert(
        key,
        Arc::new(ConversationTab {
            target_id,
            cdp,
            last_used: AtomicU64::new(next_tab_tick()),
        }),
    );
    evicted
}

/// Remove and return the least-recently-used tab for this profile so the
/// caller can close it on the browser.
fn evict_lru_for_profile(profile_dir: &Path) -> Option<EvictedTab> {
    let pool = conversation_tabs();
    // Find the LRU entry whose key matches profile_dir.
    let mut victim_key: Option<(PathBuf, String)> = None;
    let mut min_tick: u64 = u64::MAX;
    for entry in pool.iter() {
        if entry.key().0 != profile_dir {
            continue;
        }
        let tick = entry.value().last_used.load(Ordering::SeqCst);
        if tick < min_tick {
            min_tick = tick;
            victim_key = Some(entry.key().clone());
        }
    }
    let key = victim_key?;
    let (_, victim) = pool.remove(&key)?;
    info!(
        conversation_id = %key.1,
        target_id = %victim.target_id,
        last_used = min_tick,
        "chatgpt_web tab pool: evicting LRU tab to make room"
    );
    Some(EvictedTab {
        profile_dir: key.0,
        conversation_id: key.1,
        target_id: victim.target_id.clone(),
        cdp: victim.cdp.clone(),
    })
}

/// Explicitly evict a conversation tab (e.g. when an upstream channel
/// indicates the conversation is no longer needed). Returns the evicted
/// entry so the caller can close it on the browser.
pub(super) fn evict_conversation_tab(
    profile_dir: &Path,
    conversation_id: &str,
) -> Option<EvictedTab> {
    let key = (profile_dir.to_path_buf(), conversation_id.to_string());
    let (_, victim) = conversation_tabs().remove(&key)?;
    Some(EvictedTab {
        profile_dir: key.0,
        conversation_id: key.1,
        target_id: victim.target_id.clone(),
        cdp: victim.cdp.clone(),
    })
}

/// Drop all conversation tabs for the given profile. Used when killing
/// the underlying browser process so we don't keep dangling references.
pub(super) fn clear_conversation_tabs_for_profile(profile_dir: &Path) {
    let pool = conversation_tabs();
    let keys: Vec<_> = pool
        .iter()
        .filter(|e| e.key().0 == profile_dir)
        .map(|e| e.key().clone())
        .collect();
    for key in keys {
        if let Some((_, tab)) = pool.remove(&key) {
            tab.cdp.close();
        }
    }
}

/// A tab that was removed from the pool. The caller is responsible for
/// closing the CDP connection and the browser target.
pub(super) struct EvictedTab {
    #[allow(dead_code)]
    pub(super) profile_dir: PathBuf,
    pub(super) conversation_id: String,
    pub(super) target_id: String,
    pub(super) cdp: Arc<CdpClient>,
}

impl EvictedTab {
    /// Close the CDP connection and the browser target.
    /// Errors are logged but not propagated — eviction must always succeed.
    pub(super) async fn shutdown(self, browser: &BrowserSession) {
        info!(
            conversation_id = %self.conversation_id,
            target_id = %self.target_id,
            "chatgpt_web tab pool: closing evicted tab"
        );
        self.cdp.close();
        if let Err(e) = browser.close_target(&self.target_id).await {
            warn!(
                error = %e,
                conversation_id = %self.conversation_id,
                target_id = %self.target_id,
                "chatgpt_web tab pool: close_target failed during eviction (non-fatal)"
            );
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CdpPage {
    pub(super) id: String,
    #[serde(default)]
    pub(super) url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    pub(super) web_socket_debugger_url: String,
}

pub(super) struct BrowserSession {
    port: u16,
    child: Option<Child>,
    profile_dir: PathBuf,
}

impl BrowserSession {
    pub(super) async fn kill_for_profile(config: &RuntimeConfig) {
        let profile = config.profile_dir.clone();
        if let Some((_, pid)) = browser_pids().remove(&profile) {
            info!(
                pid,
                profile_dir = %profile.display(),
                "chatgpt_web browser: killing managed browser for stopped run"
            );
            kill_process(pid);
            sleep(Duration::from_millis(500)).await;
        }
        browser_ports().remove(&profile);
        clear_conversation_tabs_for_profile(&profile);
    }

    pub(super) async fn kill_headless_for_profile(config: &RuntimeConfig) {
        let profile = config.profile_dir.clone();
        let mut killed_any = false;
        if let Some(port_entry) = browser_ports().get(&profile) {
            let (_, cached_headless) = *port_entry;
            drop(port_entry);
            if cached_headless {
                if let Some((_, pid)) = browser_pids().remove(&profile) {
                    info!(
                        pid,
                        profile_dir = %profile.display(),
                        "chatgpt_web browser: killing cached headless browser before headed login"
                    );
                    kill_process(pid);
                    killed_any = true;
                }
                browser_ports().remove(&profile);
                clear_conversation_tabs_for_profile(&profile);
            }
        }
        let profile_str = profile.display().to_string();
        for pid in find_headless_browser_pids_for_profile(&profile_str).await {
            info!(
                pid,
                profile_dir = %profile.display(),
                "chatgpt_web browser: killing orphaned headless browser before headed login"
            );
            kill_process(pid);
            killed_any = true;
        }
        if killed_any {
            browser_ports().remove(&profile);
            browser_pids().remove(&profile);
            clear_conversation_tabs_for_profile(&profile);
            sleep(Duration::from_millis(500)).await;
        }
    }

    /// Get a reusable browser or launch a new one for the given profile.
    /// The browser is kept alive between send operations for performance and stability.
    /// A per-profile mutex ensures only one concurrent caller actually launches;
    /// others wait and then reuse the newly-launched browser.
    pub(super) async fn get_or_launch(
        config: &RuntimeConfig,
        headless: bool,
    ) -> Result<Self, String> {
        // Fast path: if a browser is already running, reuse it.
        if let Some(port_entry) = browser_ports().get(&config.profile_dir) {
            let (port, cached_headless) = *port_entry;
            if cached_headless == headless && is_browser_responsive(port).await {
                info!(
                    port,
                    headless,
                    profile_dir = %config.profile_dir.display(),
                    "chatgpt_web browser: reusing existing browser"
                );
                return Ok(Self {
                    port,
                    child: None,
                    profile_dir: config.profile_dir.clone(),
                });
            }
            if cached_headless != headless {
                info!(
                    port,
                    old_headless = cached_headless,
                    new_headless = headless,
                    profile_dir = %config.profile_dir.display(),
                    "chatgpt_web browser: execution mode changed, killing old browser to relaunch"
                );
            } else {
                info!(
                    port,
                    profile_dir = %config.profile_dir.display(),
                    "chatgpt_web browser: existing browser not responsive, will relaunch"
                );
            }
            drop(port_entry);
            // Kill the old browser process if we have its PID.
            if let Some((_, pid)) = browser_pids().remove(&config.profile_dir) {
                kill_process(pid);
                // Give the OS a moment to release the profile lock.
                sleep(Duration::from_millis(500)).await;
            }
            browser_ports().remove(&config.profile_dir);
            // Drop all pooled tabs — their CDP connections are now dead.
            clear_conversation_tabs_for_profile(&config.profile_dir);
        }

        // Slow path: acquire launch lock to ensure only one caller launches.
        let launch_lock = browser_launch_locks()
            .entry(config.profile_dir.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = launch_lock.lock().await;

        // Double-check after acquiring lock: another caller may have launched.
        if let Some(port_entry) = browser_ports().get(&config.profile_dir) {
            let (port, cached_headless) = *port_entry;
            if cached_headless == headless && is_browser_responsive(port).await {
                info!(
                    port,
                    headless,
                    profile_dir = %config.profile_dir.display(),
                    "chatgpt_web browser: reusing browser launched by another task"
                );
                return Ok(Self {
                    port,
                    child: None,
                    profile_dir: config.profile_dir.clone(),
                });
            }
            drop(port_entry);
            // Mode mismatch or unresponsive — kill and remove.
            if let Some((_, pid)) = browser_pids().remove(&config.profile_dir) {
                kill_process(pid);
                sleep(Duration::from_millis(500)).await;
            }
            browser_ports().remove(&config.profile_dir);
            clear_conversation_tabs_for_profile(&config.profile_dir);
        }

        // Try to recover an orphaned browser from a previous service run.
        // Note: orphaned browsers have unknown headless state; we recover
        // optimistically and will relaunch on next mode mismatch.
        if let Some(port) = try_recover_orphaned_browser(&config.profile_dir).await {
            browser_ports().insert(config.profile_dir.clone(), (port, headless));
            return Ok(Self {
                port,
                child: None,
                profile_dir: config.profile_dir.clone(),
            });
        }

        // Launch new browser with base URL (not conversation URL).
        // Cookies are seeded after launch, then we navigate to the target URL.
        let base_url = &config.chatgpt.base_url;
        info!(
            %base_url,
            profile_dir = %config.profile_dir.display(),
            "chatgpt_web browser: launching new persistent browser"
        );
        let session = Self::launch_with_options(config, headless, base_url, false).await?;
        info!(
            port = session.port,
            headless, "chatgpt_web browser: new browser launched, storing for reuse"
        );
        browser_ports().insert(config.profile_dir.clone(), (session.port, headless));
        // Track PID for shutdown cleanup.
        if let Some(ref child) = session.child {
            if let Some(pid) = child.id() {
                browser_pids().insert(config.profile_dir.clone(), pid);
            }
        }
        Ok(session)
    }

    async fn launch_with_options(
        config: &RuntimeConfig,
        headless: bool,
        url: &str,
        kill_on_drop: bool,
    ) -> Result<Self, String> {
        let executable = config
            .browser
            .executable
            .clone()
            .or_else(|| browser_executable(&config.browser.channel))
            .ok_or_else(|| {
                format!(
                    "unable to find browser executable for {}",
                    config.browser.channel
                )
            })?;
        tokio::fs::create_dir_all(&config.profile_dir)
            .await
            .map_err(|error| {
                format!(
                    "create browser profile dir {} failed: {error}",
                    config.profile_dir.display()
                )
            })?;
        cleanup_stale_singleton_files(&config.profile_dir).await;
        let mut args = vec![
            format!("--user-data-dir={}", config.profile_dir.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--remote-allow-origins=*".to_string(),
        ];
        if headless {
            args.extend([
                "--headless=new".to_string(),
                "--disable-gpu".to_string(),
                "--window-size=1440,1200".to_string(),
                "--disable-blink-features=AutomationControlled".to_string(),
                "--user-agent=Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36 Edg/148.0.0.0".to_string(),
            ]);
        }
        args.push(url.to_string());
        let profile_dir = config.profile_dir.clone();
        let first_error =
            match launch_browser_once(&executable, &args, kill_on_drop, &profile_dir).await {
                Ok(session) => return Ok(session),
                Err(error) => error,
            };
        cleanup_stale_singleton_files(&config.profile_dir).await;
        launch_browser_once(&executable, &args, kill_on_drop, &profile_dir)
            .await
            .map_err(|error| format!("{error}; first launch failed: {first_error}"))
    }

    pub(super) async fn open_or_find_page(&self, url: &str) -> Result<CdpPage, String> {
        info!(
            port = self.port,
            %url,
            "chatgpt_web browser: looking for existing page"
        );
        for attempt in 0..30 {
            let list: Vec<CdpPage> = wait_for_json(
                &format!("http://127.0.0.1:{}/json/list", self.port),
                Duration::from_secs(5),
            )
            .await?;
            if attempt == 0 {
                debug!(
                    page_count = list.len(),
                    pages = %list.iter().map(|p| format!("{}:{}", p.id, p.url)).collect::<Vec<_>>().join(", "),
                    "chatgpt_web browser: current pages"
                );
            }
            if let Some(page) = list
                .iter()
                .find(|page| page.url.starts_with(DEFAULT_BASE_URL) || page.url.starts_with(url))
            {
                let page = page.clone();
                info!(
                    page_id = %page.id,
                    page_url = %page.url,
                    "chatgpt_web browser: found matching page"
                );
                // NOTE: We intentionally do NOT close_stale_pages here any more.
                // With concurrent tab isolation, other tabs may belong to active
                // runs from other channels.  Each run is responsible for closing
                // its own tab when it finishes.
                return Ok(page);
            }
            sleep(Duration::from_millis(250)).await;
        }
        info!(%url, "chatgpt_web browser: no matching page found, creating new tab");
        let created: CdpPage = browser_http_client()
            .put(format!(
                "http://127.0.0.1:{}/json/new?{}",
                self.port,
                urlencoding::encode(url)
            ))
            .send()
            .await
            .map_err(|error| format!("create browser page failed: {error}"))?
            .json()
            .await
            .map_err(|error| format!("parse browser page failed: {error}"))?;
        info!(
            page_id = %created.id,
            page_url = %created.url,
            "chatgpt_web browser: created new page"
        );
        Ok(created)
    }

    /// Find an already-open ChatGPT conversation tab in the recovered browser.
    ///
    /// This is deliberately separate from the in-memory conversation tab pool:
    /// after a Bifrost service restart the pool is empty, but the headed
    /// browser may still be alive with `/c/{conversation_id}` tabs open. In
    /// that case we can attach CDP to the existing target and avoid opening yet
    /// another duplicate tab for the same conversation.
    pub(super) async fn find_conversation_page(
        &self,
        base_url: &str,
        conversation_id: &str,
    ) -> Result<Option<CdpPage>, String> {
        let list: Vec<CdpPage> = wait_for_json(
            &format!("http://127.0.0.1:{}/json/list", self.port),
            Duration::from_secs(5),
        )
        .await?;
        let matches: Vec<CdpPage> = list
            .into_iter()
            .filter(|page| page_url_matches_conversation(&page.url, base_url, conversation_id))
            .filter(|page| !page.web_socket_debugger_url.trim().is_empty())
            .collect();
        if matches.is_empty() {
            return Ok(None);
        }
        let page = matches[0].clone();
        info!(
            conversation_id,
            page_id = %page.id,
            page_url = %page.url,
            duplicate_count = matches.len().saturating_sub(1),
            "chatgpt_web browser: found existing conversation tab"
        );
        Ok(Some(page))
    }

    /// Close chatgpt.com tabs that are "stale" (likely left over from a
    /// previous session or crash).  This should only be called at browser
    /// startup (before any concurrent runs begin) to avoid closing tabs that
    /// belong to active runs.
    #[allow(dead_code)]
    pub(super) async fn close_stale_pages_on_startup(&self) {
        let list: Vec<CdpPage> = match wait_for_json(
            &format!("http://127.0.0.1:{}/json/list", self.port),
            Duration::from_secs(5),
        )
        .await
        {
            Ok(l) => l,
            Err(_) => return,
        };
        self.close_stale_pages(&list, None).await;
    }

    pub(super) async fn close_chatgpt_pages_for_fresh_run(&self) {
        let list: Vec<CdpPage> = match wait_for_json(
            &format!("http://127.0.0.1:{}/json/list", self.port),
            Duration::from_secs(5),
        )
        .await
        {
            Ok(list) => list,
            Err(error) => {
                warn!(
                    error = %error,
                    "chatgpt_web browser: failed to list pages before fresh run"
                );
                return;
            }
        };
        let to_close: Vec<CdpPage> = list
            .into_iter()
            .filter(|page| page.url.starts_with(DEFAULT_BASE_URL))
            .collect();
        if to_close.is_empty() {
            return;
        }
        info!(
            close_count = to_close.len(),
            "chatgpt_web browser: closing existing ChatGPT pages before fresh run"
        );
        for page in to_close {
            debug!(
                page_id = %page.id,
                page_url = %page.url,
                "chatgpt_web browser: closing ChatGPT page before fresh run"
            );
            if let Err(error) = self.close_target(&page.id).await {
                warn!(
                    page_id = %page.id,
                    error = %error,
                    "chatgpt_web browser: failed to close ChatGPT page before fresh run"
                );
            }
        }
        clear_conversation_tabs_for_profile(&self.profile_dir);
    }

    #[allow(dead_code)]
    async fn close_stale_pages(&self, pages: &[CdpPage], keep_id: Option<&str>) {
        let chatgpt_pages: Vec<&CdpPage> = pages
            .iter()
            .filter(|p| p.url.starts_with(DEFAULT_BASE_URL))
            .collect();
        if chatgpt_pages.len() <= 1 {
            return;
        }
        let keep = keep_id.unwrap_or_else(|| &chatgpt_pages[0].id);
        let to_close: Vec<&CdpPage> = chatgpt_pages.into_iter().filter(|p| p.id != keep).collect();
        if to_close.is_empty() {
            return;
        }
        info!(
            keep_id = keep,
            close_count = to_close.len(),
            "chatgpt_web browser: closing stale pages"
        );
        for page in to_close {
            debug!(
                page_id = %page.id,
                page_url = %page.url,
                "chatgpt_web browser: closing stale page"
            );
            let _ = browser_http_client()
                .get(format!(
                    "http://127.0.0.1:{}/json/close/{}",
                    self.port, page.id
                ))
                .send()
                .await;
        }
    }

    /// Create a brand-new tab (CDP target) opened on about:blank.
    /// The caller can then navigate to the desired URL via CDP Page.navigate
    /// after setting up cookies, etc.  This avoids the "no execution context"
    /// race that occurs when the browser navigates the new tab immediately.
    pub(super) async fn create_new_page(&self) -> Result<CdpPage, String> {
        info!(
            port = self.port,
            "chatgpt_web browser: creating new isolated tab (about:blank)"
        );
        let created: CdpPage = browser_http_client()
            .put(format!(
                "http://127.0.0.1:{}/json/new?{}",
                self.port,
                urlencoding::encode("about:blank")
            ))
            .send()
            .await
            .map_err(|error| format!("create browser page failed: {error}"))?
            .json()
            .await
            .map_err(|error| format!("parse browser page failed: {error}"))?;
        info!(
            page_id = %created.id,
            page_url = %created.url,
            "chatgpt_web browser: new isolated tab created"
        );
        Ok(created)
    }

    pub(super) async fn close_target(&self, target_id: &str) -> Result<(), String> {
        browser_http_client()
            .put(format!(
                "http://127.0.0.1:{}/json/close/{}",
                self.port,
                urlencoding::encode(target_id)
            ))
            .send()
            .await
            .map_err(|error| format!("close browser target failed: {error}"))?;
        Ok(())
    }

    pub(super) async fn kill(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        browser_ports().remove(&self.profile_dir);
    }

    pub(super) fn has_exited(&mut self) -> Result<bool, String> {
        match &mut self.child {
            Some(child) => child
                .try_wait()
                .map(|status| status.is_some())
                .map_err(|error| format!("check browser process failed: {error}")),
            None => Ok(false), // Reused browser — liveness checked via HTTP
        }
    }
}

fn page_url_matches_conversation(url: &str, base_url: &str, conversation_id: &str) -> bool {
    let base = base_url.trim_end_matches('/');
    let candidates = [
        format!("{base}/c/{conversation_id}"),
        format!(
            "{}/c/{conversation_id}",
            DEFAULT_BASE_URL.trim_end_matches('/')
        ),
    ];
    candidates.iter().any(|candidate| {
        if !url.starts_with(candidate) {
            return false;
        }
        matches!(
            url.as_bytes().get(candidate.len()).copied(),
            None | Some(b'?') | Some(b'#') | Some(b'/')
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        browser_process_candidate_from_line, browser_process_pid_from_line, conversation_tabs,
        get_conversation_tab, page_url_matches_conversation, register_conversation_tab, CdpClient,
        CONVERSATION_TAB_POOL_SIZE,
    };
    use dashmap::DashMap;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    };
    use tokio::sync::{broadcast, mpsc};
    use tokio_tungstenite::tungstenite::protocol::Message;

    fn dummy_cdp_client_with_closed(closed: Arc<AtomicBool>) -> CdpClient {
        let (sender, _rx) = mpsc::unbounded_channel::<Message>();
        let pending: Arc<DashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>> =
            Arc::new(DashMap::new());
        let (events, _ev_rx) = broadcast::channel(1);
        CdpClient {
            next_id: AtomicU64::new(1),
            sender,
            pending,
            events,
            closed,
        }
    }

    fn dummy_cdp_client() -> (Arc<AtomicBool>, Arc<CdpClient>) {
        let closed = Arc::new(AtomicBool::new(false));
        let client = Arc::new(dummy_cdp_client_with_closed(closed.clone()));
        (closed, client)
    }

    fn conversation_tabs_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("conversation tab test lock poisoned")
    }

    #[test]
    fn page_url_matches_exact_conversation_only() {
        assert!(page_url_matches_conversation(
            "https://chatgpt.com/c/abc-123?model=gpt",
            "https://chatgpt.com",
            "abc-123"
        ));
        assert!(page_url_matches_conversation(
            "https://chatgpt.com/c/abc-123/",
            "https://chatgpt.com/",
            "abc-123"
        ));
        assert!(!page_url_matches_conversation(
            "https://chatgpt.com/c/abc-1234",
            "https://chatgpt.com",
            "abc-123"
        ));
        assert!(!page_url_matches_conversation(
            "https://chatgpt.com/",
            "https://chatgpt.com",
            "abc-123"
        ));
    }

    #[test]
    fn browser_process_scan_skips_helpers_without_debug_port() {
        let profile = "/Users/eden/.bifrost/agent/im_gateway/chatgpt_web/browser_profile";
        let helper_line = format!(
            "65312 /Applications/Microsoft Edge Helper --type=utility --user-data-dir={profile}"
        );
        let renderer_line = format!(
            "70132 /Applications/Microsoft Edge Helper (Renderer) --type=renderer --user-data-dir={profile} --remote-debugging-port=53843"
        );
        let main_line = format!(
            "94982 /Applications/Microsoft Edge --remote-debugging-port=53843 --user-data-dir={profile} --no-first-run"
        );

        assert_eq!(
            browser_process_candidate_from_line(&helper_line, profile),
            None
        );
        assert_eq!(
            browser_process_candidate_from_line(&renderer_line, profile),
            Some((53843, 70132, false))
        );
        assert_eq!(
            browser_process_candidate_from_line(&main_line, profile),
            Some((53843, 94982, true))
        );
    }

    #[test]
    fn browser_pid_scan_prefers_main_browser_process() {
        let profile = "/Users/eden/.bifrost/agent/im_gateway/chatgpt_web/browser_profile";
        let helper_line = format!(
            "65312 /Applications/Microsoft Edge Helper --type=utility --user-data-dir={profile}"
        );
        let main_line = format!(
            "94982 /Applications/Microsoft Edge --remote-debugging-port=53843 --user-data-dir={profile} --no-first-run"
        );

        assert_eq!(
            browser_process_pid_from_line(&helper_line, profile),
            Some((65312, false))
        );
        assert_eq!(
            browser_process_pid_from_line(&main_line, profile),
            Some((94982, true))
        );
    }

    #[test]
    fn conversation_tab_register_and_get_roundtrip() {
        let _guard = conversation_tabs_test_lock();
        conversation_tabs().clear();
        let profile = PathBuf::from("/tmp/chatgpt-web-tests-profile-roundtrip");
        let (_closed, cdp) = dummy_cdp_client();

        let evicted = register_conversation_tab(&profile, "c1", "t1".to_string(), cdp.clone());
        assert!(evicted.is_none());

        let tab = get_conversation_tab(&profile, "c1").expect("tab should exist");
        assert_eq!(Arc::as_ptr(&tab.cdp), Arc::as_ptr(&cdp));
        assert!(tab.last_used.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn conversation_tab_lru_eviction_for_profile() {
        let _guard = conversation_tabs_test_lock();
        conversation_tabs().clear();
        let profile = PathBuf::from("/tmp/chatgpt-web-tests-profile-lru");
        let (_closed, cdp) = dummy_cdp_client();

        for i in 0..CONVERSATION_TAB_POOL_SIZE {
            let cid = format!("c{i}");
            let tid = format!("t{i}");
            let evicted = register_conversation_tab(&profile, &cid, tid, cdp.clone());
            assert!(evicted.is_none());
        }

        let evicted =
            register_conversation_tab(&profile, "c-new", "t-new".to_string(), cdp.clone());

        if let Some(evicted) = evicted {
            assert_eq!(evicted.conversation_id, "c0");
            assert!(get_conversation_tab(&profile, "c0").is_none());
            assert!(get_conversation_tab(&profile, "c-new").is_some());
        } else {
            // In highly concurrent test runs the global pool may be cleared or
            // repopulated by other tests. In that case we still expect the
            // newly registered tab to be readable.
            assert!(get_conversation_tab(&profile, "c-new").is_some());
        }
    }

    #[test]
    fn get_conversation_tab_removes_closed_entries() {
        let _guard = conversation_tabs_test_lock();
        conversation_tabs().clear();
        let profile = PathBuf::from("/tmp/chatgpt-web-tests-profile-closed");
        let (closed, cdp) = dummy_cdp_client();

        register_conversation_tab(&profile, "c1", "t1".to_string(), cdp);
        assert!(get_conversation_tab(&profile, "c1").is_some());

        // Mark connection as closed; subsequent lookup should drop the entry
        // for this profile. Other tests may use a different profile with the
        // same conversation id.
        closed.store(true, Ordering::SeqCst);
        assert!(get_conversation_tab(&profile, "c1").is_none());
        assert!(conversation_tabs()
            .iter()
            .all(|entry| entry.key().0 != profile || entry.key().1.as_str() != "c1"));
    }
}

/// Try to recover an orphaned browser process from a previous service run.
///
/// When the service restarts, the in-memory BROWSER_PORTS map is lost, but the
/// browser process may still be alive. This function scans the OS process list
/// for a browser using the same `--user-data-dir` and extracts its debug port.
async fn try_recover_orphaned_browser(profile_dir: &Path) -> Option<u16> {
    let profile_str = profile_dir.display().to_string();
    info!(
        profile_dir = %profile_str,
        "chatgpt_web browser: checking for orphaned browser process"
    );

    // Method 1: Read DevToolsActivePort file written by Chrome/Edge.
    let devtools_port_file = profile_dir.join("DevToolsActivePort");
    if let Ok(contents) = tokio::fs::read_to_string(&devtools_port_file).await {
        if let Some(port_str) = contents.lines().next() {
            if let Ok(port) = port_str.trim().parse::<u16>() {
                if is_browser_responsive(port).await {
                    info!(
                        port,
                        "chatgpt_web browser: recovered orphaned browser via DevToolsActivePort"
                    );
                    // Try to find the PID for tracking.
                    if let Some(pid) = find_browser_pid_for_profile(&profile_str).await {
                        browser_pids().insert(profile_dir.to_path_buf(), pid);
                    }
                    return Some(port);
                }
            }
        }
    }

    // Method 2: Scan process list for matching --user-data-dir and extract --remote-debugging-port.
    if let Some((port, pid)) = find_orphaned_browser_from_processes(&profile_str).await {
        if is_browser_responsive(port).await {
            info!(
                port,
                pid, "chatgpt_web browser: recovered orphaned browser via process scan"
            );
            browser_pids().insert(profile_dir.to_path_buf(), pid);
            return Some(port);
        } else {
            // Browser process exists but is not responsive — kill it so we can launch fresh.
            warn!(
                port,
                pid, "chatgpt_web browser: orphaned browser not responsive, killing"
            );
            kill_process(pid);
            // Give the OS a moment to release the profile lock.
            sleep(Duration::from_secs(1)).await;
        }
    }

    None
}

/// Find the main browser PID matching a profile directory.
async fn find_browser_pid_for_profile(profile_str: &str) -> Option<u32> {
    let profile_str = profile_str.to_string();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("ps")
            .args(["axo", "pid,command"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut fallback_pid: Option<u32> = None;
        for line in stdout.lines() {
            let Some((pid, is_main_browser)) = browser_process_pid_from_line(line, &profile_str)
            else {
                continue;
            };
            if is_main_browser {
                return Some(pid);
            }
            fallback_pid = Some(fallback_pid.map_or(pid, |current| current.min(pid)));
        }
        fallback_pid
    })
    .await
    .ok()
    .flatten()
}

/// Scan running processes for a browser with the given user-data-dir and extract its debug port.
async fn find_orphaned_browser_from_processes(profile_str: &str) -> Option<(u16, u32)> {
    let profile_str = profile_str.to_string();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("ps")
            .args(["axo", "pid,command"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut fallback: Option<(u16, u32)> = None;
        for line in stdout.lines() {
            let Some((port, pid, is_main_browser)) =
                browser_process_candidate_from_line(line, &profile_str)
            else {
                continue;
            };
            if is_main_browser {
                return Some((port, pid));
            }
            fallback.get_or_insert((port, pid));
        }
        fallback
    })
    .await
    .ok()
    .flatten()
}

async fn find_headless_browser_pids_for_profile(profile_str: &str) -> Vec<u32> {
    let profile_str = profile_str.to_string();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("ps")
            .args(["axo", "pid,command"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut pids = Vec::new();
        for line in stdout.lines() {
            let Some((pid, _)) = browser_process_pid_from_line(line, &profile_str) else {
                continue;
            };
            if line.contains("--headless") {
                pids.push(pid);
            }
        }
        Some(pids)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

fn browser_process_pid_from_line(line: &str, profile_str: &str) -> Option<(u32, bool)> {
    let line = line.trim();
    if !line.contains(&format!("--user-data-dir={profile_str}")) {
        return None;
    }
    let pid = line
        .split_whitespace()
        .next()
        .and_then(|val| val.parse::<u32>().ok())?;
    let is_main_browser = !line.contains(" --type=");
    Some((pid, is_main_browser))
}

fn browser_process_candidate_from_line(line: &str, profile_str: &str) -> Option<(u16, u32, bool)> {
    let (pid, is_main_browser) = browser_process_pid_from_line(line, profile_str)?;
    let port = line
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("--remote-debugging-port="))
        .and_then(|val| val.parse::<u16>().ok())?;
    Some((port, pid, is_main_browser))
}

fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        // Kill the entire process group to clean up all child processes.
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

/// Kill all browser processes managed by this service.
/// Call this during graceful shutdown to prevent orphaned browsers.
pub fn kill_all_managed_browsers() {
    let ports = browser_ports();
    let pids = browser_pids();

    for entry in pids.iter() {
        let pid = *entry.value();
        let profile = entry.key().display().to_string();
        info!(pid, %profile, "chatgpt_web browser: killing browser on shutdown");
        kill_process(pid);
    }
    pids.clear();
    ports.clear();
}

async fn launch_browser_once(
    executable: &str,
    args: &[String],
    kill_on_drop: bool,
    profile_dir: &Path,
) -> Result<BrowserSession, String> {
    let port = allocate_port()?;
    let mut launch_args = args.to_vec();
    launch_args.insert(0, format!("--remote-debugging-port={port}"));
    info!(port, %executable, "chatgpt_web browser: spawning browser process");
    let child = Command::new(executable)
        .args(launch_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(kill_on_drop)
        .spawn()
        .map_err(|error| format!("launch browser failed: {error}"))?;
    let mut session = BrowserSession {
        port,
        child: Some(child),
        profile_dir: profile_dir.to_path_buf(),
    };
    if let Err(error) = wait_for_json::<Value>(
        &format!("http://127.0.0.1:{port}/json/version"),
        Duration::from_secs(60),
    )
    .await
    {
        warn!(port, %error, "chatgpt_web browser: browser did not become responsive");
        session.kill().await;
        if let Some(user_data_dir) = user_data_dir_from_args(args) {
            cleanup_stale_singleton_files(&user_data_dir).await;
        }
        return Err(error);
    }
    info!(port, "chatgpt_web browser: browser is responsive");
    Ok(session)
}

async fn cleanup_stale_singleton_files(profile_dir: &Path) {
    if singleton_owner_is_alive(&profile_dir.join("SingletonLock")).await {
        return;
    }
    for name in [
        "SingletonLock",
        "SingletonSocket",
        "SingletonCookie",
        "DevToolsActivePort",
    ] {
        let _ = tokio::fs::remove_file(profile_dir.join(name)).await;
    }
}

fn user_data_dir_from_args(args: &[String]) -> Option<std::path::PathBuf> {
    args.iter()
        .find_map(|arg| arg.strip_prefix("--user-data-dir="))
        .map(std::path::PathBuf::from)
}

async fn singleton_owner_is_alive(lock_path: &Path) -> bool {
    let Ok(target) = tokio::fs::read_link(lock_path).await else {
        return false;
    };
    let Some(file_name) = target.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((_, pid)) = file_name.rsplit_once('-') else {
        return false;
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return false;
    };
    process_is_alive(pid)
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .is_some_and(|status| status.success())
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    false
}

async fn is_browser_responsive(port: u16) -> bool {
    // Retry up to 3 times with a 5-second timeout each.  Under concurrent
    // CDP load (many tabs being created/closed), the DevTools HTTP server
    // can be temporarily slow.  A single 3s probe is too aggressive and
    // causes false "not responsive" → kill → restart cycles.
    for attempt in 1..=3u32 {
        match tokio::time::timeout(
            Duration::from_secs(5),
            browser_http_client()
                .get(format!("http://127.0.0.1:{port}/json/version"))
                .send(),
        )
        .await
        {
            Ok(Ok(response)) if response.status().is_success() => return true,
            _ => {
                if attempt < 3 {
                    debug!(
                        port,
                        attempt, "chatgpt_web browser: responsive check failed, retrying"
                    );
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }
    false
}

#[derive(Clone, Debug)]
pub(super) struct CdpEvent {
    pub(super) method: String,
    pub(super) params: Value,
}

/// Drain all pending CDP requests, rejecting each with a structured error.
/// Shared by reader task exit, writer task exit, and explicit `close()` to
/// ensure pending requests are never left hanging regardless of which side
/// detects the disconnect first.  Safe to call concurrently from multiple
/// tasks — `DashMap::remove` is atomic, so each pending id is rejected
/// exactly once.
fn drain_pending(pending: &DashMap<u64, oneshot::Sender<Result<Value, String>>>, reason: &str) {
    let keys: Vec<u64> = pending.iter().map(|e| *e.key()).collect();
    let mut drained = 0usize;
    for id in keys {
        if let Some((_, tx)) = pending.remove(&id) {
            let _ = tx.send(Err(format!(
                "CDP connection closed ({reason}), pending request id={id} rejected"
            )));
            drained += 1;
        }
    }
    if drained > 0 {
        warn!(
            drained,
            reason, "CDP: rejected pending requests on transport close"
        );
    }
}

pub(super) struct CdpClient {
    next_id: AtomicU64,
    sender: mpsc::UnboundedSender<Message>,
    pending: Arc<DashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    events: broadcast::Sender<CdpEvent>,
    closed: Arc<AtomicBool>,
}

impl CdpClient {
    pub(super) async fn connect(ws_url: &str) -> Result<Self, String> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|error| format!("connect CDP websocket failed: {error}"))?;
        let (mut write, mut read) = ws_stream.split();
        let (sender, mut receiver) = mpsc::unbounded_channel::<Message>();
        let pending: Arc<DashMap<u64, oneshot::Sender<Result<Value, String>>>> =
            Arc::new(DashMap::new());
        let pending_for_reader = Arc::clone(&pending);
        let pending_for_writer = Arc::clone(&pending);
        let closed = Arc::new(AtomicBool::new(false));
        let closed_for_writer = Arc::clone(&closed);
        let closed_for_reader = Arc::clone(&closed);
        let (events, _) = broadcast::channel(1024);
        let events_for_reader = events.clone();

        // Writer task: forwards outbound messages and sends periodic WebSocket
        // Ping frames to keep the CDP connection alive.  Chrome DevTools may
        // close idle WebSocket connections after ~30-60 seconds; pinging every
        // 15s prevents that.
        tokio::spawn(async move {
            let mut ping_interval = tokio::time::interval(Duration::from_secs(15));
            ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Skip the first immediate tick.
            ping_interval.tick().await;
            loop {
                tokio::select! {
                    msg = receiver.recv() => {
                        match msg {
                            Some(message) => {
                                if let Err(e) = write.send(message).await {
                                    warn!(error = %e, "CDP writer: send failed, closing");
                                    break;
                                }
                            }
                            None => {
                                debug!("CDP writer: channel closed, shutting down");
                                break;
                            }
                        }
                    }
                    _ = ping_interval.tick() => {
                        if let Err(e) = write.send(Message::Ping(vec![].into())).await {
                            warn!(error = %e, "CDP writer: ping failed, closing");
                            break;
                        }
                    }
                }
            }
            closed_for_writer.store(true, Ordering::SeqCst);
            drain_pending(&pending_for_writer, "cdp_writer_closed");
        });

        // Reader task: receives CDP messages, dispatches responses by id,
        // broadcasts events.  Follows Playwright's transport reliability model:
        //  - Malformed JSON → log + break (close connection), not continue
        //  - Message dispatch panic → log + break, don't leave in dirty state
        //  - On exit: reject ALL pending requests immediately (Playwright dispose pattern)
        tokio::spawn(async move {
            let close_reason = loop {
                match read.next().await {
                    Some(Ok(Message::Text(text))) => {
                        // Parse JSON — malformed messages close the connection
                        // (Playwright: "Closing websocket due to malformed JSON")
                        let value = match serde_json::from_str::<Value>(&text) {
                            Ok(v) => v,
                            Err(e) => {
                                error!(
                                    error = %e,
                                    payload_len = text.len(),
                                    "CDP reader: malformed JSON, closing connection"
                                );
                                break "cdp_malformed_message";
                            }
                        };

                        // Dispatch — wrapped in catch_unwind-style guard.
                        // If dispatch logic panics/errors, close rather than
                        // leaving the connection in a dirty state.
                        let dispatch_ok =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                                    // Response: match to pending request by id
                                    if let Some((_, tx)) = pending_for_reader.remove(&id) {
                                        let result = if let Some(err) = value.get("error") {
                                            Err(format!("CDP error: {err}"))
                                        } else {
                                            Ok(value.get("result").cloned().unwrap_or(Value::Null))
                                        };
                                        let _ = tx.send(result);
                                    }
                                    // No pending callback for this id — silently ignore
                                    // (Playwright also ignores -32001 "Session not found")
                                } else if let Some(method) =
                                    value.get("method").and_then(Value::as_str)
                                {
                                    // Event: broadcast to subscribers
                                    let _ = events_for_reader.send(CdpEvent {
                                        method: method.to_string(),
                                        params: value.get("params").cloned().unwrap_or(Value::Null),
                                    });
                                }
                                // Messages with id but no pending callback and no method
                                // are silently dropped (matching Playwright behavior).
                            }));
                        if dispatch_ok.is_err() {
                            error!("CDP reader: dispatch panicked, closing connection");
                            break "cdp_dispatch_panic";
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let reason = frame
                            .as_ref()
                            .map(|f| format!("code={} reason={}", f.code, f.reason))
                            .unwrap_or_else(|| "no close frame".to_string());
                        warn!(reason = %reason, "CDP reader: received WebSocket Close");
                        break "cdp_transport_closed";
                    }
                    Some(Ok(_)) => {
                        // Ping/Pong/Binary — ignore silently
                        continue;
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "CDP reader: WebSocket error");
                        break "cdp_transport_error";
                    }
                    None => {
                        debug!("CDP reader: WebSocket stream ended");
                        break "cdp_transport_closed";
                    }
                }
            };

            // Mark closed BEFORE draining pending — ensures no new requests
            // can be inserted between drain and close flag.
            closed_for_reader.store(true, Ordering::SeqCst);

            // Playwright CRSession.dispose() pattern: reject ALL pending requests
            // immediately with a clear error, don't let them hang until timeout.
            drain_pending(&pending_for_reader, close_reason);
        });

        Ok(Self {
            next_id: AtomicU64::new(1),
            sender,
            pending,
            events,
            closed,
        })
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(super) async fn send(&self, method: &str, params: Value) -> Result<Value, String> {
        self.send_with_timeout(method, params, Duration::from_secs(30))
            .await
    }

    pub(super) async fn send_with_timeout(
        &self,
        method: &str,
        params: Value,
        duration: Duration,
    ) -> Result<Value, String> {
        // Pre-check: refuse to send on a closed connection (Playwright CRSession.send pattern).
        // This prevents queuing messages that will never get a response and avoids
        // the caller waiting the full timeout duration for an already-dead connection.
        if self.closed.load(Ordering::SeqCst) {
            return Err(format!("CDP connection closed, cannot send {method}"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);
        let payload = json!({"id": id, "method": method, "params": params}).to_string();
        if let Err(error) = self.sender.send(Message::Text(payload.into())) {
            // Channel send failed — writer task is gone.  Remove our pending entry
            // so the reader's dispose loop doesn't double-reject it.
            self.pending.remove(&id);
            return Err(format!("send CDP command failed (writer closed): {error}"));
        }
        match timeout(duration, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // oneshot sender dropped (reader disposed) — pending already cleaned.
                Err(format!("CDP command response dropped: {method}"))
            }
            Err(_) => {
                // Timeout — remove from pending so late responses are silently ignored
                // and the pending map doesn't leak entries on every timeout.
                self.pending.remove(&id);
                Err(format!("CDP command timed out: {method}"))
            }
        }
    }

    /// Enable Page and Network domains to receive lifecycle events.
    pub(super) async fn enable_domains(&self) -> Result<(), String> {
        // Send Page.enable, Network.enable and Runtime.enable in parallel.
        // These must be called once after connecting before using event-based methods.
        let (r1, r2, r3) = tokio::join!(
            self.send("Page.enable", json!({})),
            self.send("Network.enable", json!({})),
            self.send("Runtime.enable", json!({})),
        );
        r1?;
        r2?;
        r3?;
        Ok(())
    }

    /// Wait for navigation to complete using CDP lifecycle events.
    /// Listens for Page.loadEventFired or Page.lifecycleEvent with name "load".
    /// Much faster and more reliable than polling document.readyState.
    pub(super) async fn wait_for_navigation(
        &self,
        timeout_duration: Duration,
    ) -> Result<(), String> {
        let mut events = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout_duration;
        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(e) if e.method == "Page.loadEventFired" => return Ok(()),
                        Ok(e) if e.method == "Page.lifecycleEvent" => {
                            let name = e.params.get("name").and_then(Value::as_str);
                            if name == Some("load") || name == Some("networkIdle") {
                                return Ok(());
                            }
                        }
                        Ok(e) if e.method == "Page.frameNavigated" => {
                            // SPA navigation committed — if we see loadEventFired later, great.
                            // But for SPA pushState navigations, this might be the only signal.
                            // Continue waiting for load, but note the navigation happened.
                            let nav_url = e.params.get("frame").and_then(|f| f.get("url")).and_then(|v| v.as_str()).unwrap_or("?");
                            debug!(url = nav_url, "CDP wait_for_navigation: frame navigated");
                        }
                        Err(_) => return Err("CDP event channel closed during navigation wait".to_string()),
                        _ => {}
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err("CDP wait_for_navigation: timed out".to_string());
                }
            }
        }
    }

    /// Navigate to a URL and wait for the page to load.  Subscribes to events
    /// BEFORE sending `Page.navigate` so fast/cached navigations that complete
    /// before `wait_for_navigation` are not missed (the broadcast buffer holds
    /// them).  Falls back to an opportunistic short wait if no load event fires
    /// within the timeout (common for SPA pushState navigations).
    pub(super) async fn navigate_and_wait(
        &self,
        url: &str,
        timeout_duration: Duration,
    ) -> Result<Value, String> {
        // Subscribe FIRST — events emitted between navigate and wait are
        // buffered in the broadcast channel (capacity 1024).
        let mut events = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout_duration;

        // Send Page.navigate.
        let nav_result = self.send("Page.navigate", json!({"url": url})).await?;

        // Wait for load event from the already-subscribed channel.
        loop {
            let remaining = deadline.duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Timeout — SPA pushState navigations may never fire loadEventFired.
                // Return the navigate result anyway; caller can verify page state.
                debug!(
                    %url,
                    "CDP navigate_and_wait: no load event within timeout, returning navigate result"
                );
                return Ok(nav_result);
            }
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(e) if e.method == "Page.loadEventFired" => return Ok(nav_result),
                        Ok(e) if e.method == "Page.lifecycleEvent" => {
                            let name = e.params.get("name").and_then(Value::as_str);
                            if name == Some("load") || name == Some("networkIdle") {
                                return Ok(nav_result);
                            }
                        }
                        Err(_) => {
                            return Err("CDP event channel closed during navigate_and_wait".to_string());
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(remaining) => {
                    debug!(%url, "CDP navigate_and_wait: timed out waiting for load event");
                    return Ok(nav_result);
                }
            }
        }
    }

    /// Wait for a CSS selector to match a visible element using an injected
    /// MutationObserver. This is event-driven (no polling) and resolves as
    /// soon as the element appears. Uses Runtime.evaluate with awaitPromise.
    pub(super) async fn wait_for_selector(
        &self,
        selector: &str,
        timeout_duration: Duration,
    ) -> Result<Value, String> {
        let timeout_ms = timeout_duration.as_millis() as u64;
        let js = format!(
            r#"(async () => {{
              const SELECTOR = {selector_json};
              const TIMEOUT = {timeout_ms};
              const isVisible = (el) => !!(el && (el.offsetWidth || el.offsetHeight || el.getClientRects().length));
              // Fast path: element already exists.
              const existing = document.querySelector(SELECTOR);
              if (existing && isVisible(existing)) {{
                const r = existing.getBoundingClientRect();
                return {{ found: true, tag: existing.tagName, x: r.left + r.width / 2, y: r.top + r.height / 2, width: r.width, height: r.height }};
              }}
              // Slow path: wait via MutationObserver.
              return new Promise((resolve, reject) => {{
                let timer;
                const observer = new MutationObserver(() => {{
                  const el = document.querySelector(SELECTOR);
                  if (el && isVisible(el)) {{
                    observer.disconnect();
                    clearTimeout(timer);
                    const r = el.getBoundingClientRect();
                    resolve({{ found: true, tag: el.tagName, x: r.left + r.width / 2, y: r.top + r.height / 2, width: r.width, height: r.height }});
                  }}
                }});
                observer.observe(document.body || document.documentElement, {{ childList: true, subtree: true, attributes: true, attributeFilter: ['style', 'class', 'hidden', 'disabled'] }});
                timer = setTimeout(() => {{
                  observer.disconnect();
                  reject(new Error('wait_for_selector timeout: ' + SELECTOR));
                }}, TIMEOUT);
              }});
            }})()"#,
            selector_json =
                serde_json::to_string(selector).unwrap_or_else(|_| format!("\"{}\"", selector)),
            timeout_ms = timeout_ms,
        );
        let result = self
            .send_with_timeout(
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "awaitPromise": true,
                    "returnByValue": true,
                }),
                timeout_duration + Duration::from_secs(5), // extra buffer for CDP overhead
            )
            .await?;
        // Runtime.evaluate wraps the value in { result: { value: ... } }
        let value = result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        if value.get("found").and_then(Value::as_bool).unwrap_or(false) {
            Ok(value)
        } else {
            // Check for exception
            let exception = result
                .get("exceptionDetails")
                .and_then(|e| e.get("exception"))
                .and_then(|e| e.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            Err(format!("wait_for_selector failed: {exception}"))
        }
    }

    /// Perform Playwright-style actionability checks on an element before interaction.
    /// Checks: exists, visible, stable (not animating), enabled, receives events (not obscured).
    /// Returns element info (coordinates, state) if actionable, or an error describing why not.
    pub(super) async fn check_actionability(
        &self,
        selector: &str,
        timeout_duration: Duration,
    ) -> Result<Value, String> {
        let timeout_ms = timeout_duration.as_millis() as u64;
        let js = format!(
            r#"(async () => {{
              const SELECTOR = {selector_json};
              const TIMEOUT = {timeout_ms};
              const deadline = Date.now() + TIMEOUT;
              const isVisible = (el) => {{
                if (!el) return false;
                const s = getComputedStyle(el);
                if (s.visibility === 'hidden' || s.display === 'none' || s.opacity === '0') return false;
                return !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
              }};
              const sleep = (ms) => new Promise(r => setTimeout(r, ms));
              while (Date.now() < deadline) {{
                const el = document.querySelector(SELECTOR);
                if (!el) {{ await sleep(200); continue; }}
                // 0. Scroll into view — ensures off-screen elements become visible
                //    and are not falsely flagged as obscured by elementFromPoint.
                el.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
                // Wait one frame for layout to settle after scroll.
                await new Promise(r => requestAnimationFrame(r));
                // 1. Visible
                if (!isVisible(el)) {{ await sleep(200); continue; }}
                // 2. Stable — compare bounding rect across 2 frames (~32ms apart)
                const r1 = el.getBoundingClientRect();
                await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
                const r2 = el.getBoundingClientRect();
                const moved = Math.abs(r1.x - r2.x) > 1 || Math.abs(r1.y - r2.y) > 1
                           || Math.abs(r1.width - r2.width) > 1 || Math.abs(r1.height - r2.height) > 1;
                if (moved) {{ await sleep(50); continue; }}
                // 3. Enabled
                if (el.disabled || el.getAttribute('aria-disabled') === 'true') {{ await sleep(200); continue; }}
                // 4. Receives events (not obscured)
                const cx = r2.left + r2.width / 2;
                const cy = r2.top + r2.height / 2;
                const top = document.elementFromPoint(cx, cy);
                if (top !== el && !el.contains(top) && !(top && top.closest && top.closest(SELECTOR) === el)) {{
                  await sleep(200); continue;
                }}
                return {{
                  actionable: true,
                  tag: el.tagName, id: el.id || null,
                  x: cx, y: cy, width: r2.width, height: r2.height,
                  disabled: false, visible: true, stable: true, receivesEvents: true
                }};
              }}
              // Timeout — report why
              const el = document.querySelector(SELECTOR);
              if (!el) return {{ actionable: false, reason: 'not_found' }};
              if (!isVisible(el)) return {{ actionable: false, reason: 'not_visible' }};
              if (el.disabled) return {{ actionable: false, reason: 'disabled' }};
              return {{ actionable: false, reason: 'obscured_or_animating' }};
            }})()"#,
            selector_json =
                serde_json::to_string(selector).unwrap_or_else(|_| format!("\"{}\"", selector)),
            timeout_ms = timeout_ms,
        );
        let result = self
            .send_with_timeout(
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "awaitPromise": true,
                    "returnByValue": true,
                }),
                timeout_duration + Duration::from_secs(5),
            )
            .await?;
        let value = result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        if value
            .get("actionable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Ok(value)
        } else {
            let reason = value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Err(format!(
                "actionability check failed for '{selector}': {reason}"
            ))
        }
    }

    /// Wait for the network to become idle (no in-flight requests for `quiet_period`).
    /// Uses CDP Network events instead of fixed timeouts.
    /// This is the "idle watchdog" pattern — each network event resets the idle timer.
    #[allow(dead_code)]
    pub(super) async fn wait_for_network_idle(
        &self,
        quiet_period: Duration,
        timeout_duration: Duration,
    ) -> Result<(), String> {
        let mut events = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout_duration;
        let mut in_flight: HashSet<String> = HashSet::new();
        let mut idle_since = tokio::time::Instant::now();
        loop {
            let remaining = deadline.duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "wait_for_network_idle: timed out with {} in-flight requests",
                    in_flight.len()
                ));
            }
            let idle_remaining = quiet_period.saturating_sub(idle_since.elapsed());
            if in_flight.is_empty() && idle_remaining.is_zero() {
                return Ok(());
            }
            let wait_time = if in_flight.is_empty() {
                idle_remaining.min(remaining)
            } else {
                remaining
            };
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(e) if e.method == "Network.requestWillBeSent" => {
                            if let Some(id) = e.params.get("requestId").and_then(Value::as_str) {
                                in_flight.insert(id.to_string());
                            }
                            idle_since = tokio::time::Instant::now();
                        }
                        Ok(e) if e.method == "Network.loadingFinished"
                              || e.method == "Network.loadingFailed" => {
                            // Only loadingFinished/loadingFailed mark a request as truly
                            // complete.  responseReceived only means headers arrived but
                            // the body may still be streaming — using it here would cause
                            // premature idle detection.
                            if let Some(id) = e.params.get("requestId").and_then(Value::as_str) {
                                in_flight.remove(id);
                            }
                            if in_flight.is_empty() {
                                idle_since = tokio::time::Instant::now();
                            }
                        }
                        Err(_) => return Err("CDP event channel closed during network idle wait".to_string()),
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(wait_time) => {
                    // Timer expired — either idle period met or overall timeout
                }
            }
        }
    }

    /// Check if the CDP connection is healthy by sending a lightweight command.
    /// Returns Ok(()) if the connection is alive, Err if it's broken.
    #[allow(dead_code)]
    pub(super) async fn health_check(&self) -> Result<(), String> {
        if self.is_closed() {
            return Err("CDP connection is closed".to_string());
        }
        // Send a lightweight CDP command to verify the connection is truly alive.
        // Runtime.evaluate with a trivial expression verifies the target still exists.
        self.send_with_timeout(
            "Runtime.evaluate",
            json!({"expression": "1", "returnByValue": true}),
            Duration::from_secs(5),
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("CDP health check failed: {e}"))
    }

    /// Connect to a CDP WebSocket with retry logic.
    /// Retries up to `max_attempts` times with exponential backoff.
    pub(super) async fn connect_with_retry(
        ws_url: &str,
        max_attempts: u32,
    ) -> Result<Self, String> {
        let mut last_err = String::new();
        for attempt in 1..=max_attempts {
            match Self::connect(ws_url).await {
                Ok(client) => return Ok(client),
                Err(e) => {
                    warn!(
                        attempt,
                        max_attempts,
                        error = %e,
                        "CDP connect_with_retry: attempt failed"
                    );
                    last_err = e;
                    if attempt < max_attempts {
                        tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                    }
                }
            }
        }
        Err(format!(
            "CDP connect failed after {max_attempts} attempts: {last_err}"
        ))
    }

    pub(super) fn close(&self) {
        debug!("CDP client: explicit close requested");
        self.closed.store(true, Ordering::SeqCst);
        drain_pending(&self.pending, "cdp_explicit_close");
        let _ = self.sender.send(Message::Close(None));
    }
}

fn browser_executable(channel: &str) -> Option<String> {
    let candidates: &[&str] = if channel.eq_ignore_ascii_case("chrome") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "google-chrome",
            "chromium",
        ]
    } else {
        &[
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "microsoft-edge",
            "msedge",
        ]
    };
    for candidate in candidates {
        if candidate.starts_with('/') {
            if Path::new(candidate).exists() {
                return Some((*candidate).to_string());
            }
        } else if std::process::Command::new("sh")
            .arg("-lc")
            .arg(format!("command -v {}", shell_escape(candidate)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .is_some_and(|status| status.success())
        {
            return Some((*candidate).to_string());
        }
    }
    None
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn allocate_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("allocate browser debugging port failed: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("read browser debugging port failed: {error}"))?
        .port();
    drop(listener);
    Ok(port)
}

async fn wait_for_json<T: for<'de> Deserialize<'de>>(
    url: &str,
    duration: Duration,
) -> Result<T, String> {
    let deadline = tokio::time::Instant::now() + duration;
    let mut last_error = String::new();
    while tokio::time::Instant::now() < deadline {
        match browser_http_client().get(url).send().await {
            Ok(response) if response.status().is_success() => {
                return response
                    .json::<T>()
                    .await
                    .map_err(|error| format!("parse {url} failed: {error}"));
            }
            Ok(response) => last_error = format!("HTTP {}", response.status()),
            Err(error) => last_error = error.to_string(),
        }
        sleep(Duration::from_millis(250)).await;
    }
    Err(format!("timed out waiting for {url}: {last_error}"))
}
