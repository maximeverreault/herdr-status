//! One task per server: keeps a subscription open, reconciles with snapshots,
//! rebuilds when the pane set changes, backs off on failure.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use herdr_client::transport::TransportError;
use herdr_client::{Client, ClientError, Event, Subscription};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::model::ServerState;
use crate::reducer::{apply_snapshot, apply_status, set_connecting, set_offline, set_online};
use crate::transport_source::TransportSource;

/// Parameterless subscriptions requested on every connection.
pub const LIFECYCLE_SUBSCRIPTIONS: [&str; 13] = [
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.exited",
    "pane.agent_detected",
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "workspace.updated",
    "workspace.metadata_updated",
    "tab.created",
    "tab.closed",
    "tab.renamed",
];

pub fn subscriptions_for(pane_ids: &[String]) -> Vec<Subscription> {
    LIFECYCLE_SUBSCRIPTIONS
        .iter()
        .map(|t| Subscription::lifecycle(t))
        .chain(pane_ids.iter().map(|p| Subscription::pane_agent_status(p)))
        .collect()
}

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub reconcile: Duration,
    pub debounce: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Retry interval when the server is simply not running (local socket missing).
    pub not_running_retry: Duration,
    /// Once online this long, the next failure starts again at `backoff_min`.
    pub stable_after: Duration,
    /// Upper bound on one snapshot or subscribe round trip.
    pub request_timeout: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            reconcile: Duration::from_secs(30),
            debounce: Duration::from_millis(250),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            not_running_retry: Duration::from_secs(5),
            stable_after: Duration::from_secs(60),
            request_timeout: Duration::from_secs(20),
        }
    }
}

impl WatcherConfig {
    pub fn with_reconcile_seconds(mut self, seconds: u64) -> Self {
        self.reconcile = Duration::from_secs(seconds.max(5));
        self
    }
}

#[derive(Debug)]
pub enum WatcherCommand {
    Reconnect,
    Shutdown,
}

pub struct WatcherHandle {
    commands: mpsc::Sender<WatcherCommand>,
    task: JoinHandle<()>,
}

impl WatcherHandle {
    /// Drop the current connection and reconnect now, resetting backoff.
    pub fn reconnect(&self) {
        let _ = self.commands.try_send(WatcherCommand::Reconnect);
    }

    pub async fn shutdown(self) {
        let _ = self.commands.send(WatcherCommand::Shutdown).await;
        let _ = self.task.await;
    }
}

pub fn spawn_watcher(
    state: ServerState,
    source: Arc<dyn TransportSource>,
    cfg: WatcherConfig,
    updates: mpsc::Sender<ServerState>,
) -> WatcherHandle {
    let (commands, rx) = mpsc::channel(8);
    let backoff = cfg.backoff_min;
    let watcher = Watcher {
        state,
        source,
        cfg,
        updates,
        commands: rx,
        backoff,
        rebuild_streak: 0,
    };
    let task = tokio::spawn(watcher.run());
    WatcherHandle { commands, task }
}

/// User-facing text for a failure (spec section 14).
pub fn reason_text(error: &ClientError) -> String {
    match error {
        ClientError::Transport(TransportError::NotRunning(_)) => "herdr is not running".to_string(),
        ClientError::Transport(TransportError::Launch(msg)) => msg.clone(),
        ClientError::Transport(TransportError::Timeout(msg)) => msg.clone(),
        ClientError::Transport(TransportError::Io(e)) => format!("connection error: {e}"),
        ClientError::Api(body) => format!("herdr error {}: {}", body.code, body.message),
        ClientError::Closed(msg) => format!("connection closed {msg}"),
        ClientError::Protocol(msg) => msg.clone(),
    }
}

struct Watcher {
    state: ServerState,
    source: Arc<dyn TransportSource>,
    cfg: WatcherConfig,
    updates: mpsc::Sender<ServerState>,
    commands: mpsc::Receiver<WatcherCommand>,
    backoff: Duration,
    /// Consecutive rebuilds without a stable session in between.
    rebuild_streak: u32,
}

/// Rapid consecutive rebuilds allowed before the session counts as failed.
const MAX_REBUILD_STREAK: u32 = 5;

/// Why a session ended.
enum SessionEnd {
    /// Pane set changed or events were lost: resubscribe without touching the chip.
    /// `counted` is true for involuntary rebuilds (EOF, events lost, subscribe
    /// rejected), which count toward the rebuild cap; pane-driven rebuilds do not.
    Rebuild {
        counted: bool,
    },
    /// User asked for a reconnect.
    Reconnect,
    Failed {
        reason: String,
        not_running: bool,
    },
}

/// The receiver is gone or Shutdown was requested.
struct Stop;

impl Watcher {
    async fn run(mut self) {
        let mut planned_rebuild = false;
        loop {
            if !planned_rebuild {
                set_connecting(&mut self.state);
                if self.publish().await.is_err() {
                    return;
                }
            }
            planned_rebuild = false;
            match self.run_session().await {
                Err(Stop) => return,
                Ok(SessionEnd::Rebuild { counted }) => {
                    if counted {
                        self.rebuild_streak += 1;
                    }
                    planned_rebuild = true;
                }
                Ok(SessionEnd::Reconnect) => {
                    self.rebuild_streak = 0;
                    self.backoff = self.cfg.backoff_min;
                }
                Ok(SessionEnd::Failed {
                    reason,
                    not_running,
                }) => {
                    self.rebuild_streak = 0;
                    let wait = if not_running {
                        self.cfg.not_running_retry
                    } else {
                        self.backoff
                    };
                    tracing::info!(server = %self.state.id, %reason, ?wait, "server offline");
                    set_offline(&mut self.state, reason, SystemTime::now() + wait);
                    if self.publish().await.is_err() {
                        return;
                    }
                    if !not_running {
                        self.backoff = (self.backoff * 2).min(self.cfg.backoff_max);
                        self.source.invalidate();
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        cmd = self.commands.recv() => match cmd {
                            Some(WatcherCommand::Reconnect) => self.backoff = self.cfg.backoff_min,
                            Some(WatcherCommand::Shutdown) | None => return,
                        },
                    }
                }
            }
        }
    }

    async fn publish(&self) -> Result<(), Stop> {
        self.updates
            .send(self.state.clone())
            .await
            .map_err(|_| Stop)
    }

    fn failed(&self, error: ClientError) -> SessionEnd {
        let not_running = matches!(error, ClientError::Transport(TransportError::NotRunning(_)));
        SessionEnd::Failed {
            reason: reason_text(&error),
            not_running,
        }
    }

    /// A rebuild, unless rebuilds keep happening in quick succession. A session
    /// that was online for at least `backoff_min` clears the streak first.
    fn rebuild(&mut self, online_since: Option<Instant>) -> SessionEnd {
        if online_since.is_some_and(|t| t.elapsed() >= self.cfg.backoff_min) {
            self.rebuild_streak = 0;
        }
        if self.rebuild_streak >= MAX_REBUILD_STREAK {
            SessionEnd::Failed {
                reason: "subscription keeps closing".into(),
                not_running: false,
            }
        } else {
            SessionEnd::Rebuild { counted: true }
        }
    }

    async fn run_session(&mut self) -> Result<SessionEnd, Stop> {
        let transport = match self.source.transport().await {
            Ok(t) => t,
            Err(reason) => {
                return Ok(SessionEnd::Failed {
                    reason,
                    not_running: false,
                })
            }
        };
        let client = Client::new(transport.clone());

        let timeout = self.cfg.request_timeout;

        let first = match tokio::time::timeout(timeout, client.snapshot()).await {
            Err(_) => return Ok(timed_out()),
            Ok(Err(e)) => return Ok(self.failed(e)),
            Ok(Ok(s)) => s,
        };
        let pane_ids: Vec<String> = first.pane_ids().iter().map(|p| p.to_string()).collect();
        let mut stream =
            match tokio::time::timeout(timeout, client.subscribe(subscriptions_for(&pane_ids)))
                .await
            {
                Err(_) => return Ok(timed_out()),
                // A pane vanished between the snapshot and the subscribe, so herdr
                // rejected or closed it: take a fresh snapshot and try again.
                Ok(Err(ClientError::Closed(_) | ClientError::Api(_))) => {
                    return Ok(self.rebuild(None))
                }
                Ok(Err(e)) => return Ok(self.failed(e)),
                Ok(Ok(s)) => s,
            };
        let subscribed: HashSet<String> = pane_ids.into_iter().collect();
        let authoritative = match tokio::time::timeout(timeout, client.snapshot()).await {
            Err(_) => return Ok(timed_out()),
            Ok(Err(e)) => return Ok(self.failed(e)),
            Ok(Ok(s)) => s,
        };
        apply_snapshot(&mut self.state, &authoritative, SystemTime::now());
        set_online(&mut self.state, &transport.describe());
        self.publish().await?;
        let online_since = Instant::now();

        let mut reconcile = tokio::time::interval(self.cfg.reconcile);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        reconcile.tick().await; // the immediate first tick
        let mut dirty: Option<(Instant, bool)> = None; // (deadline, rebuild)

        loop {
            let dirty_deadline = dirty.map(|(d, _)| d);
            tokio::select! {
                event = stream.next() => match event {
                    Ok(Some(Event::PaneAgentStatusChanged(e))) => {
                        if apply_status(&mut self.state, &e, SystemTime::now()) {
                            self.publish().await?;
                        } else {
                            dirty = Some((Instant::now() + self.cfg.debounce, dirty.is_some_and(|(_, r)| r)));
                        }
                    }
                    Ok(Some(Event::PaneCreated { .. } | Event::PaneClosed { .. } | Event::PaneExited { .. } | Event::PaneAgentDetected { .. })) => {
                        dirty = Some((Instant::now() + self.cfg.debounce, true));
                    }
                    Ok(Some(Event::PaneUpdated { .. } | Event::Other)) => {
                        if dirty.is_none() {
                            dirty = Some((Instant::now() + self.cfg.debounce, false));
                        }
                    }
                    Ok(None) => {
                        tracing::info!(server = %self.state.id, "subscription closed; rebuilding");
                        return Ok(self.rebuild(Some(online_since)));
                    }
                    Err(e) => {
                        if e.api_code() == Some("events_lost") {
                            tracing::info!(server = %self.state.id, "events lost; rebuilding subscription");
                            return Ok(self.rebuild(Some(online_since)));
                        }
                        self.reset_backoff_if_stable(online_since);
                        return Ok(self.failed(e));
                    }
                },
                _ = sleep_until_opt(dirty_deadline) => {
                    let (_, rebuild) = dirty.take().expect("dirty is set when its sleep fires");
                    if rebuild {
                        return Ok(SessionEnd::Rebuild { counted: false });
                    }
                    if let Some(end) = self.resnapshot(&client, online_since, &subscribed).await? {
                        return Ok(end);
                    }
                }
                _ = reconcile.tick() => {
                    if let Some(end) = self.resnapshot(&client, online_since, &subscribed).await? {
                        return Ok(end);
                    }
                }
                cmd = self.commands.recv() => match cmd {
                    Some(WatcherCommand::Reconnect) => return Ok(SessionEnd::Reconnect),
                    Some(WatcherCommand::Shutdown) | None => return Err(Stop),
                },
            }
        }
    }

    /// Take a snapshot and publish. `Ok(Some(end))` when the server failed, or
    /// when the pane set no longer matches `subscribed` (a missed `pane.created`
    /// or `pane.closed`), which needs a rebuilt subscription.
    async fn resnapshot(
        &mut self,
        client: &Client,
        online_since: Instant,
        subscribed: &HashSet<String>,
    ) -> Result<Option<SessionEnd>, Stop> {
        match tokio::time::timeout(self.cfg.request_timeout, client.snapshot()).await {
            Ok(Ok(snapshot)) => {
                apply_snapshot(&mut self.state, &snapshot, SystemTime::now());
                self.publish().await?;
                let current: HashSet<String> = snapshot
                    .pane_ids()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                if current != *subscribed {
                    tracing::info!(server = %self.state.id, "pane set changed since subscribing; rebuilding");
                    return Ok(Some(SessionEnd::Rebuild { counted: false }));
                }
                Ok(None)
            }
            Ok(Err(e)) => {
                self.reset_backoff_if_stable(online_since);
                Ok(Some(self.failed(e)))
            }
            Err(_) => {
                self.reset_backoff_if_stable(online_since);
                Ok(Some(timed_out()))
            }
        }
    }

    fn reset_backoff_if_stable(&mut self, online_since: Instant) {
        if online_since.elapsed() >= self.cfg.stable_after {
            self.backoff = self.cfg.backoff_min;
        }
    }
}

fn timed_out() -> SessionEnd {
    SessionEnd::Failed {
        reason: "herdr did not answer in time".into(),
        not_running: false,
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}
