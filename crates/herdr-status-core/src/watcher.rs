//! One task per server: keeps a subscription open, reconciles with snapshots,
//! rebuilds when the pane set changes, backs off on failure (spec section 8).
//!
//! ```text
//! Connecting --connect(): transport, S1, subscribe, S2--> Online
//! Connecting --connect() fails--------------------------> Offline(reason, retry_at)
//! Online     --Rebuild (pane set changed, EOF, ...)-----> connect() again, chip stays Online
//! Online     --failure----------------------------------> Offline(reason, retry_at)
//! Online     --Reconnect command------------------------> Connecting
//! Offline    --backoff wait or Reconnect command--------> Connecting
//! any state  --shutdown---------------------------------> task ends
//! ```
//!
//! A rebuild is counted (towards `MAX_REBUILD_STREAK`) only when it was
//! involuntary: EOF, `events_lost`, or a rejected subscribe. Rebuilds caused by
//! a changed pane set are free. `RetryPolicy` holds the backoff and the streak.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use herdr_client::transport::TransportError;
use herdr_client::{Client, ClientError, Event, EventStream, SessionSnapshot, Subscription};
use tokio::sync::{mpsc, watch};
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
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// How long `shutdown` waits for the task before aborting it.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

impl WatcherHandle {
    /// Drop the current connection and reconnect now, resetting backoff.
    pub fn reconnect(&self) {
        let _ = self.commands.try_send(WatcherCommand::Reconnect);
    }

    /// Stop the watcher promptly. The cancel signal interrupts whatever the task
    /// is awaiting (first contact, a hanging request, a full update channel);
    /// if it has still not ended after 500 ms it is aborted. Aborting is safe:
    /// the watcher has no state to flush, and bridge child processes are spawned
    /// with `kill_on_drop`, so dropping the task's futures ends them.
    pub async fn shutdown(mut self) {
        let _ = self.cancel.send(true);
        let _ = self.commands.try_send(WatcherCommand::Shutdown);
        if tokio::time::timeout(SHUTDOWN_GRACE, &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

pub fn spawn_watcher(
    state: ServerState,
    source: Arc<dyn TransportSource>,
    cfg: WatcherConfig,
    updates: mpsc::Sender<ServerState>,
) -> WatcherHandle {
    let (commands, rx) = mpsc::channel(8);
    let (cancel, cancelled) = watch::channel(false);
    let watcher = Watcher {
        state,
        source,
        policy: RetryPolicy::new(&cfg),
        cfg,
        updates,
        commands: rx,
        cancelled,
        last_offline_reason: None,
    };
    let task = tokio::spawn(watcher.run());
    WatcherHandle {
        commands,
        cancel,
        task,
    }
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

/// Counted rebuilds allowed in quick succession; the next one is a failure.
const MAX_REBUILD_STREAK: u32 = 5;

/// Why a session ended.
enum SessionEnd {
    /// Resubscribe without touching the chip. `cause` is set for involuntary
    /// rebuilds (EOF, events lost, subscribe rejected or closed), which count
    /// toward the rebuild cap; pane-set changes leave it `None`.
    Rebuild { cause: Option<ClientError> },
    /// User asked for a reconnect.
    Reconnect,
    Failed {
        reason: String,
        not_running: bool,
        /// Cached discovery (WSL or SSH herdr path) is probably wrong: the
        /// source could not produce a transport, or the bridge failed to launch.
        stale_discovery: bool,
    },
}

/// Backoff and rebuild-streak bookkeeping, driven only by how sessions end and
/// how long they were online. Pure, so it is tested without real time.
#[derive(Debug, Clone)]
struct RetryPolicy {
    backoff_min: Duration,
    backoff_max: Duration,
    stable_after: Duration,
    not_running_retry: Duration,
    /// Wait for the next ordinary (not "not running") failure.
    backoff: Duration,
    /// Wait chosen for the most recent ordinary failure.
    current_wait: Duration,
    /// Counted rebuilds without a failure, reconnect or `backoff_min` online in between.
    rebuild_streak: u32,
}

impl RetryPolicy {
    fn new(cfg: &WatcherConfig) -> Self {
        Self {
            backoff_min: cfg.backoff_min,
            backoff_max: cfg.backoff_max,
            stable_after: cfg.stable_after,
            not_running_retry: cfg.not_running_retry,
            backoff: cfg.backoff_min,
            current_wait: cfg.backoff_min,
            rebuild_streak: 0,
        }
    }

    /// Record how a session ended. `online_for` is `None` when it never got online.
    fn on_session_end(&mut self, online_for: Option<Duration>, end: &SessionEnd) {
        if let Some(online_for) = online_for {
            if online_for >= self.backoff_min {
                self.rebuild_streak = 0;
            }
            if online_for >= self.stable_after {
                self.backoff = self.backoff_min;
            }
        }
        match end {
            SessionEnd::Rebuild { cause: Some(_) } => self.rebuild_streak += 1,
            SessionEnd::Rebuild { cause: None } => {}
            SessionEnd::Reconnect => {
                self.rebuild_streak = 0;
                self.backoff = self.backoff_min;
            }
            SessionEnd::Failed { not_running, .. } => {
                self.rebuild_streak = 0;
                if !*not_running {
                    self.current_wait = self.backoff;
                    self.backoff = (self.backoff * 2).min(self.backoff_max);
                }
            }
        }
    }

    /// Too many counted rebuilds in quick succession.
    fn cap_tripped(&self) -> bool {
        self.rebuild_streak > MAX_REBUILD_STREAK
    }

    /// How long to wait after the failure just recorded.
    fn wait_for(&self, not_running: bool) -> Duration {
        if not_running {
            self.not_running_retry
        } else {
            self.current_wait
        }
    }
}

struct Watcher {
    state: ServerState,
    source: Arc<dyn TransportSource>,
    cfg: WatcherConfig,
    policy: RetryPolicy,
    updates: mpsc::Sender<ServerState>,
    commands: mpsc::Receiver<WatcherCommand>,
    cancelled: watch::Receiver<bool>,
    /// Last offline reason logged at info level, so repeats log at debug.
    last_offline_reason: Option<String>,
}

/// The receiver is gone or Shutdown was requested.
struct Stop;

/// An online connection: subscribed and with S2 applied.
struct Session {
    client: Client,
    stream: EventStream,
    subscribed: HashSet<String>,
    online_since: Instant,
    /// S2's pane set differs from the subscribed one.
    stale_subscription: bool,
}

/// Pending debounced work after events.
struct Dirty {
    armed_at: Instant,
    deadline: Instant,
    rebuild: bool,
}

/// Arm or extend the debounce; never later than `4 × debounce` after first armed.
fn arm(dirty: &mut Option<Dirty>, rebuild: bool, debounce: Duration) {
    let now = Instant::now();
    let armed_at = dirty.as_ref().map_or(now, |d| d.armed_at);
    *dirty = Some(Dirty {
        armed_at,
        deadline: (now + debounce).min(armed_at + debounce * 4),
        rebuild: rebuild || dirty.as_ref().is_some_and(|d| d.rebuild),
    });
}

impl Watcher {
    async fn run(mut self) {
        let mut cancelled = self.cancelled.clone();
        tokio::select! {
            biased;
            // `Err` means the handle was dropped: stop as well.
            _ = cancelled.wait_for(|c| *c) => {}
            _ = self.run_loop() => {}
        }
    }

    async fn run_loop(&mut self) -> Result<(), Stop> {
        let mut planned_rebuild = false;
        loop {
            if !planned_rebuild {
                set_connecting(&mut self.state);
                self.publish().await?;
            }
            planned_rebuild = false;
            let (end, online_for) = self.run_session().await?;
            self.policy.on_session_end(online_for, &end);
            let end = match end {
                SessionEnd::Rebuild { cause: Some(cause) } if self.policy.cap_tripped() => {
                    let failed = SessionEnd::Failed {
                        reason: reason_text(&cause),
                        not_running: false,
                        stale_discovery: false,
                    };
                    self.policy.on_session_end(None, &failed);
                    failed
                }
                other => other,
            };
            match end {
                SessionEnd::Rebuild { .. } => planned_rebuild = true,
                SessionEnd::Reconnect => {}
                SessionEnd::Failed {
                    reason,
                    not_running,
                    stale_discovery,
                } => {
                    let wait = self.policy.wait_for(not_running);
                    if self.last_offline_reason.as_deref() == Some(reason.as_str()) {
                        tracing::debug!(server = %self.state.id, %reason, ?wait, "server offline");
                    } else {
                        tracing::info!(server = %self.state.id, %reason, ?wait, "server offline");
                        self.last_offline_reason = Some(reason.clone());
                    }
                    set_offline(&mut self.state, reason, SystemTime::now() + wait);
                    self.publish().await?;
                    if stale_discovery {
                        self.source.invalidate();
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        cmd = self.commands.recv() => match cmd {
                            Some(WatcherCommand::Reconnect) => {
                                self.policy.on_session_end(None, &SessionEnd::Reconnect);
                            }
                            Some(WatcherCommand::Shutdown) | None => return Err(Stop),
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

    /// One connection attempt and, if it gets online, its event loop. Also
    /// returns how long the session was online (`None` if it never was).
    async fn run_session(&mut self) -> Result<(SessionEnd, Option<Duration>), Stop> {
        let session = match self.connect().await? {
            Ok(session) => session,
            Err(end) => return Ok((end, None)),
        };
        let online_since = session.online_since;
        let end = self.event_loop(session).await?;
        Ok((end, Some(online_since.elapsed())))
    }

    /// Transport, S1, subscribe, S2, then publish Online.
    async fn connect(&mut self) -> Result<Result<Session, SessionEnd>, Stop> {
        let transport = match self.source.transport().await {
            Ok(t) => t,
            Err(reason) => {
                return Ok(Err(SessionEnd::Failed {
                    reason,
                    not_running: false,
                    stale_discovery: true,
                }))
            }
        };
        let client = Client::new(transport.clone());

        let first = match self.timed(client.snapshot()).await {
            Ok(s) => s,
            Err(end) => return Ok(Err(end)),
        };
        let pane_ids: Vec<String> = first.pane_ids().iter().map(|p| p.to_string()).collect();
        let subscribe = client.subscribe(subscriptions_for(&pane_ids));
        let stream = match tokio::time::timeout(self.cfg.request_timeout, subscribe).await {
            Err(_) => return Ok(Err(timed_out())),
            // A pane vanished between the snapshot and the subscribe, so herdr
            // rejected or closed it: take a fresh snapshot and try again.
            Ok(Err(e @ (ClientError::Closed(_) | ClientError::Api(_)))) => {
                return Ok(Err(SessionEnd::Rebuild { cause: Some(e) }))
            }
            Ok(Err(e)) => return Ok(Err(failed(e))),
            Ok(Ok(s)) => s,
        };
        let subscribed: HashSet<String> = pane_ids.into_iter().collect();
        let authoritative = match self.timed(client.snapshot()).await {
            Ok(s) => s,
            Err(end) => return Ok(Err(end)),
        };
        apply_snapshot(&mut self.state, &authoritative, SystemTime::now());
        set_online(&mut self.state, &transport.describe());
        self.last_offline_reason = None;
        self.publish().await?;
        Ok(Ok(Session {
            client,
            stream,
            stale_subscription: pane_set(&authoritative) != subscribed,
            subscribed,
            online_since: Instant::now(),
        }))
    }

    async fn event_loop(&mut self, session: Session) -> Result<SessionEnd, Stop> {
        let Session {
            client,
            mut stream,
            subscribed,
            stale_subscription,
            ..
        } = session;
        let debounce = self.cfg.debounce;
        let period = self.cfg.reconcile;
        let mut reconcile = tokio::time::interval_at(Instant::now() + period, period);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut dirty: Option<Dirty> = None;
        if stale_subscription {
            arm(&mut dirty, true, debounce);
        }

        loop {
            let dirty_deadline = dirty.as_ref().map(|d| d.deadline);
            tokio::select! {
                event = stream.next() => match event {
                    Ok(Some(Event::PaneAgentStatusChanged(e))) => {
                        if apply_status(&mut self.state, &e, SystemTime::now()) {
                            self.publish().await?;
                        } else {
                            arm(&mut dirty, false, debounce);
                        }
                    }
                    Ok(Some(Event::PaneCreated { .. } | Event::PaneClosed { .. } | Event::PaneExited { .. } | Event::PaneAgentDetected { .. })) => {
                        arm(&mut dirty, true, debounce);
                    }
                    Ok(Some(Event::PaneUpdated { .. } | Event::Other)) => {
                        if dirty.is_none() {
                            arm(&mut dirty, false, debounce);
                        }
                    }
                    Ok(None) => {
                        tracing::info!(server = %self.state.id, "subscription closed; rebuilding");
                        let cause = ClientError::Closed("while streaming events".into());
                        return Ok(SessionEnd::Rebuild { cause: Some(cause) });
                    }
                    Err(e) if e.api_code() == Some("events_lost") => {
                        tracing::info!(server = %self.state.id, "events lost; rebuilding subscription");
                        return Ok(SessionEnd::Rebuild { cause: Some(e) });
                    }
                    Err(e) => return Ok(failed(e)),
                },
                _ = sleep_until_opt(dirty_deadline) => {
                    if let Some(fired) = dirty.take() {
                        if fired.rebuild {
                            return Ok(SessionEnd::Rebuild { cause: None });
                        }
                        if let Some(end) = self.resnapshot(&client, &subscribed).await? {
                            return Ok(end);
                        }
                        reconcile.reset();
                    }
                }
                _ = reconcile.tick() => {
                    if let Some(end) = self.resnapshot(&client, &subscribed).await? {
                        return Ok(end);
                    }
                    reconcile.reset();
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
        subscribed: &HashSet<String>,
    ) -> Result<Option<SessionEnd>, Stop> {
        let snapshot = match self.timed(client.snapshot()).await {
            Ok(s) => s,
            Err(end) => return Ok(Some(end)),
        };
        apply_snapshot(&mut self.state, &snapshot, SystemTime::now());
        self.publish().await?;
        if pane_set(&snapshot) != *subscribed {
            tracing::info!(server = %self.state.id, "pane set changed since subscribing; rebuilding");
            return Ok(Some(SessionEnd::Rebuild { cause: None }));
        }
        Ok(None)
    }

    /// Run one request under `request_timeout`.
    async fn timed<T>(
        &self,
        request: impl Future<Output = Result<T, ClientError>>,
    ) -> Result<T, SessionEnd> {
        match tokio::time::timeout(self.cfg.request_timeout, request).await {
            Err(_) => Err(timed_out()),
            Ok(Err(e)) => Err(failed(e)),
            Ok(Ok(v)) => Ok(v),
        }
    }
}

fn failed(error: ClientError) -> SessionEnd {
    SessionEnd::Failed {
        not_running: matches!(error, ClientError::Transport(TransportError::NotRunning(_))),
        stale_discovery: matches!(error, ClientError::Transport(TransportError::Launch(_))),
        reason: reason_text(&error),
    }
}

fn timed_out() -> SessionEnd {
    SessionEnd::Failed {
        reason: "herdr did not answer in time".into(),
        not_running: false,
        stale_discovery: false,
    }
}

fn pane_set(snapshot: &SessionSnapshot) -> HashSet<String> {
    snapshot
        .pane_ids()
        .into_iter()
        .map(str::to_string)
        .collect()
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy::new(&WatcherConfig {
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(4),
            not_running_retry: Duration::from_secs(5),
            stable_after: Duration::from_secs(60),
            ..WatcherConfig::default()
        })
    }

    fn failed(not_running: bool) -> SessionEnd {
        SessionEnd::Failed {
            reason: "x".into(),
            not_running,
            stale_discovery: false,
        }
    }

    fn counted() -> SessionEnd {
        SessionEnd::Rebuild {
            cause: Some(ClientError::Closed("x".into())),
        }
    }

    fn uncounted() -> SessionEnd {
        SessionEnd::Rebuild { cause: None }
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let mut p = policy();
        let mut waits = Vec::new();
        for _ in 0..4 {
            p.on_session_end(None, &failed(false));
            waits.push(p.wait_for(false).as_secs());
        }
        assert_eq!(waits, vec![1, 2, 4, 4]);
    }

    #[test]
    fn not_running_uses_its_own_interval_and_does_not_double() {
        let mut p = policy();
        p.on_session_end(None, &failed(true));
        assert_eq!(p.wait_for(true), Duration::from_secs(5));
        p.on_session_end(None, &failed(false));
        assert_eq!(p.wait_for(false), Duration::from_secs(1));
    }

    #[test]
    fn backoff_resets_after_a_stable_session() {
        let mut p = policy();
        for _ in 0..3 {
            p.on_session_end(None, &failed(false));
        }
        p.on_session_end(Some(Duration::from_secs(59)), &failed(false));
        assert_eq!(p.wait_for(false), Duration::from_secs(4), "not yet stable");
        p.on_session_end(Some(Duration::from_secs(60)), &failed(false));
        assert_eq!(p.wait_for(false), Duration::from_secs(1));
        // Also when the stable session ends in a rebuild.
        p.on_session_end(None, &failed(false));
        p.on_session_end(Some(Duration::from_secs(61)), &counted());
        p.on_session_end(None, &failed(false));
        assert_eq!(p.wait_for(false), Duration::from_secs(1));
    }

    #[test]
    fn reconnect_resets_backoff_and_streak() {
        let mut p = policy();
        p.on_session_end(None, &failed(false));
        p.on_session_end(None, &failed(false));
        p.on_session_end(None, &counted());
        p.on_session_end(None, &SessionEnd::Reconnect);
        p.on_session_end(None, &failed(false));
        assert_eq!(p.wait_for(false), Duration::from_secs(1));
        assert_eq!(p.rebuild_streak, 0);
    }

    #[test]
    fn only_counted_rebuilds_build_the_streak() {
        let mut p = policy();
        for _ in 0..10 {
            p.on_session_end(Some(Duration::ZERO), &uncounted());
        }
        assert_eq!(p.rebuild_streak, 0);
        for _ in 0..MAX_REBUILD_STREAK {
            p.on_session_end(None, &counted());
            assert!(!p.cap_tripped());
        }
        p.on_session_end(Some(Duration::from_millis(10)), &counted());
        assert!(p.cap_tripped());
        p.on_session_end(None, &failed(false));
        assert_eq!(p.rebuild_streak, 0, "a failure clears the streak");
    }

    #[test]
    fn streak_resets_after_backoff_min_online() {
        let mut p = policy();
        for _ in 0..MAX_REBUILD_STREAK {
            p.on_session_end(None, &counted());
        }
        p.on_session_end(Some(Duration::from_secs(1)), &counted());
        assert_eq!(p.rebuild_streak, 1);
        assert!(!p.cap_tripped());
    }
}
