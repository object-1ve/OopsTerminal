========== crates/jcode-app-core/src/server/state.rs (26881 bytes) ==========
use crate::bus::FileOp;
use crate::plan::VersionedPlan;
use crate::protocol::ServerEvent;
use jcode_agent_runtime::{
    InterruptSignal, SoftInterruptMessage, SoftInterruptQueue, SoftInterruptSource,
};
use jcode_swarm_core::{SwarmLifecycleStatus, SwarmMemberRecord, SwarmRole};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Instant;
use tokio::sync::{RwLock, mpsc};

/// Process-global registry mapping session id -> background-tool signal.
///
/// The background-tool ("move tool to background", Alt+B/Ctrl+B) signal lives on
/// the `Agent`, so a `SessionControlHandle` can normally only obtain it by
/// locking the agent mutex. When a turn is busy (e.g. running `await_members`),
/// `refresh_session_control_handle` falls back to a lock-free `cancel_only`
/// handle that historically dropped the background signal entirely, which made
/// Alt+B/Ctrl+B silently no-op (`BACKGROUND_TOOL_SIGNAL_FIRE result=no_signal_handle`).
///
/// This registry is populated every time a full `SessionControlHandle` is built
/// (which always has both the session id and the correct signal), so the
/// lock-free fallback can still fire the background signal without the agent
/// lock. Entries are keyed by session id; renames/removals reuse
/// [`rename_background_tool_signal`]/[`remove_background_tool_signal`] alongside
/// the existing shutdown-signal lifecycle.
static BACKGROUND_TOOL_SIGNALS: LazyLock<StdMutex<HashMap<String, InterruptSignal>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Register (or replace) the background-tool signal for a session.
pub(super) fn register_background_tool_signal(session_id: &str, signal: InterruptSignal) {
    if let Ok(mut map) = BACKGROUND_TOOL_SIGNALS.lock() {
        map.insert(session_id.to_string(), signal);
    }
}

/// Look up the registered background-tool signal for a session, if any.
pub(super) fn background_tool_signal_for_session(session_id: &str) -> Option<InterruptSignal> {
    BACKGROUND_TOOL_SIGNALS
        .lock()
        .ok()
        .and_then(|map| map.get(session_id).cloned())
}

/// Move a session's background-tool signal registration to a new session id.
pub(super) fn rename_background_tool_signal(old_session_id: &str, new_session_id: &str) {
    if old_session_id == new_session_id {
        return;
    }
    if let Ok(mut map) = BACKGROUND_TOOL_SIGNALS.lock()
        && let Some(signal) = map.remove(old_session_id)
    {
        map.insert(new_session_id.to_string(), signal);
    }
}

/// Drop a session's background-tool signal registration.
pub(super) fn remove_background_tool_signal(session_id: &str) {
    if let Ok(mut map) = BACKGROUND_TOOL_SIGNALS.lock() {
        map.remove(session_id);
    }
}

/// Record of a file access by an agent
#[derive(Clone, Debug)]
pub struct FileAccess {
    pub session_id: String,
    pub op: FileOp,
    pub timestamp: Instant,
    pub absolute_time: std::time::SystemTime,
    pub intent: Option<String>,
    pub summary: Option<String>,
    pub detail: Option<String>,
}

pub(super) fn latest_peer_touches(
    accesses: &[FileAccess],
    current_session_id: &str,
    swarm_session_ids: &HashSet<String>,
) -> Vec<FileAccess> {
    let mut latest_by_session: HashMap<&str, &FileAccess> = HashMap::new();

    for access in accesses.iter().filter(|access| {
        access.session_id != current_session_id
            && swarm_session_ids.contains(&access.session_id)
            && access.op.is_modification()
    }) {
        latest_by_session
            .entry(&access.session_id)
            .and_modify(|existing| {
                if access.timestamp > existing.timestamp {
                    *existing = access;
                }
            })
            .or_insert(access);
    }

    let mut latest: Vec<FileAccess> = latest_by_session.into_values().cloned().collect();
    latest.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    latest
}

/// Shared ownership of the core persisted swarm coordination state.
#[derive(Clone)]
pub struct SwarmState {
    pub members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    pub coordinators: Arc<RwLock<HashMap<String, String>>>,
}

/// First-class snapshot of a single swarm's logical runtime state.
#[derive(Clone, Debug)]
pub struct SwarmRuntime {
    pub swarm_id: String,
    pub coordinator_session_id: Option<String>,
    pub member_session_ids: HashSet<String>,
    pub members: Vec<SwarmMember>,
    pub plan: Option<VersionedPlan>,
}

impl SwarmRuntime {
    pub fn has_any_state(&self) -> bool {
        self.plan.is_some() || self.coordinator_session_id.is_some() || !self.members.is_empty()
    }
}

/// Live transport attachment for a connected session.
#[derive(Clone, Debug)]
pub struct LiveSessionAttachment {
    pub connection_id: String,
    pub event_tx: mpsc::UnboundedSender<ServerEvent>,
}

impl SwarmState {
    pub fn new(
        members: HashMap<String, SwarmMember>,
        swarms_by_id: HashMap<String, HashSet<String>>,
        plans: HashMap<String, VersionedPlan>,
        coordinators: HashMap<String, String>,
    ) -> Self {
        Self {
            members: Arc::new(RwLock::new(members)),
            swarms_by_id: Arc::new(RwLock::new(swarms_by_id)),
            plans: Arc::new(RwLock::new(plans)),
            coordinators: Arc::new(RwLock::new(coordinators)),
        }
    }

    pub async fn load_runtime(&self, swarm_id: &str) -> SwarmRuntime {
        let plan = {
            let plans = self.plans.read().await;
            plans.get(swarm_id).cloned()
        };
        let coordinator_session_id = {
            let coordinators = self.coordinators.read().await;
            coordinators.get(swarm_id).cloned()
        };
        let member_session_ids = {
            let swarms = self.swarms_by_id.read().await;
            swarms.get(swarm_id).cloned().unwrap_or_default()
        };
        let mut members = {
            let members = self.members.read().await;
            members
                .values()
                .filter(|member| member.swarm_id.as_deref() == Some(swarm_id))
                .cloned()
                .collect::<Vec<_>>()
        };
        members.sort_by(|left, right| left.session_id.cmp(&right.session_id));

        SwarmRuntime {
            swarm_id: swarm_id.to_string(),
            coordinator_session_id,
            member_session_ids,
            members,
            plan,
        }
    }
}

/// Information about a session in a swarm
#[derive(Clone, Debug)]
pub struct SwarmMember {
    pub session_id: String,
    /// Primary channel to send events to this session.
    ///
    /// This remains for backward-compatible single-sender call sites and for
    /// headless sessions that do not maintain a live attachment map.
    pub event_tx: mpsc::UnboundedSender<ServerEvent>,
    /// Live client attachments for this session keyed by connection id.
    pub event_txs: HashMap<String, mpsc::UnboundedSender<ServerEvent>>,
    /// Working directory (used to derive swarm id)
    pub working_dir: Option<PathBuf>,
    /// Swarm identifier (shared across worktrees)
    pub swarm_id: Option<String>,
    /// Whether swarm coordination is enabled for this member
    pub swarm_enabled: bool,
    /// Lifecycle status (ready, running, completed, failed, stopped, etc.)
    pub status: String,
    /// Optional detail (current task, error, etc.)
    pub detail: Option<String>,
    /// Stable, human-readable label of the task/role this member was spawned
    /// or assigned for (compacted from the spawn prompt or plan item). Unlike
    /// `detail`, this is not overwritten by transient status updates.
    pub task_label: Option<String>,
    /// Friendly name like "fox"
    pub friendly_name: Option<String>,
    /// Session that should receive direct completion report-back for this member, if any.
    pub report_back_to_session_id: Option<String>,
    /// Latest explicit completion report submitted by this member.
    pub latest_completion_report: Option<String>,
    /// Role: "agent" or "coordinator"
    pub role: String,
    /// When this member joined the swarm
    pub joined_at: Instant,
    /// When status was last changed
    pub last_status_change: Instant,
    /// Whether this is a headless (spawned) session vs a TUI-connected session.
    /// Headless sessions should not be automatically elected as coordinator.
    pub is_headless: bool,
    /// Recent streamed output tail (last few lines of in-progress assistant
    /// text), captured for inline swarm gallery rendering. Updated by the bus
    /// monitor from worker streaming taps; not persisted.
    pub output_tail: Option<String>,
    /// Aggregate todo progress (completed, total) for this member's session,
    /// updated from `TodoUpdated` bus events. Surfaced on the inline swarm
    /// strip; not persisted.
    pub todo_progress: Option<(u32, u32)>,
    /// Compact snapshot of this member's todo list (content + status), capped
    /// at a few entries by the bus monitor. Rendered in the focused inline
    /// swarm panel; not persisted.
    pub todo_items: Vec<crate::protocol::SwarmTodoItem>,
    /// Ephemeral model/timing metadata for the inline swarm card.
    pub runtime: crate::protocol::SwarmMemberRuntime,
}

impl SwarmMember {
    pub fn durable_record(&self) -> SwarmMemberRecord {
        SwarmMemberRecord {
            session_id: self.session_id.clone(),
            working_dir: self.working_dir.clone(),
            swarm_id: self.swarm_id.clone(),
            swarm_enabled: self.swarm_enabled,
            status: SwarmLifecycleStatus::from(self.status.clone()),
            detail: self.detail.clone(),
            task_label: self.task_label.clone(),
            friendly_name: self.friendly_name.clone(),
            report_back_to_session_id: self.report_back_to_session_id.clone(),
            latest_completion_report: self.latest_completion_report.clone(),
            role: SwarmRole::from(self.role.clone()),
            is_headless: self.is_headless,
        }
    }

    pub fn live_attachments(&self) -> Vec<LiveSessionAttachment> {
        self.event_txs
            .iter()
            .map(|(connection_id, event_tx)| LiveSessionAttachment {
                connection_id: connection_id.clone(),
                event_tx: event_tx.clone(),
            })
            .collect()
    }

    pub fn from_record(
        record: SwarmMemberRecord,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Self {
        Self {
            session_id: record.session_id,
            event_tx,
            event_txs: HashMap::new(),
            working_dir: record.working_dir,
            swarm_id: record.swarm_id,
            swarm_enabled: record.swarm_enabled,
            status: record.status.as_str().into_owned(),
            detail: record.detail,
            task_label: record.task_label,
            friendly_name: record.friendly_name,
            report_back_to_session_id: record.report_back_to_session_id,
            latest_completion_report: record.latest_completion_report,
            role: record.role.as_str().into_owned(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: record.is_headless,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        }
    }
}

/// A shared context entry stored by the server
#[derive(Clone, Debug)]
pub struct SharedContext {
    pub key: String,
    pub value: String,
    pub from_session: String,
    pub from_name: Option<String>,
    /// When this context was created
    pub created_at: Instant,
    /// When this context was last updated
    pub updated_at: Instant,
}

/// Event types for real-time event subscription
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SwarmEventType {
    /// A file was touched (read/write/edit)
    FileTouch {
        path: String,
        op: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A notification was broadcast
    Notification {
        notification_type: String,
        message: String,
    },
    /// A swarm plan was updated
    PlanUpdate { swarm_id: String, item_count: usize },
    /// A plan proposal was submitted
    PlanProposal {
        swarm_id: String,
        proposer_session: String,
        item_count: usize,
    },
    /// Shared context was updated
    ContextUpdate { swarm_id: String, key: String },
    /// Session status changed
    StatusChange {
        old_status: String,
        new_status: String,
    },
    /// Session joined/left swarm
    MemberChange {
        action: String, // "joined" or "left"
    },
}

/// A swarm event with metadata
#[derive(Clone, Debug)]
pub struct SwarmEvent {
    pub id: u64,
    pub session_id: String,
    pub session_name: Option<String>,
    pub swarm_id: Option<String>,
    pub event: SwarmEventType,
    pub timestamp: Instant,
    pub absolute_time: std::time::SystemTime,
}

/// Ring buffer for recent swarm events
pub(super) const MAX_EVENT_HISTORY: usize = 5000;

pub(super) type SessionInterruptQueues = Arc<RwLock<HashMap<String, SoftInterruptQueue>>>;

pub(super) async fn register_session_event_sender(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    connection_id: &str,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) {
    let mut members = swarm_members.write().await;
    if let Some(member) = members.get_mut(session_id) {
        member.event_tx = event_tx.clone();
        member.event_txs.insert(connection_id.to_string(), event_tx);
    }
}

pub(super) async fn unregister_session_event_sender(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    connection_id: &str,
) {
    let mut members = swarm_members.write().await;
    if let Some(member) = members.get_mut(session_id) {
        member.event_txs.remove(connection_id);
        if let Some((_, tx)) = member.event_txs.iter().next() {
            member.event_tx = tx.clone();
        }
    }
}

pub(super) async fn fanout_session_event(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    event: ServerEvent,
) -> usize {
    let targets = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(session_id) else {
            return 0;
        };

        member.event_txs.retain(|_, tx| !tx.is_closed());

        if member.event_txs.is_empty() {
            vec![member.event_tx.clone()]
        } else {
            if let Some((_, tx)) = member.event_txs.iter().next() {
                member.event_tx = tx.clone();
            }
            member.event_txs.values().cloned().collect::<Vec<_>>()
        }
    };

    let mut delivered = 0;
    for tx in targets {
        if tx.send(event.clone()).is_ok() {
            delivered += 1;
        }
    }
    delivered
}

pub(super) async fn fanout_live_client_event(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    event: ServerEvent,
) -> usize {
    let targets = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(session_id) else {
            return 0;
        };

        member.event_txs.retain(|_, tx| !tx.is_closed());
        member.event_txs.values().cloned().collect::<Vec<_>>()
    };

    let mut delivered = 0;
    for tx in targets {
        if tx.send(event.clone()).is_ok() {
            delivered += 1;
        }
    }
    delivered
}

pub(super) fn session_event_fanout_sender(
    session_id: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> mpsc::UnboundedSender<ServerEvent> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = fanout_session_event(&swarm_members, &session_id, event).await;
        }
    });
    tx
}

pub(super) fn session_event_fanout_sender_with_fallback(
    session_id: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    fallback_tx: mpsc::UnboundedSender<ServerEvent>,
) -> mpsc::UnboundedSender<ServerEvent> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if fanout_session_event(&swarm_members, &session_id, event.clone()).await == 0 {
                let _ = fallback_tx.send(event);
            }
        }
    });
    tx
}

pub(super) fn enqueue_soft_interrupt(
    queue: &SoftInterruptQueue,
    content: String,
    images: Vec<(String, String)>,
    urgent: bool,
    source: SoftInterruptSource,
) -> bool {
    let content_bytes = content.len();
    let content_chars = content.chars().count();
    if let Ok(mut pending) = queue.lock() {
        let pending_before = pending.len();
        pending.push(SoftInterruptMessage {
            content,
            images,
            urgent,
            source,
        });
        crate::logging::info(&format!(
            "SOFT_INTERRUPT_QUEUE_PUSH source={:?} urgent={} content_bytes={} content_chars={} pending_before={} pending_after={}",
            source,
            urgent,
            content_bytes,
            content_chars,
            pending_before,
            pending.len()
        ));
        true
    } else {
        crate::logging::warn(&format!(
            "SOFT_INTERRUPT_QUEUE_PUSH_FAILED source={:?} urgent={} content_bytes={} content_chars={} reason=queue_lock_poisoned",
            source, urgent, content_bytes, content_chars
        ));
        false
    }
}

/// Lock-free control-plane handles for a live session.
///
/// This intentionally exposes only out-of-band controls that are safe to use
/// while a turn owns the Agent mutex. Stateful operations such as history
/// mutation, model changes, or direct tool execution should continue to
/// coordinate through the Agent lock after the turn is idle/stopped.
#[derive(Clone)]
pub struct SessionControlHandle {
    pub session_id: String,
    soft_interrupt_queue: SoftInterruptQueue,
    background_tool_signal: Option<InterruptSignal>,
    stop_current_turn_signal: InterruptSignal,
}

impl SessionControlHandle {
    pub fn new(
        session_id: impl Into<String>,
        soft_interrupt_queue: SoftInterruptQueue,
        background_tool_signal: InterruptSignal,
        stop_current_turn_signal: InterruptSignal,
    ) -> Self {
        let session_id = session_id.into();
        // Mirror the signal into the process-global registry so the lock-free
        // `cancel_only` fallback (used while the agent mutex is busy, e.g. during
        // `await_members`) can still fire it. Without this, Alt+B/Ctrl+B silently
        // no-ops for busy turns.
        register_background_tool_signal(&session_id, background_tool_signal.clone());
        Self {
            session_id,
            soft_interrupt_queue,
            background_tool_signal: Some(background_tool_signal),
            stop_current_turn_signal,
        }
    }

    pub fn cancel_only(
        session_id: impl Into<String>,
        soft_interrupt_queue: SoftInterruptQueue,
        stop_current_turn_signal: InterruptSignal,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            soft_interrupt_queue,
            background_tool_signal: None,
            stop_current_turn_signal,
        }
    }

    pub fn queue_soft_interrupt(
        &self,
        content: String,
        images: Vec<(String, String)>,
        urgent: bool,
        source: SoftInterruptSource,
    ) -> bool {
        enqueue_soft_interrupt(&self.soft_interrupt_queue, content, images, urgent, source)
    }

    pub fn clear_soft_interrupts(&self) {
        if let Ok(mut queue) = self.soft_interrupt_queue.lock() {
            let cleared = queue.len();
            queue.clear();
            crate::logging::info(&format!(
                "SOFT_INTERRUPT_QUEUE_CLEAR session={} cleared={}",
                self.session_id, cleared
            ));
        } else {
            crate::logging::warn(&format!(
                "SOFT_INTERRUPT_QUEUE_CLEAR_FAILED session={} reason=queue_lock_poisoned",
                self.session_id
            ));
        }
    }

    /// Fire the stop-current-turn signal. Returns the signal's fire epoch so
    /// callers that schedule a deferred [`reset_cancel_if_epoch`](Self::reset_cancel_if_epoch)
    /// can avoid erasing a newer cancel that fired in the meantime (issue #428).
    ///
    /// Also fires every cancel signal registered for currently running turns
    /// of this session. The handle's own signal can be a stale instance that
    /// the streaming turn never observes (reattach after reload/disconnect,
    /// server-initiated turns, headless recovery), which used to make Esc show
    /// "Interrupting..." while the model kept generating for minutes
    /// (issue #428).
    pub fn request_cancel(&self) -> u64 {
        crate::logging::info(&format!(
            "SESSION_CANCEL_SIGNAL_FIRE session={}",
            self.session_id
        ));
        self.stop_current_turn_signal.fire();
        let active_turn_signals =
            crate::turn_cancel_registry::active_turn_signals(&self.session_id);
        let mut fired_active = 0usize;
        for signal in &active_turn_signals {
            if signal.same_instance(&self.stop_current_turn_signal) {
                continue;
            }
            signal.fire();
            fired_active += 1;
        }
        if fired_active > 0 {
            crate::logging::info(&format!(
                "SESSION_CANCEL_ACTIVE_TURN_SIGNALS_FIRED session={} fired={} registered={}",
                self.session_id,
                fired_active,
                active_turn_signals.len()
            ));
        }
        self.stop_current_turn_signal.epoch()
    }

    pub fn reset_cancel(&self) {
        crate::logging::info(&format!(
            "SESSION_CANCEL_SIGNAL_RESET session={}",
            self.session_id
        ));
        self.stop_current_turn_signal.reset();
    }

    /// Reset the cancel signal only if no newer cancel fired since `epoch`
    /// was captured from [`request_cancel`](Self::request_cancel). Timed
    /// resets (used when the running turn is not owned by this connection)
    /// must use this instead of [`reset_cancel`](Self::reset_cancel):
    /// an unconditional deferred reset can erase a newer, not-yet-observed
    /// cancel, making repeated Esc presses appear to be ignored (issue #428).
    pub fn reset_cancel_if_epoch(&self, epoch: u64) -> bool {
        let reset = self.stop_current_turn_signal.reset_if_epoch(epoch);
        crate::logging::info(&format!(
            "SESSION_CANCEL_SIGNAL_RESET session={} epoch={} applied={}",
            self.session_id, epoch, reset
        ));
        reset
    }

    pub fn request_background_current_tool(&self) -> bool {
        // Prefer the directly-held signal; fall back to the process-global
        // registry for lock-free (`cancel_only`) handles built while the agent
        // mutex was busy. This is what makes Alt+B/Ctrl+B work during a busy
        // turn such as `await_members`.
        let signal = self
            .background_tool_signal
            .clone()
            .or_else(|| background_tool_signal_for_session(&self.session_id));
        if let Some(signal) = signal {
            signal.fire();
            crate::logging::info(&format!(
                "BACKGROUND_TOOL_SIGNAL_FIRE session={} result=sent",
                self.session_id
            ));
            true
        } else {
            crate::logging::warn(&format!(
                "BACKGROUND_TOOL_SIGNAL_FIRE session={} result=no_signal_handle",
                self.session_id
            ));
            false
        }
    }

    pub fn stop_current_turn_signal(&self) -> InterruptSignal {
        self.stop_current_turn_signal.clone()
    }
}

pub(super) async fn register_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    session_id: &str,
    queue: SoftInterruptQueue,
) {
    let mut guard = queues.write().await;
    guard.insert(session_id.to_string(), queue);
}

pub(super) async fn rename_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    old_session_id: &str,
    new_session_id: &str,
) {
    let mut guard = queues.write().await;
    if let Some(queue) = guard.remove(old_session_id) {
        guard.insert(new_session_id.to_string(), queue);
    }
}

pub(super) async fn remove_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    session_id: &str,
) {
    let mut guard = queues.write().await;
    guard.remove(session_id);
}

pub(super) async fn queue_soft_interrupt_for_session(
    session_id: &str,
    content: String,
    urgent: bool,
    source: SoftInterruptSource,
    queues: &SessionInterruptQueues,
    sessions: &super::SessionAgents,
) -> bool {
    if let Some(queue) = queues.read().await.get(session_id).cloned() {
        return enqueue_soft_interrupt(&queue, content, Vec::new(), urgent, source);
    }

    let queue = {
        let guard = sessions.read().await;
        guard.get(session_id).and_then(|agent| {
            agent
                .try_lock()
                .ok()
                .map(|agent_guard| agent_guard.soft_interrupt_queue())
        })
    };

    if let Some(queue) = queue {
        register_session_interrupt_queue(queues, session_id, queue.clone()).await;
        enqueue_soft_interrupt(&queue, content, Vec::new(), urgent, source)
    } else {
        let session_exists = {
            let guard = sessions.read().await;
            guard.contains_key(session_id)
        } || crate::session::session_exists(session_id);

        if !session_exists {
            return false;
        }

        crate::soft_interrupt_store::append(
            session_id,
            SoftInterruptMessage {
                content,
                images: Vec::new(),
                urgent,
                source,
            },
        )
        .map(|_| true)
        .unwrap_or_else(|err| {
            crate::logging::warn(&format!(
                "Failed to persist deferred soft interrupt for session {}: {}",
                session_id, err
            ));
            false
        })
    }
}

========== crates/jcode-app-core/src/server/client_session.rs (66417 bytes) ==========
#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::client_state::{handle_get_history, spawn_model_prefetch_update};
use super::{
    ClientConnectionInfo, ClientDebugState, FileTouchService, SessionInterruptQueues, SwarmEvent,
    SwarmMember, SwarmState, VersionedPlan, broadcast_swarm_status, fanout_live_client_event,
    persist_swarm_state_for, register_background_tool_signal, register_session_event_sender,
    register_session_interrupt_queue, remove_background_tool_signal, remove_plan_participant,
    remove_session_channel_subscriptions, remove_session_from_swarm,
    remove_session_interrupt_queue, rename_background_tool_signal, rename_plan_participant,
    rename_session_interrupt_queue, send_swarm_plan_to_session, swarm_id_for_dir,
    unregister_session_event_sender, update_member_status,
};
use crate::agent::Agent;
use crate::message::ContentBlock;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::WriteHalf;
use anyhow::Result;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;
const RELOAD_RESTORE_MARKER_MAX_AGE: Duration = Duration::from_secs(60);

pub(super) fn session_was_interrupted_by_reload(agent: &Agent) -> bool {
    let messages = agent.messages();
    let Some(last) = messages.last() else {
        return false;
    };

    last.content.iter().any(|block| match block {
        ContentBlock::Text { text, .. } => {
            text.ends_with("[generation interrupted - server reloading]")
        }
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            content == "Reload initiated. Process restarting..."
                || (is_error.unwrap_or(false)
                    && (content.contains("interrupted by server reload")
                        || content.contains("Skipped - server reloading")))
        }
        _ => false,
    })
}

pub(super) fn restored_session_was_interrupted(
    session_id: &str,
    previous_status: &crate::session::SessionStatus,
    agent: &Agent,
) -> bool {
    let last_is_user = agent
        .last_message_role()
        .as_ref()
        .map(|role| *role == crate::message::Role::User)
        .unwrap_or(false);
    let last_is_reload_interrupted = session_was_interrupted_by_reload(agent);
    let closed_pending_user_during_reload =
        matches!(previous_status, crate::session::SessionStatus::Closed)
            && last_is_user
            && crate::server::reload_marker_active(RELOAD_RESTORE_MARKER_MAX_AGE);

    if last_is_user && matches!(previous_status, crate::session::SessionStatus::Active) {
        crate::logging::info(&format!(
            "Session {} was Active with pending user message - treating as interrupted",
            session_id
        ));
    }

    if last_is_reload_interrupted {
        crate::logging::info(&format!(
            "Session {} contains reload interruption markers - will auto-resume",
            session_id
        ));
    }

    if closed_pending_user_during_reload {
        crate::logging::info(&format!(
            "Session {} was Closed with a pending user message during a recent reload - treating as interrupted",
            session_id
        ));
    }

    matches!(
        previous_status,
        crate::session::SessionStatus::Crashed { .. }
    ) || (matches!(previous_status, crate::session::SessionStatus::Active) && last_is_user)
        || last_is_reload_interrupted
        || closed_pending_user_during_reload
}

fn mark_remote_reload_started(request_id: &str) {
    crate::server::write_reload_state(
        request_id,
        jcode_build_meta::version(),
        crate::server::ReloadPhase::Starting,
        None,
    );
}

async fn rename_shutdown_signal(
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    old_session_id: &str,
    new_session_id: &str,
) {
    if old_session_id == new_session_id {
        return;
    }

    let mut signals = shutdown_signals.write().await;
    if let Some(signal) = signals.remove(old_session_id) {
        signals.insert(new_session_id.to_string(), signal);
    }
    drop(signals);
    rename_background_tool_signal(old_session_id, new_session_id);
    // In-flight turns are registered in the process-global cancel registry by
    // session id. Attaching to / resuming a session renames it underneath a
    // still-streaming turn, so the registration must follow, or a later Esc
    // finds no active-turn signal for the new id and the model keeps
    // generating (issue #732, regression of issue #428).
    crate::turn_cancel_registry::rename_active_turns(old_session_id, new_session_id);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_clear_session(
    id: u64,
    client_selfdev: bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let clear_start = Instant::now();
    let old_session_id = client_session_id.clone();
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "clear_start".to_string()),
            ("request_id", id.to_string()),
            ("session_id", old_session_id.clone()),
            ("client_connection_id", client_connection_id.to_string()),
            ("client_selfdev", client_selfdev.to_string()),
        ],
    );
    let (preserve_debug, working_dir) = {
        let agent_guard = agent.lock().await;
        (
            agent_guard.is_debug(),
            agent_guard.working_dir().map(str::to_string),
        )
    };

    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let mut new_agent = Agent::new_with_initial_working_dir(
        Arc::clone(provider),
        registry.clone(),
        working_dir.as_deref(),
    );
    let new_id = new_agent.session_id().to_string();

    if client_selfdev {
        new_agent.set_canary("self-dev");
    }
    if preserve_debug {
        new_agent.set_debug(true);
    }

    let mut agent_guard = agent.lock().await;
    *agent_guard = new_agent;
    drop(agent_guard);

    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.remove(client_session_id);
        sessions_guard.insert(new_id.clone(), Arc::clone(agent));
    }
    crate::runtime_memory_log::emit_event(
        crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
            "session_cleared",
            "session_replaced_with_fresh_agent",
        )
        .with_session_id(new_id.clone())
        .force_attribution(),
    );
    {
        let agent_guard = agent.lock().await;
        register_session_interrupt_queue(
            soft_interrupt_queues,
            &new_id,
            agent_guard.soft_interrupt_queue(),
        )
        .await;

        let mut signals = shutdown_signals.write().await;
        signals.remove(client_session_id);
        signals.insert(new_id.clone(), agent_guard.graceful_shutdown_signal());
        drop(signals);
        remove_background_tool_signal(client_session_id);
        register_background_tool_signal(&new_id, agent_guard.background_tool_signal());
    }
    remove_session_interrupt_queue(soft_interrupt_queues, client_session_id).await;

    let swarm_id_for_update = {
        let mut members = swarm_members.write().await;
        if let Some(mut member) = members.remove(client_session_id) {
            let swarm_id = member.swarm_id.clone();
            member.session_id = new_id.clone();
            member.status = "ready".to_string();
            member.detail = None;
            members.insert(new_id.clone(), member);
            swarm_id
        } else {
            None
        }
    };
    if let Some(ref swarm_id) = swarm_id_for_update {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(client_session_id);
            swarm.insert(new_id.clone());
        }
    }
    file_touch.clear_session(client_session_id).await;
    remove_session_channel_subscriptions(
        client_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;
    update_member_status(
        &new_id,
        "ready",
        None,
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;
    if let Some(ref swarm_id) = swarm_id_for_update {
        rename_plan_participant(swarm_id, client_session_id, &new_id, swarm_plans).await;
    }

    *client_session_id = new_id.clone();
    {
        let mut connections = client_connections.write().await;
        if let Some(info) = connections.get_mut(client_connection_id) {
            info.session_id = new_id.clone();
            info.last_seen = Instant::now();
        }
    }
    let _ = client_event_tx.send(ServerEvent::SessionId { session_id: new_id });
    let _ = client_event_tx.send(ServerEvent::Done { id });
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "clear_done".to_string()),
            ("request_id", id.to_string()),
            ("old_session_id", old_session_id),
            ("new_session_id", client_session_id.clone()),
            ("client_connection_id", client_connection_id.to_string()),
            ("preserve_debug", preserve_debug.to_string()),
            (
                "swarm_id_updated",
                swarm_id_for_update.is_some().to_string(),
            ),
            ("elapsed_ms", clear_start.elapsed().as_millis().to_string()),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
async fn ensure_client_swarm_member(
    client_session_id: &str,
    client_connection_id: &str,
    friendly_name: &Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    agent: &Arc<Mutex<Agent>>,
    swarm_enabled: bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> bool {
    let (working_dir, derived_swarm_id, fallback_name) = {
        // A target-aware subscribe can attach to an agent that is in the middle
        // of a turn. Never wait for that turn's agent lock just to populate
        // connection metadata: doing so prevents the subscribe request from
        // completing, so subsequent state requests sit unread until the desktop
        // client times out. The persisted startup stub has the same immutable
        // identity metadata and is safe to read while the live agent is busy.
        let (working_dir, fallback_name) = match agent.try_lock() {
            Ok(agent_guard) => (
                agent_guard.working_dir().map(PathBuf::from),
                agent_guard
                    .session_short_name()
                    .map(|value| value.to_string()),
            ),
            Err(_) => {
                crate::logging::info(&format!(
                    "Subscribe metadata for busy session {} is using the persisted startup stub",
                    client_session_id
                ));
                crate::session::Session::load_startup_stub(client_session_id)
                    .map(|session| (session.working_dir.map(PathBuf::from), session.short_name))
                    .unwrap_or((None, None))
            }
        };
        let derived_swarm_id = if swarm_enabled {
            swarm_id_for_dir(working_dir.clone())
        } else {
            None
        };
        (working_dir, derived_swarm_id, fallback_name)
    };

    // Prefer the currently restored agent/session identity over the temporary
    // name captured at raw socket accept time. During resume/reconnect bursts,
    // the temporary pre-resume session name can otherwise leak onto the real
    // resumed session and corrupt swarm metadata.
    let member_name = fallback_name.or_else(|| friendly_name.clone());
    let mut inserted = false;
    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(client_session_id) {
            member.event_tx = client_event_tx.clone();
            member
                .event_txs
                .insert(client_connection_id.to_string(), client_event_tx.clone());
            member.swarm_enabled = swarm_enabled;
            member.is_headless = false;
            if member_name.is_some() {
                member.friendly_name = member_name.clone();
            }
        } else {
            let now = Instant::now();
            members.insert(
                client_session_id.to_string(),
                SwarmMember {
                    session_id: client_session_id.to_string(),
                    event_tx: client_event_tx.clone(),
                    event_txs: HashMap::from([(
                        client_connection_id.to_string(),
                        client_event_tx.clone(),
                    )]),
                    working_dir: working_dir.clone(),
                    swarm_id: derived_swarm_id.clone(),
                    swarm_enabled,
                    status: "ready".to_string(),
                    detail: None,
                    task_label: None,
                    friendly_name: member_name.clone(),
                    report_back_to_session_id: None,
                    latest_completion_report: None,
                    role: "agent".to_string(),
                    joined_at: now,
                    last_status_change: now,
                    is_headless: false,
                    output_tail: None,
                    todo_progress: None,
                    todo_items: Vec::new(),
                    runtime: crate::protocol::SwarmMemberRuntime::default(),
                },
            );
            inserted = true;
        }
    }

    if inserted && let Some(ref swarm_id_ref) = derived_swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(swarm_id_ref.to_string())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.to_string());
        drop(swarms);
        super::record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            client_session_id.to_string(),
            member_name,
            Some(swarm_id_ref.to_string()),
            crate::server::SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
        )
        .await;
    }

    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "swarm_member_registered".to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_connection_id", client_connection_id.to_string()),
            ("inserted", inserted.to_string()),
            ("swarm_enabled", swarm_enabled.to_string()),
            (
                "swarm_id",
                derived_swarm_id.unwrap_or_else(|| "none".to_string()),
            ),
        ],
    );

    inserted
}

/// Resolve the working directory a subscribe should actually bind to.
///
/// Returns the reported dir when it is acceptable, or the session's existing
/// dir when the report is rejected by [`subscribe_working_dir_replacement`].
/// Every consumer of a subscribe cwd (agent state, swarm id, project-local MCP
/// resolution) must agree on this one answer, otherwise the session's tools,
/// swarm grouping, and MCP config can each resolve against a different
/// directory (issue #481).
pub(super) fn effective_subscribe_working_dir(
    current: Option<&str>,
    reported: &str,
    home: Option<&Path>,
) -> String {
    match subscribe_working_dir_replacement(current, reported, home) {
        Some(accepted) => accepted,
        None => current
            .map(str::to_string)
            .unwrap_or_else(|| reported.trim().to_string()),
    }
}

/// Decide whether a client-reported subscribe cwd may replace the session's
/// current working directory.
///
/// Requiring a subscribe cwd to be non-empty and absolute (the earlier
/// require-cwd change) is necessary but not sufficient: a client that launches
/// with an inherited environment can report the user's *home* directory even
/// though the real project lives elsewhere. Accepting that silently re-pins the
/// session to home, so bash/file tools run against home while the header still
/// shows the project path (issue #481).
///
/// The rule is deliberately narrow so it cannot break legitimate directory
/// changes: a reported cwd that is exactly the home directory is ignored *only*
/// when the session already has a different working directory. Working in home
/// on purpose (no prior cwd, or a session already pinned to home) still works,
/// and every other path is accepted as before.
pub(super) fn subscribe_working_dir_replacement(
    current: Option<&str>,
    reported: &str,
    home: Option<&Path>,
) -> Option<String> {
    let reported_trimmed = reported.trim();
    if reported_trimmed.is_empty() {
        return None;
    }
    let current = current.map(str::trim).filter(|dir| !dir.is_empty());
    if current == Some(reported_trimmed) {
        return None;
    }
    if let (Some(current), Some(home)) = (current, home)
        && Path::new(reported_trimmed) == home
        && Path::new(current) != home
    {
        return None;
    }
    Some(reported_trimmed.to_string())
}

fn log_ignored_subscribe_working_dir(session_id: &str, current: &str, reported: &str) {
    crate::logging::warn(&format!(
        "Ignoring subscribe working_dir {} for session {}: it is the home directory while the session is already bound to {} (issue #481)",
        reported, session_id, current
    ));
}

fn apply_or_defer_subscribe_working_dir(
    agent: &Arc<Mutex<Agent>>,
    working_dir: &str,
    session_id: &str,
) {
    let home = dirs::home_dir();
    if let Ok(mut agent_guard) = agent.try_lock() {
        match subscribe_working_dir_replacement(
            agent_guard.working_dir(),
            working_dir,
            home.as_deref(),
        ) {
            Some(accepted) => agent_guard.set_working_dir(&accepted),
            None => {
                if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                {
                    log_ignored_subscribe_working_dir(session_id, current, working_dir);
                }
            }
        }
        return;
    }

    let agent = Arc::clone(agent);
    let working_dir = working_dir.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        match subscribe_working_dir_replacement(
            agent_guard.working_dir(),
            &working_dir,
            home.as_deref(),
        ) {
            Some(accepted) => {
                agent_guard.set_working_dir(&accepted);
                crate::logging::info(&format!(
                    "Applied deferred subscribe working directory for session {}",
                    session_id
                ));
            }
            None => {
                if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                {
                    log_ignored_subscribe_working_dir(&session_id, current, &working_dir);
                }
            }
        }
    });
}

fn apply_or_defer_subscribe_selfdev(agent: &Arc<Mutex<Agent>>, session_id: &str) {
    if let Ok(mut agent_guard) = agent.try_lock() {
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        return;
    }

    let agent = Arc::clone(agent);
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        crate::logging::info(&format!(
            "Applied deferred self-dev subscribe metadata for session {}",
            session_id
        ));
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_subscribe(
    id: u64,
    subscribe_working_dir: Option<String>,
    selfdev: Option<bool>,
    register_mcp_tools: bool,
    client_selfdev: &mut bool,
    client_session_id: &str,
    client_connection_id: &str,
    friendly_name: &Option<String>,
    agent: &Arc<Mutex<Agent>>,
    registry: &Registry,
    swarm_enabled: bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let subscribe_start = Instant::now();
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "subscribe_start".to_string()),
            ("request_id", id.to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_connection_id", client_connection_id.to_string()),
            (
                "working_dir_set",
                subscribe_working_dir.is_some().to_string(),
            ),
            ("register_mcp_tools", register_mcp_tools.to_string()),
            ("swarm_enabled", swarm_enabled.to_string()),
        ],
    );
    ensure_client_swarm_member(
        client_session_id,
        client_connection_id,
        friendly_name,
        client_event_tx,
        agent,
        swarm_enabled,
        swarm_members,
        swarms_by_id,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;

    if let Some(ref dir) = subscribe_working_dir {
        apply_or_defer_subscribe_working_dir(agent, dir, client_session_id);

        // Swarm grouping must use the *bound* directory, not the raw report, or
        // a home-dir subscribe would still re-key the session's swarm even
        // though its agent stayed in the project (issue #481).
        let bound_dir = {
            let current = agent
                .try_lock()
                .ok()
                .and_then(|guard| guard.working_dir().map(str::to_string));
            effective_subscribe_working_dir(current.as_deref(), dir, dirs::home_dir().as_deref())
        };
        let new_path = PathBuf::from(&bound_dir);
        let new_swarm_id = swarm_id_for_dir(Some(new_path.clone()));
        let mut old_swarm_id: Option<String> = None;
        let mut updated_swarm_id: Option<String> = None;
        {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                old_swarm_id = member.swarm_id.clone();
                member.working_dir = Some(new_path);
                member.swarm_id = if member.swarm_enabled {
                    new_swarm_id.clone()
                } else {
                    None
                };
                updated_swarm_id = member.swarm_id.clone();
            }
        }

        if let Some(ref old_id) = old_swarm_id {
            if updated_swarm_id.as_ref() != Some(old_id) {
                remove_session_channel_subscriptions(
                    client_session_id,
                    channel_subscriptions,
                    channel_subscriptions_by_session,
                )
                .await;
            }
            let mut swarms = swarms_by_id.write().await;
            if let Some(swarm) = swarms.get_mut(old_id) {
                swarm.remove(client_session_id);
                if swarm.is_empty() {
                    swarms.remove(old_id);
                }
            }
        }

        if let Some(ref new_id) = updated_swarm_id {
            let mut swarms = swarms_by_id.write().await;
            swarms
                .entry(new_id.clone())
                .or_insert_with(HashSet::new)
                .insert(client_session_id.to_string());
        }

        if updated_swarm_id != old_swarm_id {
            crate::logging::event_info(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "subscribe_swarm_changed".to_string()),
                    ("session_id", client_session_id.to_string()),
                    ("client_connection_id", client_connection_id.to_string()),
                    (
                        "old_swarm_id",
                        old_swarm_id.clone().unwrap_or_else(|| "none".to_string()),
                    ),
                    (
                        "new_swarm_id",
                        updated_swarm_id
                            .clone()
                            .unwrap_or_else(|| "none".to_string()),
                    ),
                ],
            );
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                member.role = "agent".to_string();
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(&old_id)
                    .map(|session_id| session_id == client_session_id)
                    .unwrap_or(false)
            };
            if was_coordinator {
                let mut new_coordinator: Option<String> = None;
                {
                    let swarms = swarms_by_id.read().await;
                    if let Some(swarm) = swarms.get(&old_id) {
                        new_coordinator = swarm.iter().min().cloned();
                    }
                }
                {
                    let mut coordinators = swarm_coordinators.write().await;
                    coordinators.remove(&old_id);
                    if let Some(ref new_id) = new_coordinator {
                        coordinators.insert(old_id.clone(), new_id.clone());
                    }
                }
                if let Some(new_id) = new_coordinator.clone() {
                    let members = swarm_members.read().await;
                    if let Some(member) = members.get(&new_id) {
                        let _ = member.event_tx.send(ServerEvent::Notification {
                            from_session: new_id.clone(),
                            from_name: member.friendly_name.clone(),
                            notification_type: NotificationType::Message {
                                scope: Some("swarm".to_string()),
                                channel: None,
                                tldr: None,
                            },
                            message: "You are now the coordinator for this swarm.".to_string(),
                        });
                    }
                }
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            if updated_swarm_id.as_ref() != Some(&old_id) {
                remove_plan_participant(&old_id, client_session_id, swarm_plans).await;
                let swarm_state = SwarmState {
                    members: Arc::clone(swarm_members),
                    swarms_by_id: Arc::clone(swarms_by_id),
                    plans: Arc::clone(swarm_plans),
                    coordinators: Arc::clone(swarm_coordinators),
                };
                persist_swarm_state_for(&old_id, &swarm_state).await;
            }
            broadcast_swarm_status(&old_id, swarm_members, swarms_by_id).await;
        }
        if let Some(new_id) = updated_swarm_id
            && old_swarm_id.as_ref() != Some(&new_id)
        {
            broadcast_swarm_status(&new_id, swarm_members, swarms_by_id).await;
        }
    }

    let should_selfdev = *client_selfdev || matches!(selfdev, Some(true));

    if should_selfdev {
        *client_selfdev = true;
        apply_or_defer_subscribe_selfdev(agent, client_session_id);
        registry.register_selfdev_tools().await;
    }

    let mcp_register_ms = if register_mcp_tools {
        let mcp_register_start = Instant::now();
        // Resolve project-local MCP config against the session working dir,
        // not the server process cwd (issue #420). Prefer the subscribe
        // request's dir; fall back to the agent's stored session dir.
        let mcp_working_dir = match subscribe_working_dir.as_ref() {
            // Resolve against the bound directory so a rejected home-dir report
            // cannot point project-local MCP discovery at home (issue #481).
            Some(dir) => {
                let current = agent
                    .try_lock()
                    .ok()
                    .and_then(|guard| guard.working_dir().map(str::to_string));
                Some(PathBuf::from(effective_subscribe_working_dir(
                    current.as_deref(),
                    dir,
                    dirs::home_dir().as_deref(),
                )))
            }
            None => agent
                .try_lock()
                .ok()
                .and_then(|agent_guard| agent_guard.working_dir().map(PathBuf::from))
                .or_else(|| {
                    crate::session::Session::load_startup_stub(client_session_id)
                        .ok()
                        .and_then(|session| session.working_dir.map(PathBuf::from))
                }),
        };
        registry
            .register_mcp_tools_for_dir(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(client_session_id.to_string()),
                mcp_working_dir,
            )
            .await;
        mcp_register_start.elapsed().as_millis()
    } else {
        0
    };

    crate::logging::info(&format!(
        "[TIMING] handle_subscribe: session={}, working_dir_set={}, selfdev={}, mcp_register={}ms, total={}ms",
        client_session_id,
        subscribe_working_dir.is_some(),
        should_selfdev,
        mcp_register_ms,
        subscribe_start.elapsed().as_millis(),
    ));
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "subscribe_done".to_string()),
            ("request_id", id.to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_connection_id", client_connection_id.to_string()),
            ("mcp_register_ms", mcp_register_ms.to_string()),
            (
                "elapsed_ms",
                subscribe_start.elapsed().as_millis().to_string(),
            ),
        ],
    );

    if subscribe_should_mark_ready(client_session_id, swarm_members).await {
        update_member_status(
            client_session_id,
            "ready",
            None,
            swarm_members,
            swarms_by_id,
            Some(event_history),
            Some(event_counter),
            Some(swarm_event_tx),
        )
        .await;
    }

    // Re-send the current swarm plan so a reconnecting client renders the
    // plan graph immediately instead of waiting for the next plan mutation.
    send_swarm_plan_to_session(client_session_id, swarm_members, swarm_plans).await;

    // Tell the client which session it is bound to. Local clients learn this
    // from their own launch state, but a remote client (gateway/WebSocket) has
    // no other source, and without it a dropped connection cannot reattach:
    // the next Subscribe carries no `target_session_id`, so the server hands
    // it a brand-new session and the in-flight turn becomes unreachable.
    let _ = client_event_tx.send(ServerEvent::SessionId {
        session_id: client_session_id.to_string(),
    });
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

async fn subscribe_should_mark_ready(
    client_session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let members = swarm_members.read().await;
    members
        .get(client_session_id)
        .is_none_or(|member| member.status != "running")
}

async fn rename_swarm_member_session(
    old_session_id: &str,
    new_session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    // Never hold both swarm maps at once. Coordinator cleanup reads them in the
    // opposite order, so retaining the member write guard while waiting for the
    // swarm map can permanently deadlock reconnects and every later subscribe.
    let renamed_swarm_id = {
        let mut members = swarm_members.write().await;
        let renamed_swarm_id = members.remove(old_session_id).and_then(|mut member| {
            let swarm_id = member.swarm_id.clone();
            member.session_id = new_session_id.to_string();
            member.status = "ready".to_string();
            member.detail = None;
            members.insert(new_session_id.to_string(), member);
            swarm_id
        });

        // Keep the spawn tree intact across the rename: children that reported
        // back to the old session id must follow it.
        for member in members.values_mut() {
            if member.report_back_to_session_id.as_deref() == Some(old_session_id) {
                member.report_back_to_session_id = Some(new_session_id.to_string());
            }
        }
        renamed_swarm_id
    };

    if let Some(swarm_id) = renamed_swarm_id {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(&swarm_id) {
            swarm.remove(old_session_id);
            swarm.insert(new_session_id.to_string());
        }
    }
}

pub(super) async fn handle_reload(
    id: u64,
    force: bool,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    // A non-forced reload (e.g. `jcode server reload`) is a graceful upgrade
    // request: only reload when this server is provably running older code than
    // an available reload candidate. This keeps us from downgrading a newer
    // server (such as a self-dev daemon next to an older release client) and
    // from re-entering the reload-loop family (#277), where a server that merely
    // "differs" can never make the difference go away by reloading.
    if !force && !super::server_has_newer_binary() {
        crate::logging::info(&format!(
            "handle_reload: skipping non-forced reload for client_session_id={} (no strictly-newer binary)",
            client_session_id
        ));
        // Tell the requester this was a deliberate no-op (not a silent success)
        // so callers like `jcode server reload` can report "already up to date"
        // distinctly from an actual reload.
        let _ = client_event_tx.send(ServerEvent::ReloadProgress {
            step: "skip".to_string(),
            message: "Server already running the newest binary; no reload needed.".to_string(),
            success: Some(true),
            output: None,
        });
        let _ = client_event_tx.send(ServerEvent::Done { id });
        return;
    }

    let request_id = crate::id::new_id("reload");
    mark_remote_reload_started(&request_id);

    let (triggering_session, prefer_selfdev_binary) = match agent.try_lock() {
        Ok(agent_guard) => (
            Some(agent_guard.session_id().to_string()),
            agent_guard.is_canary(),
        ),
        Err(_) => {
            crate::logging::warn(&format!(
                "SERVER_RELOAD_AGENT_BUSY request_id={} client_session_id={} fallback_triggering_session={} prefer_selfdev_binary=false",
                request_id, client_session_id, client_session_id
            ));
            (Some(client_session_id.to_string()), false)
        }
    };

    let live_sessions = {
        let members = swarm_members.read().await;
        members
            .iter()
            .filter_map(|(session_id, member)| {
                if member.event_txs.is_empty() {
                    None
                } else {
                    Some(session_id.clone())
                }
            })
            .collect::<Vec<_>>()
    };

    let mut delivered = 0;
    for session_id in &live_sessions {
        delivered += fanout_live_client_event(
            swarm_members,
            session_id,
            ServerEvent::Reloading { new_socket: None },
        )
        .await;
    }
    if delivered == 0 {
        let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });
    }

    let hash = jcode_build_meta::git_hash().to_string();
    let signal_request_id =
        crate::server::send_reload_signal(hash, triggering_session.clone(), prefer_selfdev_binary);

    crate::logging::info(&format!(
        "handle_reload: queued reload signal {} from remote client request {} (triggering_session={:?}, prefer_selfdev_binary={}, reload_notified_sessions={}, reload_notified_clients={})",
        signal_request_id,
        request_id,
        triggering_session,
        prefer_selfdev_binary,
        live_sessions.len(),
        delivered
    ));

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
async fn cleanup_detached_source_session_if_unused(
    old_session_id: &str,
    client_connection_id: &str,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    unregister_session_event_sender(swarm_members, old_session_id, client_connection_id).await;

    if !remove_detached_source_if_unclaimed(
        old_session_id,
        client_connection_id,
        source_agent,
        sessions,
        client_connections,
    )
    .await
    {
        return;
    }

    {
        let mut agent_guard = source_agent.lock().await;
        agent_guard.mark_closed();
    }

    {
        let mut signals = shutdown_signals.write().await;
        signals.remove(old_session_id);
    }
    remove_background_tool_signal(old_session_id);
    remove_session_interrupt_queue(soft_interrupt_queues, old_session_id).await;
    remove_session_channel_subscriptions(
        old_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;
    file_touch.clear_session(old_session_id).await;

    let removed_swarm_id = {
        let mut members = swarm_members.write().await;
        members
            .remove(old_session_id)
            .and_then(|member| member.swarm_id)
    };
    if let Some(swarm_id) = removed_swarm_id {
        remove_session_from_swarm(
            old_session_id,
            &swarm_id,
            swarm_members,
            swarms_by_id,
            swarm_coordinators,
            swarm_plans,
        )
        .await;
    }
}

/// Removes a detached source only while holding the same connection-registry
/// write lock used to claim a live resume target. The connection registry is
/// the attachment authority, so the lock order for transitions is always
/// `client_connections` then `sessions`.
async fn remove_detached_source_if_unclaimed(
    old_session_id: &str,
    client_connection_id: &str,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
) -> bool {
    let connections = client_connections.write().await;
    if connections
        .values()
        .any(|info| info.client_id != client_connection_id && info.session_id == old_session_id)
    {
        return false;
    }

    let mut sessions_guard = sessions.write().await;
    let owns_source = sessions_guard
        .get(old_session_id)
        .map(|existing| Arc::ptr_eq(existing, source_agent))
        .unwrap_or(false);
    if owns_source {
        sessions_guard.remove(old_session_id);
    }
    owns_source
}

/// Atomically reserves an existing live target for this connection.
///
/// Reserving under the connection write lock prevents another connection's
/// detached-source cleanup from observing no users after we have selected the
/// target but before our connection record is updated.
async fn claim_live_target_agent(
    session_id: &str,
    client_connection_id: &str,
    client_instance_id: Option<&str>,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
) -> Option<Arc<Mutex<Agent>>> {
    let mut connections = client_connections.write().await;
    let sessions_guard = sessions.read().await;
    let target = sessions_guard
        .get(session_id)
        .filter(|existing| !Arc::ptr_eq(existing, source_agent))
        .cloned()?;

    let info = connections.get_mut(client_connection_id)?;
    info.session_id = session_id.to_string();
    info.client_instance_id = client_instance_id.map(str::to_string);
    info.last_seen = Instant::now();
    Some(target)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_resume_session(
    id: u64,
    session_id: String,
    working_dir_override: Option<&str>,
    client_instance_id: Option<&str>,
    client_has_local_history: bool,
    allow_session_takeover: bool,
    client_selfdev: &mut bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> Result<Arc<Mutex<Agent>>> {
    let resume_start = Instant::now();
    let incoming_client_instance_id = client_instance_id.map(str::to_string);
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "resume_start".to_string()),
            ("request_id", id.to_string()),
            ("source_session_id", client_session_id.clone()),
            ("target_session_id", session_id.clone()),
            ("client_connection_id", client_connection_id.to_string()),
            (
                "client_instance_id",
                incoming_client_instance_id
                    .clone()
                    .unwrap_or_else(|| "none".to_string()),
            ),
            (
                "client_has_local_history",
                client_has_local_history.to_string(),
            ),
            ("allow_takeover", allow_session_takeover.to_string()),
        ],
    );
    let live_target_agent = claim_live_target_agent(
        &session_id,
        client_connection_id,
        incoming_client_instance_id.as_deref(),
        agent,
        sessions,
        client_connections,
    )
    .await;

    if let Some(live_target_agent) = live_target_agent.as_ref() {
        let old_session_id = client_session_id.clone();

        let conflicting_live_client = {
            let connections = client_connections.read().await;
            connections
                .values()
                .find(|info| {
                    info.client_id != client_connection_id && info.session_id == session_id
                })
                .cloned()
        };
        let live_target_busy = live_target_agent.try_lock().is_err();
        crate::logging::info(&format!(
            "Resume attach to existing live session {} from temporary {} on connection {}: live_target_busy={}, conflict_owner={}, conflict_processing={}, allow_takeover={}, local_history={}, incoming_instance={:?}",
            session_id,
            old_session_id,
            client_connection_id,
            live_target_busy,
            conflicting_live_client
                .as_ref()
                .map(|info| info.client_id.as_str())
                .unwrap_or("<none>"),
            conflicting_live_client
                .as_ref()
                .map(|info| info.is_processing)
                .unwrap_or(false),
            allow_session_takeover,
            client_has_local_history,
            incoming_client_instance_id
        ));

        cleanup_detached_source_session_if_unused(
            &old_session_id,
            client_connection_id,
            agent,
            sessions,
            shutdown_signals,
            soft_interrupt_queues,
            client_connections,
            swarm_members,
            swarms_by_id,
            file_touch,
            channel_subscriptions,
            channel_subscriptions_by_session,
            swarm_plans,
            swarm_coordinators,
        )
        .await;

        if let Some(conflict) = conflicting_live_client {
            let incoming_instance_id = incoming_client_instance_id.as_deref();
            let existing_instance_id = conflict.client_instance_id.as_deref();
            let distinct_client_instances = incoming_instance_id
                .zip(existing_instance_id)
                .map(|(incoming, existing)| incoming != existing)
                .unwrap_or(false);
            let can_take_over_live_session =
                allow_session_takeover && client_has_local_history && !distinct_client_instances;

            if can_take_over_live_session {
                let (disconnect_tx, debug_client_id, transferred_processing, transferred_tool_name) = {
                    let mut connections = client_connections.write().await;
                    let removed = connections.remove(&conflict.client_id);
                    if let Some(info) = removed {
                        (
                            Some(info.disconnect_tx),
                            info.debug_client_id,
                            info.is_processing,
                            info.current_tool_name,
                        )
                    } else {
                        (
                            None,
                            conflict.debug_client_id,
                            conflict.is_processing,
                            conflict.current_tool_name,
                        )
                    }
                };
                if transferred_processing {
                    crate::logging::warn(&format!(
                        "Taking over live session {} from {} while old owner reports processing; new connection receives status/tool metadata but not the old processing task handle",
                        session_id, conflict.client_id
                    ));
                } else {
                    crate::logging::info(&format!(
                        "Taking over live session {} from idle owner {}",
                        session_id, conflict.client_id
                    ));
                }

                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(client_connection_id) {
                        info.is_processing = transferred_processing;
                        info.current_tool_name = transferred_tool_name;
                    }
                }

                if let Some(debug_client_id) = debug_client_id.as_deref() {
                    let mut debug_state = client_debug_state.write().await;
                    debug_state.unregister(debug_client_id);
                }

                if let Some(disconnect_tx) = disconnect_tx {
                    let _ = disconnect_tx.send(());
                }
            }
        }

        register_session_event_sender(
            swarm_members,
            &session_id,
            client_connection_id,
            client_event_tx.clone(),
        )
        .await;

        let is_canary = live_target_agent
            .try_lock()
            .ok()
            .map(|agent_guard| agent_guard.is_canary())
            .or_else(|| {
                crate::session::Session::load_startup_stub(&session_id)
                    .ok()
                    .map(|session| session.is_canary)
            })
            .unwrap_or(false);
        if is_canary {
            *client_selfdev = true;
            registry.register_selfdev_tools().await;
        }

        *client_session_id = session_id.clone();

        handle_get_history(
            id,
            &session_id,
            false,
            live_target_agent,
            provider,
            sessions,
            client_connections,
            client_count,
            writer,
            server_name,
            server_icon,
            None,
        )
        .await?;
        let _ = client_event_tx.send(ServerEvent::Done { id });
        // Resolve project-local MCP config against the resumed session's
        // working dir, not the server process cwd (issue #420).
        // Do not block on the agent lock here: the target agent may be busy
        // mid-turn (lock held), and awaiting it would deadlock the resume.
        let mcp_working_dir = working_dir_override.map(PathBuf::from).or_else(|| {
            live_target_agent
                .try_lock()
                .ok()
                .and_then(|agent_guard| agent_guard.working_dir().map(PathBuf::from))
                .or_else(|| {
                    crate::session::Session::load_startup_stub(&session_id)
                        .ok()
                        .and_then(|session| session.working_dir.map(PathBuf::from))
                })
        });
        registry
            .register_mcp_tools_for_dir(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(session_id.clone()),
                mcp_working_dir,
            )
            .await;
        spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(live_target_agent));
        crate::logging::event_info(
            "SESSION_LIFECYCLE",
            vec![
                ("phase", "resume_live_attach_done".to_string()),
                ("request_id", id.to_string()),
                ("old_session_id", old_session_id),
                ("target_session_id", session_id.clone()),
                ("client_connection_id", client_connection_id.to_string()),
                ("live_target_busy", live_target_busy.to_string()),
                ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
            ],
        );
        return Ok(Arc::clone(live_target_agent));
    }

    let conflicting_live_client = {
        let connections = client_connections.read().await;
        connections
            .values()
            .find(|info| info.client_id != client_connection_id && info.session_id == session_id)
            .cloned()
    };

    if let Some(conflict) = conflicting_live_client {
        let incoming_instance_id = incoming_client_instance_id.as_deref();
        let existing_instance_id = conflict.client_instance_id.as_deref();
        let same_client_instance = incoming_instance_id
            .zip(existing_instance_id)
            .map(|(incoming, existing)| incoming == existing)
            .unwrap_or(false);
        let distinct_client_instances = incoming_instance_id
            .zip(existing_instance_id)
            .map(|(incoming, existing)| incoming != existing)
            .unwrap_or(false);
        let can_take_over_live_session = allow_session_takeover
            && (same_client_instance || (client_has_local_history && !distinct_client_instances));

        crate::logging::info(&format!(
            "Resume attach decision for session {} on connection {}: allow_takeover={}, local_history={}, same_client_instance={}, distinct_client_instances={}, incoming_instance={:?}, existing_instance={:?}, existing_owner={}",
            session_id,
            client_connection_id,
            allow_session_takeover,
            client_has_local_history,
            same_client_instance,
            distinct_client_instances,
            incoming_client_instance_id,
            conflict.client_instance_id,
            conflict.client_id,
        ));

        if can_take_over_live_session {
            crate::logging::info(&format!(
                "Taking over live session {} on connection {} by superseding {}",
                session_id, client_connection_id, conflict.client_id
            ));

            let (disconnect_tx, debug_client_id, transferred_processing, transferred_tool_name) = {
                let mut connections = client_connections.write().await;
                let removed = connections.remove(&conflict.client_id);
                if let Some(info) = removed {
                    (
                        Some(info.disconnect_tx),
                        info.debug_client_id,
                        info.is_processing,
                        info.current_tool_name,
                    )
                } else {
                    (
                        None,
                        conflict.debug_client_id,
                        conflict.is_processing,
                        conflict.current_tool_name,
                    )
                }
            };

            {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(client_connection_id) {
                    info.is_processing = transferred_processing;
                    info.current_tool_name = transferred_tool_name;
                }
            }

            if let Some(debug_client_id) = debug_client_id.as_deref() {
                let mut debug_state = client_debug_state.write().await;
                debug_state.unregister(debug_client_id);
            }

            if let Some(disconnect_tx) = disconnect_tx {
                let _ = disconnect_tx.send(());
            }
        } else {
            if allow_session_takeover && distinct_client_instances {
                crate::logging::warn(&format!(
                    "Rejecting reconnect takeover for session {} on connection {} because the incoming client is a different live instance from the current owner; incoming_instance={:?}, existing_instance={:?}, existing live owner is {}",
                    session_id,
                    client_connection_id,
                    incoming_client_instance_id,
                    conflict.client_instance_id,
                    conflict.client_id
                ));
            } else if allow_session_takeover && !client_has_local_history && !same_client_instance {
                crate::logging::warn(&format!(
                    "Rejecting reconnect takeover for session {} on connection {} because the incoming client does not match the existing owner instance and has no local history; incoming_instance={:?}, existing_instance={:?}, existing live owner is {}",
                    session_id,
                    client_connection_id,
                    incoming_client_instance_id,
                    conflict.client_instance_id,
                    conflict.client_id
                ));
            } else {
                crate::logging::warn(&format!(
                    "Rejecting duplicate live attach for session {} on connection {} because {} is already attached",
                    session_id, client_connection_id, conflict.client_id
                ));
            }
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Session '{}' is already live but could not be shared safely with this connection.",
                    session_id
                ),
                retry_after_secs: Some(1),
            });
            crate::logging::event_warn(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "resume_rejected".to_string()),
                    ("request_id", id.to_string()),
                    ("target_session_id", session_id.clone()),
                    ("client_connection_id", client_connection_id.to_string()),
                    ("conflict_client_id", conflict.client_id),
                    ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
                ],
            );
            return Ok(Arc::clone(agent));
        }
    }

    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let (result, is_canary) = {
        let mut agent_guard = agent.lock().await;
        let result =
            agent_guard.restore_session_with_working_dir(&session_id, working_dir_override);
        if *client_selfdev {
            agent_guard.set_canary("self-dev");
        }
        let is_canary = agent_guard.is_canary();
        (result, is_canary)
    };

    let was_interrupted = match &result {
        Ok(status) => {
            let agent_guard = agent.lock().await;
            restored_session_was_interrupted(&session_id, status, &agent_guard)
        }
        Err(_) => false,
    };

    if result.is_ok() && is_canary {
        *client_selfdev = true;
        registry.register_selfdev_tools().await;
    }

    match result {
        Ok(_prev_status) => {
            let old_session_id = client_session_id.clone();
            *client_session_id = session_id.clone();

            {
                let mut sessions_guard = sessions.write().await;
                sessions_guard.remove(&old_session_id);
                sessions_guard.insert(session_id.clone(), Arc::clone(agent));
            }
            crate::runtime_memory_log::emit_event(
                crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                    "session_resumed",
                    "existing_session_attached",
                )
                .with_session_id(session_id.clone())
                .force_attribution(),
            );
            rename_shutdown_signal(shutdown_signals, &old_session_id, &session_id).await;
            rename_session_interrupt_queue(soft_interrupt_queues, &old_session_id, &session_id)
                .await;
            {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(client_connection_id) {
                    info.session_id = session_id.clone();
                    info.client_instance_id = incoming_client_instance_id.clone();
                    info.last_seen = Instant::now();
                }
            }

            rename_swarm_member_session(&old_session_id, &session_id, swarm_members, swarms_by_id)
                .await;
            remove_session_channel_subscriptions(
                &old_session_id,
                channel_subscriptions,
                channel_subscriptions_by_session,
            )
            .await;
            file_touch.clear_session(&old_session_id).await;
            {
                let mut coordinators = swarm_coordinators.write().await;
                for coordinator in coordinators.values_mut() {
                    if *coordinator == old_session_id {
                        *coordinator = session_id.clone();
                    }
                }
            }
            update_member_status(
                &session_id,
                "ready",
                None,
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
            if let Some(swarm_id) = {
                let members = swarm_members.read().await;
                members
                    .get(&session_id)
                    .and_then(|member| member.swarm_id.clone())
            } {
                rename_plan_participant(&swarm_id, &old_session_id, &session_id, swarm_plans).await;
                let swarm_state = SwarmState {
                    members: Arc::clone(swarm_members),
                    swarms_by_id: Arc::clone(swarms_by_id),
                    plans: Arc::clone(swarm_plans),
                    coordinators: Arc::clone(swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
            }

            register_session_event_sender(
                swarm_members,
                &session_id,
                client_connection_id,
                client_event_tx.clone(),
            )
            .await;

            handle_get_history(
                id,
                &session_id,
                false,
                agent,
                provider,
                sessions,
                client_connections,
                client_count,
                writer,
                server_name,
                server_icon,
                Some(was_interrupted),
            )
            .await?;
            let _ = client_event_tx.send(ServerEvent::Done { id });
            // Re-send the swarm plan AFTER the History payload: the client
            // clears its plan snapshot on session change, so without this the
            // plan graph would stay blank until the next plan mutation.
            send_swarm_plan_to_session(&session_id, swarm_members, swarm_plans).await;
            // Resolve project-local MCP config against the restored session's
            // working dir, not the server process cwd (issue #420).
            let mcp_working_dir = {
                let agent_guard = agent.lock().await;
                agent_guard.working_dir().map(PathBuf::from)
            };
            registry
                .register_mcp_tools_for_dir(
                    Some(client_event_tx.clone()),
                    Some(Arc::clone(mcp_pool)),
                    Some(session_id.clone()),
                    mcp_working_dir,
                )
                .await;
            spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(agent));
            crate::logging::event_info(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "resume_restored_done".to_string()),
                    ("request_id", id.to_string()),
                    ("old_session_id", old_session_id),
                    ("target_session_id", session_id.clone()),
                    ("client_connection_id", client_connection_id.to_string()),
                    ("was_interrupted", was_interrupted.to_string()),
                    ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
                ],
            );
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Failed to restore session: {}",
                    crate::util::format_error_chain(&error)
                ),
                retry_after_secs: None,
            });
            crate::logging::event_warn(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "resume_restore_failed".to_string()),
                    ("request_id", id.to_string()),
                    ("target_session_id", session_id),
                    ("client_connection_id", client_connection_id.to_string()),
                    ("error", crate::util::format_error_chain(&error)),
                    ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
                ],
            );
        }
    }

    Ok(Arc::clone(agent))
}

#[cfg(test)]
#[path = "client_session_tests.rs"]
mod tests;

========== crates/jcode-app-core/src/server/runtime.rs (17897 bytes) ==========
use super::client_lifecycle::handle_client;
use super::debug::{ClientConnectionInfo, ClientDebugState, handle_debug_client};
use super::debug_jobs::DebugJob;
use super::util::get_shared_mcp_pool;
use super::{
    AwaitMembersRuntime, FileTouchService, ServerIdentity, SessionInterruptQueues, SharedContext,
    SwarmEvent, SwarmMutationRuntime, SwarmState,
};
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::gateway::GatewayClient;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use crate::transport::{Listener, Stream};
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

/// Owns every connection task spawned by a server runtime.
///
/// Dropping a `JoinHandle` detaches its task, so accepting a connection must not
/// discard the handle. This scope gives the accept loops and their children one
/// cancellation boundary and lets server shutdown wait until all children have
/// observed cancellation and released their resources.
#[derive(Default)]
struct RuntimeTaskScope {
    cancellation: CancellationToken,
    tasks: Mutex<JoinSet<()>>,
}

impl RuntimeTaskScope {
    async fn spawn<F, Fut>(&self, task: F) -> bool
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.cancellation.is_cancelled() {
            return false;
        }

        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            log_task_completion(result);
        }
        if self.cancellation.is_cancelled() {
            return false;
        }

        tasks.spawn(task(self.cancellation.child_token()));
        true
    }

    async fn shutdown(&self) {
        self.cancellation.cancel();
        // Drain the set before awaiting children. An accept task may already be
        // waiting to register a just-accepted connection; leaving the mutex
        // held while joining would deadlock that task. Once cancelled, any
        // late registration observes cancellation and is rejected.
        let mut tasks = {
            let mut owned_tasks = self.tasks.lock().await;
            std::mem::take(&mut *owned_tasks)
        };
        while let Some(result) = tasks.join_next().await {
            log_task_completion(result);
        }
    }

    #[cfg(test)]
    async fn task_count(&self) -> usize {
        self.tasks.lock().await.len()
    }
}

fn log_task_completion(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result
        && !error.is_cancelled()
    {
        crate::logging::error(&format!("Server connection task failed: {error}"));
    }
}

#[derive(Clone)]
pub(super) struct ServerRuntime {
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    event_tx: broadcast::Sender<ServerEvent>,
    provider: Arc<dyn Provider>,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
    client_count: Arc<RwLock<usize>>,
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_state: SwarmState,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    file_touch: FileTouchService,
    channel_subscriptions: ChannelSubscriptions,
    channel_subscriptions_by_session: ChannelSubscriptions,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: Arc<AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    server_name: String,
    server_icon: String,
    server_identity: ServerIdentity,
    ambient_runner: Option<AmbientRunnerHandle>,
    mcp_pool: Arc<OnceCell<Arc<crate::mcp::SharedMcpPool>>>,
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: SessionInterruptQueues,
    await_members_runtime: AwaitMembersRuntime,
    swarm_mutation_runtime: SwarmMutationRuntime,
    tasks: Arc<RuntimeTaskScope>,
}

impl ServerRuntime {
    pub(super) fn from_server(server: &super::Server) -> Self {
        Self {
            sessions: Arc::clone(&server.sessions),
            event_tx: server.event_tx.clone(),
            provider: Arc::clone(&server.provider),
            is_processing: Arc::clone(&server.is_processing),
            session_id: Arc::clone(&server.session_id),
            client_count: Arc::clone(&server.client_count),
            client_connections: Arc::clone(&server.client_connections),
            swarm_state: server.swarm_state.clone(),
            shared_context: Arc::clone(&server.shared_context),
            file_touch: server.file_touch.clone(),
            channel_subscriptions: Arc::clone(&server.channel_subscriptions),
            channel_subscriptions_by_session: Arc::clone(&server.channel_subscriptions_by_session),
            client_debug_state: Arc::clone(&server.client_debug_state),
            client_debug_response_tx: server.client_debug_response_tx.clone(),
            debug_jobs: Arc::clone(&server.debug_jobs),
            event_history: Arc::clone(&server.event_history),
            event_counter: Arc::clone(&server.event_counter),
            swarm_event_tx: server.swarm_event_tx.clone(),
            server_name: server.identity.name.clone(),
            server_icon: server.identity.icon.clone(),
            server_identity: server.identity.clone(),
            ambient_runner: server.ambient_runner.clone(),
            mcp_pool: Arc::clone(&server.mcp_pool),
            shutdown_signals: Arc::clone(&server.shutdown_signals),
            soft_interrupt_queues: Arc::clone(&server.soft_interrupt_queues),
            await_members_runtime: server.await_members_runtime.clone(),
            swarm_mutation_runtime: server.swarm_mutation_runtime.clone(),
            tasks: Arc::new(RuntimeTaskScope::default()),
        }
    }

    pub(super) fn spawn_main_accept_loop(&self, listener: Listener) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        let cancellation = self.tasks.cancellation.child_token();
        tokio::spawn(async move {
            #[cfg(windows)]
            let mut listener = listener;

            loop {
                let accepted = tokio::select! {
                    _ = cancellation.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                match accepted {
                    Ok((stream, _)) => {
                        runtime.increment_client_count().await;
                        if !runtime
                            .spawn_client_task(stream, "Client error", true)
                            .await
                        {
                            runtime.decrement_client_count().await;
                            break;
                        }
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Main accept error: {}", e));
                    }
                }
            }
        })
    }

    pub(super) fn spawn_debug_accept_loop(
        &self,
        listener: Listener,
        server_start_time: Instant,
    ) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        let cancellation = self.tasks.cancellation.child_token();
        tokio::spawn(async move {
            #[cfg(windows)]
            let mut listener = listener;

            loop {
                let accepted = tokio::select! {
                    _ = cancellation.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                match accepted {
                    Ok((stream, _)) => {
                        // Debug clients do not participate in idle-timeout accounting.
                        if !runtime
                            .spawn_debug_client_task(stream, server_start_time)
                            .await
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Debug accept error: {}", e));
                    }
                }
            }
        })
    }

    pub(super) async fn spawn_gateway_accept_loop(
        &self,
        mut client_rx: mpsc::UnboundedReceiver<GatewayClient>,
    ) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                loop {
                    let gw_client = tokio::select! {
                        _ = cancellation.cancelled() => break,
                        client = client_rx.recv() => match client {
                            Some(client) => client,
                            None => break,
                        },
                    };
                    runtime.increment_client_count().await;
                    crate::logging::info(&format!(
                        "Gateway client connected: {} ({})",
                        gw_client.device_name, gw_client.device_id
                    ));
                    // Preserve prior behavior: gateway sessions do not nudge the
                    // ambient runner on disconnect.
                    if !runtime.spawn_gateway_client_task(gw_client).await {
                        runtime.decrement_client_count().await;
                        break;
                    }
                }
            })
            .await
    }

    pub(super) async fn spawn_background_task<Fut>(&self, task: Fut) -> bool
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.tasks
            .spawn(move |cancellation| async move {
                tokio::select! {
                    _ = cancellation.cancelled() => {}
                    _ = task => {}
                }
            })
            .await
    }

    async fn spawn_client_task(
        &self,
        stream: Stream,
        error_prefix: &'static str,
        nudge_ambient: bool,
    ) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                runtime
                    .run_client_stream(stream, error_prefix, nudge_ambient, cancellation)
                    .await;
            })
            .await
    }

    async fn spawn_gateway_client_task(&self, gw_client: GatewayClient) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                runtime
                    .run_client_stream(
                        gw_client.stream,
                        "Gateway client error",
                        false,
                        cancellation,
                    )
                    .await;
            })
            .await
    }

    async fn spawn_debug_client_task(&self, stream: Stream, server_start_time: Instant) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                runtime
                    .run_debug_stream(stream, server_start_time, cancellation)
                    .await;
            })
            .await
    }

    pub(super) async fn shutdown(&self) {
        self.tasks.shutdown().await;
    }

    async fn increment_client_count(&self) {
        *self.client_count.write().await += 1;
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "client_connected",
                "client_count_incremented",
            ),
        );
    }

    async fn decrement_client_count(&self) {
        *self.client_count.write().await -= 1;
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "client_disconnected",
                "client_count_decremented",
            ),
        );
    }

    async fn run_client_stream(
        self,
        stream: Stream,
        error_prefix: &'static str,
        nudge_ambient: bool,
        cancellation: CancellationToken,
    ) {
        let result = {
            let client = async {
                let mcp_pool = get_shared_mcp_pool(&self.mcp_pool).await;
                handle_client(
                    stream,
                    Arc::clone(&self.sessions),
                    self.event_tx.clone(),
                    Arc::clone(&self.provider),
                    Arc::clone(&self.is_processing),
                    Arc::clone(&self.session_id),
                    Arc::clone(&self.client_count),
                    Arc::clone(&self.client_connections),
                    Arc::clone(&self.swarm_state.members),
                    Arc::clone(&self.swarm_state.swarms_by_id),
                    Arc::clone(&self.shared_context),
                    Arc::clone(&self.swarm_state.plans),
                    Arc::clone(&self.swarm_state.coordinators),
                    self.file_touch.clone(),
                    Arc::clone(&self.channel_subscriptions),
                    Arc::clone(&self.channel_subscriptions_by_session),
                    Arc::clone(&self.client_debug_state),
                    self.client_debug_response_tx.clone(),
                    Arc::clone(&self.event_history),
                    Arc::clone(&self.event_counter),
                    self.swarm_event_tx.clone(),
                    self.server_name.clone(),
                    self.server_icon.clone(),
                    mcp_pool,
                    Arc::clone(&self.shutdown_signals),
                    Arc::clone(&self.soft_interrupt_queues),
                    self.await_members_runtime.clone(),
                    self.swarm_mutation_runtime.clone(),
                )
                .await
            };
            tokio::pin!(client);
            tokio::select! {
                result = &mut client => Some(result),
                _ = cancellation.cancelled() => None,
            }
        };

        self.decrement_client_count().await;

        if nudge_ambient && let Some(ref runner) = self.ambient_runner {
            runner.nudge();
        }

        if let Some(Err(e)) = result {
            crate::logging::error(&format!("{}: {}", error_prefix, e));
        }
    }

    async fn run_debug_stream(
        self,
        stream: Stream,
        server_start_time: Instant,
        cancellation: CancellationToken,
    ) {
        let client = async {
            let mcp_pool = Some(get_shared_mcp_pool(&self.mcp_pool).await);
            handle_debug_client(
                stream,
                Arc::clone(&self.sessions),
                Arc::clone(&self.is_processing),
                Arc::clone(&self.session_id),
                Arc::clone(&self.provider),
                Arc::clone(&self.client_connections),
                Arc::clone(&self.swarm_state.members),
                Arc::clone(&self.swarm_state.swarms_by_id),
                Arc::clone(&self.shared_context),
                Arc::clone(&self.swarm_state.plans),
                Arc::clone(&self.swarm_state.coordinators),
                self.file_touch.clone(),
                Arc::clone(&self.channel_subscriptions),
                Arc::clone(&self.channel_subscriptions_by_session),
                Arc::clone(&self.client_debug_state),
                self.client_debug_response_tx.clone(),
                Arc::clone(&self.debug_jobs),
                Arc::clone(&self.event_history),
                Arc::clone(&self.event_counter),
                self.swarm_event_tx.clone(),
                self.server_identity.clone(),
                server_start_time,
                self.ambient_runner.clone(),
                mcp_pool,
                Arc::clone(&self.shutdown_signals),
                Arc::clone(&self.soft_interrupt_queues),
            )
            .await
        };
        tokio::pin!(client);
        if let Some(Err(e)) = tokio::select! {
            result = &mut client => Some(result),
            _ = cancellation.cancelled() => None,
        } {
            crate::logging::error(&format!("Debug client error: {}", e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeTaskScope;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn runtime_task_scope_cancels_and_joins_owned_tasks() {
        let scope = RuntimeTaskScope::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);

        assert!(
            scope
                .spawn(move |cancellation| async move {
                    let _drop_flag = DropFlag(task_dropped);
                    cancellation.cancelled().await;
                })
                .await
        );
        assert_eq!(scope.task_count().await, 1);

        tokio::time::timeout(Duration::from_secs(1), scope.shutdown())
            .await
            .expect("runtime task scope should join cancelled tasks");

        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(scope.task_count().await, 0);
        assert!(
            !scope
                .spawn(|_| async { panic!("task spawned after shutdown") })
                .await
        );
    }
}


