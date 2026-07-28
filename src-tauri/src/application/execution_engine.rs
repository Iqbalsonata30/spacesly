//! Backend-owned FIFO Scheduler and fixed-size reusable Worker Pool.
//!
//! `ExecutionEngine` owns one Scheduler thread. `SchedulerStore` owns durable queue, session, and
//! assignment state; the Scheduler owns process-local Worker and cancellation handles. Five Worker
//! threads are created once, execute one mock Task Session at a time, reset their task-local
//! context, and return to idle until the engine is dropped.

use crate::domain::task_session::{
    TaskCapabilityGrant, TaskProgress, TaskRequest, TaskSessionEnvelope, TaskSessionEvent,
    TaskSessionEventInput, TaskSessionEventKind, TaskSessionId, TaskSessionSnapshot,
    TaskSessionState, TaskSessionUpdate,
};
use crate::infrastructure::mcp::mcp_connector_binding_digest;
use crate::infrastructure::scheduler_store::{
    AssignmentFence, DurableAssignment, DurableOutcome, ExternalAssignmentAuthority, FinishResult,
    SchedulerStore,
};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Fixed number of long-lived workers owned by one execution engine.
pub const MAX_EXECUTION_WORKERS: usize = 5;
const ASSIGNMENT_LEASE_DURATION: Duration = Duration::from_secs(30);
const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(10);

/// Observable lifecycle state of one reusable worker slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerState {
    /// Worker is healthy and waiting for an assignment.
    Idle,
    /// Worker owns exactly one running Task Session.
    Running,
    /// Worker thread has terminated and accepts no assignments.
    Stopped,
}

/// Read-only projection of a worker owned by the scheduler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerSnapshot {
    /// Stable one-based worker slot identifier.
    pub id: usize,
    /// Current worker lifecycle state.
    pub state: WorkerState,
    /// Session currently assigned to this worker.
    pub session_id: Option<TaskSessionId>,
    /// Number of sessions completed by this long-lived worker.
    pub completed_sessions: u64,
}

/// Cooperative cancellation handle scoped to one task assignment.
#[derive(Clone, Default)]
pub struct TaskCancellation {
    cancelled: Arc<AtomicBool>,
}

impl TaskCancellation {
    /// Returns true after cancellation has been requested for the assignment.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Returns the shared cancellation flag expected by provider runtime adapters.
    pub fn shared_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Immutable context supplied to the mock task executor.
pub struct TaskExecutionContext {
    session_id: TaskSessionId,
    worker_id: usize,
    attempt_id: u64,
    fencing_token: u64,
    request: TaskRequest,
    grants: Vec<TaskCapabilityGrant>,
    cancellation: TaskCancellation,
    event_sink: TaskEventSink,
}

impl TaskExecutionContext {
    /// Returns the task session being executed.
    pub fn session_id(&self) -> TaskSessionId {
        self.session_id
    }

    /// Returns the stable worker slot executing this assignment.
    pub fn worker_id(&self) -> usize {
        self.worker_id
    }

    /// Returns the durable assignment attempt identifier.
    pub fn attempt_id(&self) -> u64 {
        self.attempt_id
    }

    /// Returns the token required to fence stale assignment completion.
    pub fn fencing_token(&self) -> u64 {
        self.fencing_token
    }

    /// Returns the immutable request submitted for this session.
    pub fn request(&self) -> &TaskRequest {
        &self.request
    }

    /// Returns the cooperative cancellation handle for this assignment.
    pub fn cancellation(&self) -> &TaskCancellation {
        &self.cancellation
    }

    /// Returns durable grants snapshotted for this assignment.
    pub fn capability_grants(&self) -> &[TaskCapabilityGrant] {
        &self.grants
    }

    /// Returns true when this assignment has an explicit durable capability grant.
    pub fn has_capability(&self, capability: &str) -> bool {
        self.grants
            .iter()
            .any(|grant| grant.capability == capability)
    }

    /// Fails unless this exact attempt still owns an unexpired running assignment.
    pub fn ensure_current(&self) -> Result<(), TaskExecutionError> {
        self.event_sink.ensure_current()
    }

    /// Authorizes one capability for this exact current assignment attempt.
    pub fn authorize_capability(&self, capability: &str) -> Result<(), TaskExecutionError> {
        self.ensure_current()?;
        if self.has_capability(capability) {
            Ok(())
        } else {
            Err(TaskExecutionError::new(format!(
                "Task assignment lacks capability '{capability}'."
            )))
        }
    }

    /// Creates a non-secret descriptor for a subprocess pre-side-effect authority check.
    pub fn external_authority(
        &self,
        connector_id: &str,
        connector_command: &[String],
        connector_environment: &HashMap<String, String>,
    ) -> Result<ExternalAssignmentAuthority, TaskExecutionError> {
        let capability = format!("external_tools:{}", connector_id.trim());
        self.authorize_capability(&capability)?;
        let connector_binding =
            mcp_connector_binding_digest(connector_id, connector_command, connector_environment)
                .map_err(TaskExecutionError::new)?;
        self.event_sink
            .store
            .external_authority(
                self.event_sink.fence,
                &capability,
                connector_id,
                &connector_binding,
            )
            .map_err(TaskExecutionError::new)
    }

    /// Returns an attempt-unique identity suitable for runtime and conversation isolation.
    pub fn runtime_attempt_id(&self) -> String {
        format!(
            "task-{}-{}-attempt-{}-fence-{}",
            self.event_sink.store.instance_id(),
            self.session_id.0,
            self.attempt_id,
            self.fencing_token
        )
    }

    /// Appends a structured event using this assignment's durable fencing token.
    pub fn emit_event(
        &self,
        kind: TaskSessionEventKind,
        payload: serde_json::Value,
    ) -> Result<TaskSessionEvent, TaskExecutionError> {
        self.event_reporter().emit_event(kind, payload)
    }

    /// Appends a progress event and atomically updates the session progress projection.
    pub fn report_progress(
        &self,
        progress: TaskProgress,
        payload: serde_json::Value,
    ) -> Result<TaskSessionEvent, TaskExecutionError> {
        self.event_reporter().report_progress(progress, payload)
    }

    /// Returns a cloneable assignment-fenced reporter for synchronous runtime callbacks.
    pub fn event_reporter(&self) -> TaskEventReporter {
        TaskEventReporter {
            event_sink: self.event_sink.clone(),
        }
    }
}

/// Cloneable journal writer scoped to one current assignment attempt.
#[derive(Clone)]
pub struct TaskEventReporter {
    event_sink: TaskEventSink,
}

impl TaskEventReporter {
    pub fn emit_event(
        &self,
        kind: TaskSessionEventKind,
        payload: serde_json::Value,
    ) -> Result<TaskSessionEvent, TaskExecutionError> {
        if matches!(
            kind,
            TaskSessionEventKind::Lifecycle | TaskSessionEventKind::Progress
        ) {
            return Err(TaskExecutionError::new(
                "Use report_progress for progress; lifecycle events are Scheduler-owned.",
            ));
        }
        self.event_sink.emit(TaskSessionEventInput {
            kind,
            payload,
            progress: None,
        })
    }

    pub fn report_progress(
        &self,
        progress: TaskProgress,
        payload: serde_json::Value,
    ) -> Result<TaskSessionEvent, TaskExecutionError> {
        self.event_sink.emit(TaskSessionEventInput {
            kind: TaskSessionEventKind::Progress,
            payload,
            progress: Some(progress),
        })
    }
}

#[derive(Clone)]
struct TaskEventSink {
    store: SchedulerStore,
    fence: AssignmentFence,
    notifier: Arc<TaskSessionNotifier>,
}

impl TaskEventSink {
    fn emit(&self, input: TaskSessionEventInput) -> Result<TaskSessionEvent, TaskExecutionError> {
        let event = self
            .store
            .append_assignment_event(self.fence, input)
            .map_err(TaskExecutionError::new)?;
        self.notifier.publish(TaskSessionUpdate {
            session_id: event.session_id,
            latest_sequence: event.sequence,
        });
        Ok(event)
    }

    fn ensure_current(&self) -> Result<(), TaskExecutionError> {
        match self.store.assignment_is_current(self.fence) {
            Ok(true) => Ok(()),
            Ok(false) => Err(TaskExecutionError::new(
                "Task assignment authority is stale, cancelled, or expired.",
            )),
            Err(error) => Err(TaskExecutionError::new(error)),
        }
    }
}

#[derive(Default)]
struct TaskSessionNotifier {
    state: Mutex<TaskSessionNotifierState>,
}

#[derive(Default)]
struct TaskSessionNotifierState {
    subscribers: Vec<mpsc::Sender<TaskSessionUpdate>>,
    latest_by_session: HashMap<TaskSessionId, u64>,
}

impl TaskSessionNotifier {
    fn subscribe(&self) -> mpsc::Receiver<TaskSessionUpdate> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut state) = self.state.lock() {
            state.subscribers.push(sender);
        }
        receiver
    }

    fn publish(&self, update: TaskSessionUpdate) {
        if let Ok(mut state) = self.state.lock() {
            let latest = state
                .latest_by_session
                .entry(update.session_id)
                .or_default();
            if update.latest_sequence <= *latest {
                return;
            }
            *latest = update.latest_sequence;
            state
                .subscribers
                .retain(|subscriber| subscriber.send(update).is_ok());
        }
    }
}

/// Failure returned by a task executor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskExecutionError {
    message: String,
    disposition: TaskExecutionDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskExecutionDisposition {
    Failed,
    Blocked,
}

impl TaskExecutionError {
    /// Creates an execution failure with a human-readable description.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            disposition: TaskExecutionDisposition::Failed,
        }
    }

    /// Creates an operator-actionable blocked outcome.
    pub fn blocked(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            disposition: TaskExecutionDisposition::Blocked,
        }
    }

    /// Returns the failure description.
    pub fn message(&self) -> &str {
        &self.message
    }

    fn is_blocked(&self) -> bool {
        self.disposition == TaskExecutionDisposition::Blocked
    }
}

impl Display for TaskExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TaskExecutionError {}

/// Execution boundary used by reusable workers.
///
/// The first implementation is intentionally a mock. A future Agent adapter can implement this
/// trait without changing Scheduler or Worker Pool ownership.
pub trait TaskExecutor: Send + Sync + 'static {
    /// Executes one assignment and must periodically observe `context.cancellation()`.
    fn execute(&self, context: &TaskExecutionContext) -> Result<(), TaskExecutionError>;
}

/// Configurable mock executor used while the real Agent Runtime remains out of scope.
pub struct MockTaskExecutor {
    execution: Arc<
        dyn Fn(&TaskExecutionContext) -> Result<(), TaskExecutionError> + Send + Sync + 'static,
    >,
}

impl MockTaskExecutor {
    /// Creates a mock executor from a thread-safe execution function.
    pub fn new<F>(execution: F) -> Self
    where
        F: Fn(&TaskExecutionContext) -> Result<(), TaskExecutionError> + Send + Sync + 'static,
    {
        Self {
            execution: Arc::new(execution),
        }
    }

    /// Creates a successful cooperative mock with a fixed execution duration.
    pub fn succeeding(duration: Duration) -> Self {
        Self::new(move |context| {
            let deadline = Instant::now() + duration;
            while Instant::now() < deadline {
                if context.cancellation().is_cancelled() {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        })
    }
}

impl TaskExecutor for MockTaskExecutor {
    fn execute(&self, context: &TaskExecutionContext) -> Result<(), TaskExecutionError> {
        (self.execution)(context)
    }
}

/// Error returned by the scheduler-facing execution engine API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionEngineError {
    /// The scheduler thread is no longer available.
    SchedulerUnavailable,
    /// The requested task session does not exist.
    SessionNotFound(TaskSessionId),
    /// Only terminal sessions may be removed.
    SessionNotTerminal(TaskSessionId),
    /// The caller timed out waiting for a terminal session state.
    WaitTimeout(TaskSessionId),
    /// The submitted request failed validation.
    InvalidRequest(String),
    /// Durable Scheduler storage could not complete an operation.
    Persistence(String),
}

impl Display for ExecutionEngineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchedulerUnavailable => {
                formatter.write_str("Execution scheduler is unavailable.")
            }
            Self::SessionNotFound(id) => write!(formatter, "Task session {} was not found.", id.0),
            Self::SessionNotTerminal(id) => {
                write!(formatter, "Task session {} is not terminal.", id.0)
            }
            Self::WaitTimeout(id) => {
                write!(formatter, "Timed out waiting for task session {}.", id.0)
            }
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::Persistence(message) => {
                write!(formatter, "Scheduler persistence failed: {message}")
            }
        }
    }
}

impl std::error::Error for ExecutionEngineError {}

/// Backend-owned execution engine containing one Scheduler and five long-lived Workers.
///
/// Dropping the engine requests cooperative cancellation, shuts down every Worker, and joins the
/// Scheduler thread. The current mock boundary must cooperate with cancellation to guarantee a
/// bounded shutdown.
pub struct ExecutionEngine {
    sender: mpsc::Sender<SchedulerMessage>,
    notifier: Arc<TaskSessionNotifier>,
    scheduler: Option<JoinHandle<()>>,
}

impl ExecutionEngine {
    /// Starts an isolated in-memory Scheduler and exactly five reusable Workers.
    pub fn new(executor: MockTaskExecutor) -> Result<Self, ExecutionEngineError> {
        Self::new_with_executor(Arc::new(executor))
    }

    /// Starts an isolated Scheduler with a production-capable executor boundary.
    pub fn new_with_executor(
        executor: Arc<dyn TaskExecutor>,
    ) -> Result<Self, ExecutionEngineError> {
        let store = SchedulerStore::open_in_memory().map_err(ExecutionEngineError::Persistence)?;
        Self::with_store(executor, store)
    }

    /// Opens the persistent Scheduler database and starts exactly five reusable Workers.
    pub fn open_persistent(executor: MockTaskExecutor) -> Result<Self, ExecutionEngineError> {
        Self::open_persistent_with_executor(Arc::new(executor))
    }

    /// Opens the persistent Scheduler with a production-capable executor boundary.
    pub fn open_persistent_with_executor(
        executor: Arc<dyn TaskExecutor>,
    ) -> Result<Self, ExecutionEngineError> {
        let store = SchedulerStore::open().map_err(ExecutionEngineError::Persistence)?;
        Self::with_store(executor, store)
    }

    /// Opens a persistent Scheduler database at an explicit path.
    pub fn open_persistent_at(
        executor: MockTaskExecutor,
        path: PathBuf,
    ) -> Result<Self, ExecutionEngineError> {
        Self::open_persistent_at_with_executor(Arc::new(executor), path)
    }

    /// Opens a persistent Scheduler at an explicit path with a generic executor.
    pub fn open_persistent_at_with_executor(
        executor: Arc<dyn TaskExecutor>,
        path: PathBuf,
    ) -> Result<Self, ExecutionEngineError> {
        let store = SchedulerStore::open_at(path).map_err(ExecutionEngineError::Persistence)?;
        Self::with_store(executor, store)
    }

    fn with_store(
        executor: Arc<dyn TaskExecutor>,
        store: SchedulerStore,
    ) -> Result<Self, ExecutionEngineError> {
        let (sender, receiver) = mpsc::channel();
        let (startup, startup_result) = mpsc::channel();
        let scheduler_sender = sender.clone();
        let notifier = Arc::new(TaskSessionNotifier::default());
        let scheduler_notifier = notifier.clone();
        let scheduler = thread::Builder::new()
            .name("spacesly-execution-scheduler".to_string())
            .spawn(move || {
                run_scheduler(
                    receiver,
                    scheduler_sender,
                    executor,
                    store,
                    scheduler_notifier,
                    startup,
                )
            })
            .map_err(|_| ExecutionEngineError::SchedulerUnavailable)?;
        match startup_result.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                notifier,
                scheduler: Some(scheduler),
            }),
            Ok(Err(error)) => {
                let _ = scheduler.join();
                Err(error)
            }
            Err(_) => {
                let _ = scheduler.join();
                Err(ExecutionEngineError::SchedulerUnavailable)
            }
        }
    }

    /// Submits an immutable task request to the FIFO queue and returns its created session.
    pub fn submit(
        &self,
        request: TaskRequest,
    ) -> Result<TaskSessionSnapshot, ExecutionEngineError> {
        if request.label.trim().is_empty() {
            return Err(ExecutionEngineError::InvalidRequest(
                "Task label is required.".to_string(),
            ));
        }
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::Submit { request, reply })?;
        receive(response)?
    }

    /// Submits a validated versioned envelope while preserving the legacy mock request API.
    pub fn submit_envelope(
        &self,
        label: impl Into<String>,
        envelope: &TaskSessionEnvelope,
    ) -> Result<TaskSessionSnapshot, ExecutionEngineError> {
        let request = TaskRequest::from_envelope(label, envelope)
            .map_err(ExecutionEngineError::InvalidRequest)?;
        self.submit(request)
    }

    /// Atomically submits a validated envelope and explicit durable capability grants.
    pub fn submit_envelope_with_grants(
        &self,
        label: impl Into<String>,
        envelope: &TaskSessionEnvelope,
        capabilities: Vec<String>,
        grant_source: impl Into<String>,
    ) -> Result<TaskSessionSnapshot, ExecutionEngineError> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(ExecutionEngineError::InvalidRequest(
                "Task label is required.".to_string(),
            ));
        }
        let requested = match envelope {
            TaskSessionEnvelope::V1(session) => &session.requested_capabilities,
        };
        if let Some(capability) = capabilities
            .iter()
            .find(|capability| !requested.contains(capability))
        {
            return Err(ExecutionEngineError::InvalidRequest(format!(
                "Task capability '{capability}' was not requested by the envelope."
            )));
        }
        let request = TaskRequest::from_envelope(label, envelope)
            .map_err(ExecutionEngineError::InvalidRequest)?;
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::SubmitWithGrants {
            request,
            capabilities,
            grant_source: grant_source.into(),
            reply,
        })?;
        receive(response)?
    }

    /// Subscribes to best-effort post-commit wake-ups; durable replay remains authoritative.
    pub fn subscribe_updates(&self) -> mpsc::Receiver<TaskSessionUpdate> {
        self.notifier.subscribe()
    }

    /// Requests cancellation of a queued or running task session.
    ///
    /// Queued sessions become cancelled immediately. Running sessions transition through
    /// `cancelling` until the cooperative mock executor returns.
    pub fn cancel(&self, id: TaskSessionId) -> Result<bool, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::Cancel { id, reply })?;
        receive(response)?
    }

    /// Returns the current session projection, or `None` after the session is removed.
    pub fn session(
        &self,
        id: TaskSessionId,
    ) -> Result<Option<TaskSessionSnapshot>, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::GetSession { id, reply })?;
        receive(response)?
    }

    /// Returns every session currently owned by the Scheduler, ordered by session ID.
    pub fn sessions(&self) -> Result<Vec<TaskSessionSnapshot>, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::ListSessions { reply })?;
        receive(response)?
    }

    /// Returns all five Worker projections ordered by worker ID.
    pub fn workers(&self) -> Result<Vec<WorkerSnapshot>, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::ListWorkers { reply })?;
        receive(response)
    }

    /// Returns durable Task Session events with sequence greater than the supplied cursor.
    pub fn events_after(
        &self,
        id: TaskSessionId,
        sequence: u64,
    ) -> Result<Vec<TaskSessionEvent>, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::ListEvents {
            id,
            sequence,
            reply,
        })?;
        receive(response)?
    }

    /// Removes a terminal session and its task-local scheduler state.
    pub fn remove_session(&self, id: TaskSessionId) -> Result<bool, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::RemoveSession { id, reply })?;
        receive(response)?
    }

    /// Waits until a session reaches a terminal state or the timeout expires.
    pub fn wait_for_terminal(
        &self,
        id: TaskSessionId,
        timeout: Duration,
    ) -> Result<TaskSessionSnapshot, ExecutionEngineError> {
        let deadline = Instant::now() + timeout;
        loop {
            let session = self
                .session(id)?
                .ok_or(ExecutionEngineError::SessionNotFound(id))?;
            if session.state.is_terminal() {
                return Ok(session);
            }
            if Instant::now() >= deadline {
                return Err(ExecutionEngineError::WaitTimeout(id));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn send(&self, command: SchedulerCommand) -> Result<(), ExecutionEngineError> {
        self.sender
            .send(SchedulerMessage::Command(command))
            .map_err(|_| ExecutionEngineError::SchedulerUnavailable)
    }

    fn shutdown(&mut self) {
        let Some(scheduler) = self.scheduler.take() else {
            return;
        };
        let (reply, response) = mpsc::channel();
        let _ = self
            .sender
            .send(SchedulerMessage::Command(SchedulerCommand::Shutdown {
                reply,
            }));
        let _ = response.recv();
        let _ = scheduler.join();
    }
}

impl Drop for ExecutionEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn receive<T>(response: mpsc::Receiver<T>) -> Result<T, ExecutionEngineError> {
    response
        .recv()
        .map_err(|_| ExecutionEngineError::SchedulerUnavailable)
}

enum SchedulerMessage {
    Command(SchedulerCommand),
    WorkerFinished {
        worker_id: usize,
        fence: AssignmentFence,
        outcome: WorkerOutcome,
    },
}

enum SchedulerCommand {
    Submit {
        request: TaskRequest,
        reply: mpsc::Sender<Result<TaskSessionSnapshot, ExecutionEngineError>>,
    },
    SubmitWithGrants {
        request: TaskRequest,
        capabilities: Vec<String>,
        grant_source: String,
        reply: mpsc::Sender<Result<TaskSessionSnapshot, ExecutionEngineError>>,
    },
    Cancel {
        id: TaskSessionId,
        reply: mpsc::Sender<Result<bool, ExecutionEngineError>>,
    },
    GetSession {
        id: TaskSessionId,
        reply: mpsc::Sender<Result<Option<TaskSessionSnapshot>, ExecutionEngineError>>,
    },
    ListSessions {
        reply: mpsc::Sender<Result<Vec<TaskSessionSnapshot>, ExecutionEngineError>>,
    },
    ListWorkers {
        reply: mpsc::Sender<Vec<WorkerSnapshot>>,
    },
    ListEvents {
        id: TaskSessionId,
        sequence: u64,
        reply: mpsc::Sender<Result<Vec<TaskSessionEvent>, ExecutionEngineError>>,
    },
    RemoveSession {
        id: TaskSessionId,
        reply: mpsc::Sender<Result<bool, ExecutionEngineError>>,
    },
    Shutdown {
        reply: mpsc::Sender<()>,
    },
}

// Process-local state for one durable assignment owned by this Scheduler instance.
struct ActiveAssignment {
    fence: AssignmentFence,
    cancellation: TaskCancellation,
}

// Scheduler-owned channel and join handle for one long-lived Worker.
struct WorkerSlot {
    snapshot: WorkerSnapshot,
    sender: mpsc::Sender<WorkerCommand>,
    handle: Option<JoinHandle<()>>,
}

struct TaskAssignment {
    assignment: DurableAssignment,
    cancellation: TaskCancellation,
    event_sink: TaskEventSink,
}

enum WorkerCommand {
    Execute(TaskAssignment),
    Shutdown,
}

enum WorkerOutcome {
    Succeeded,
    Failed(String),
    Blocked(String),
    Cancelled,
}

// Single coordinator of durable store transitions and process-local Worker lifecycle mutations.
struct Scheduler {
    store: SchedulerStore,
    notifier: Arc<TaskSessionNotifier>,
    owner_id: u64,
    active: HashMap<TaskSessionId, ActiveAssignment>,
    workers: Vec<WorkerSlot>,
    next_lease_renewal: Instant,
}

fn run_scheduler(
    receiver: mpsc::Receiver<SchedulerMessage>,
    sender: mpsc::Sender<SchedulerMessage>,
    executor: Arc<dyn TaskExecutor>,
    store: SchedulerStore,
    notifier: Arc<TaskSessionNotifier>,
    startup: mpsc::Sender<Result<(), ExecutionEngineError>>,
) {
    let owner_id = match store.register_owner() {
        Ok(owner_id) => owner_id,
        Err(error) => {
            let _ = startup.send(Err(ExecutionEngineError::Persistence(error)));
            return;
        }
    };
    if let Err(error) = store.recover_expired() {
        let _ = startup.send(Err(ExecutionEngineError::Persistence(error)));
        return;
    }
    let workers = match start_workers(sender, executor) {
        Ok(workers) => workers,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let mut scheduler = Scheduler {
        store,
        notifier,
        owner_id,
        active: HashMap::new(),
        workers,
        next_lease_renewal: Instant::now() + LEASE_RENEW_INTERVAL,
    };
    let _ = startup.send(Ok(()));
    scheduler.dispatch();

    'scheduler: loop {
        let wait = scheduler
            .next_lease_renewal
            .saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(message) => match message {
                SchedulerMessage::Command(SchedulerCommand::Submit { request, reply }) => {
                    let result = scheduler
                        .store
                        .enqueue(&request)
                        .map_err(ExecutionEngineError::Persistence);
                    if let Ok(snapshot) = &result {
                        scheduler.publish(snapshot);
                    }
                    let _ = reply.send(result);
                }
                SchedulerMessage::Command(SchedulerCommand::SubmitWithGrants {
                    request,
                    capabilities,
                    grant_source,
                    reply,
                }) => {
                    let result = scheduler
                        .store
                        .enqueue_with_grants(&request, &capabilities, &grant_source)
                        .map_err(ExecutionEngineError::Persistence);
                    if let Ok(snapshot) = &result {
                        scheduler.publish(snapshot);
                    }
                    let _ = reply.send(result);
                }
                SchedulerMessage::Command(SchedulerCommand::Cancel { id, reply }) => {
                    let result = scheduler.cancel(id);
                    let _ = reply.send(result);
                }
                SchedulerMessage::Command(SchedulerCommand::GetSession { id, reply }) => {
                    let _ = reply.send(
                        scheduler
                            .store
                            .get_session(id)
                            .map_err(ExecutionEngineError::Persistence),
                    );
                }
                SchedulerMessage::Command(SchedulerCommand::ListSessions { reply }) => {
                    let _ = reply.send(
                        scheduler
                            .store
                            .list_sessions()
                            .map_err(ExecutionEngineError::Persistence),
                    );
                }
                SchedulerMessage::Command(SchedulerCommand::ListWorkers { reply }) => {
                    let _ = reply.send(
                        scheduler
                            .workers
                            .iter()
                            .map(|worker| worker.snapshot.clone())
                            .collect(),
                    );
                }
                SchedulerMessage::Command(SchedulerCommand::ListEvents {
                    id,
                    sequence,
                    reply,
                }) => {
                    let _ = reply.send(
                        scheduler
                            .store
                            .events_after(id, sequence)
                            .map_err(ExecutionEngineError::Persistence),
                    );
                }
                SchedulerMessage::Command(SchedulerCommand::RemoveSession { id, reply }) => {
                    let result = scheduler.remove_session(id);
                    let _ = reply.send(result);
                }
                SchedulerMessage::Command(SchedulerCommand::Shutdown { reply }) => {
                    scheduler.shutdown_workers();
                    let _ = reply.send(());
                    break 'scheduler;
                }
                SchedulerMessage::WorkerFinished {
                    worker_id,
                    fence,
                    outcome,
                } => {
                    scheduler.finish(worker_id, fence, outcome);
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                scheduler.shutdown_workers();
                break;
            }
        }
        if Instant::now() >= scheduler.next_lease_renewal {
            scheduler.renew_leases();
        }
        scheduler.dispatch();
    }
}

impl Scheduler {
    fn publish(&self, snapshot: &TaskSessionSnapshot) {
        self.notifier.publish(TaskSessionUpdate {
            session_id: snapshot.id,
            latest_sequence: snapshot.last_event_sequence,
        });
    }

    fn publish_current(&self, session_id: TaskSessionId) {
        if let Ok(Some(snapshot)) = self.store.get_session(session_id) {
            self.publish(&snapshot);
        }
    }

    fn publish_all_current(&self) {
        if let Ok(sessions) = self.store.list_sessions() {
            for session in sessions {
                self.publish(&session);
            }
        }
    }

    fn dispatch(&mut self) {
        loop {
            let Some(worker_index) = self
                .workers
                .iter()
                .position(|worker| worker.snapshot.state == WorkerState::Idle)
            else {
                return;
            };
            let worker_id = self.workers[worker_index].snapshot.id;
            let claim = self.store.claim_next(
                self.owner_id,
                worker_id,
                ASSIGNMENT_LEASE_DURATION,
                MAX_EXECUTION_WORKERS,
            );
            self.publish_all_current();
            let assignment = match claim {
                Ok(Some(assignment)) => assignment,
                Ok(None) | Err(_) => return,
            };
            let session_id = assignment.fence.session_id;
            let cancellation = TaskCancellation::default();
            let event_sink = TaskEventSink {
                store: self.store.clone(),
                fence: assignment.fence,
                notifier: self.notifier.clone(),
            };
            let assignment = TaskAssignment {
                assignment,
                cancellation: cancellation.clone(),
                event_sink,
            };
            self.active.insert(
                session_id,
                ActiveAssignment {
                    fence: assignment.assignment.fence,
                    cancellation,
                },
            );
            self.workers[worker_index].snapshot.state = WorkerState::Running;
            self.workers[worker_index].snapshot.session_id = Some(session_id);

            if self.workers[worker_index]
                .sender
                .send(WorkerCommand::Execute(assignment))
                .is_err()
            {
                if let Some(active) = self.active.remove(&session_id) {
                    if matches!(
                        self.store.finish(
                            active.fence,
                            DurableOutcome::Failed(
                                "Worker channel closed before dispatch.".to_string(),
                            ),
                        ),
                        Ok(FinishResult::Applied)
                    ) {
                        self.publish_current(session_id);
                    }
                }
                self.workers[worker_index].snapshot.state = WorkerState::Stopped;
                self.workers[worker_index].snapshot.session_id = None;
            }
        }
    }

    fn cancel(&mut self, id: TaskSessionId) -> Result<bool, ExecutionEngineError> {
        let result = self.store.cancel(id).map_err(|error| {
            if error.contains("was not found") {
                ExecutionEngineError::SessionNotFound(id)
            } else {
                ExecutionEngineError::Persistence(error)
            }
        })?;
        if result.snapshot.state == TaskSessionState::Cancelling {
            if let Some(active) = self.active.get(&id) {
                active.cancellation.cancel();
            }
        }
        if result.changed {
            self.publish(&result.snapshot);
        }
        Ok(result.changed)
    }

    fn finish(&mut self, worker_id: usize, fence: AssignmentFence, outcome: WorkerOutcome) {
        let session_id = fence.session_id;
        let Some(worker) = self.workers.iter_mut().find(|worker| {
            worker.snapshot.id == worker_id && worker.snapshot.session_id == Some(session_id)
        }) else {
            return;
        };
        worker.snapshot.state = WorkerState::Idle;
        worker.snapshot.session_id = None;
        worker.snapshot.completed_sessions = worker.snapshot.completed_sessions.saturating_add(1);

        let Some(active) = self.active.remove(&session_id) else {
            return;
        };
        if active.fence != fence {
            return;
        }
        let outcome = match outcome {
            WorkerOutcome::Succeeded => DurableOutcome::Succeeded,
            WorkerOutcome::Failed(error) => DurableOutcome::Failed(error),
            WorkerOutcome::Blocked(error) => DurableOutcome::Blocked(error),
            WorkerOutcome::Cancelled => DurableOutcome::Cancelled,
        };
        if matches!(self.store.finish(fence, outcome), Ok(FinishResult::Applied)) {
            self.publish_current(session_id);
        }
    }

    fn remove_session(&mut self, id: TaskSessionId) -> Result<bool, ExecutionEngineError> {
        let Some(session) = self
            .store
            .get_session(id)
            .map_err(ExecutionEngineError::Persistence)?
        else {
            return Ok(false);
        };
        if !session.state.is_terminal() {
            return Err(ExecutionEngineError::SessionNotTerminal(id));
        }
        self.store
            .remove_terminal(id)
            .map_err(ExecutionEngineError::Persistence)
    }

    fn renew_leases(&mut self) {
        for (session_id, active) in &self.active {
            match self.store.renew(active.fence, ASSIGNMENT_LEASE_DURATION) {
                Ok(true) => {
                    if self
                        .store
                        .get_session(*session_id)
                        .ok()
                        .flatten()
                        .is_some_and(|session| session.state == TaskSessionState::Cancelling)
                    {
                        active.cancellation.cancel();
                    }
                }
                Ok(false) | Err(_) => active.cancellation.cancel(),
            }
        }
        if self
            .store
            .recover_expired()
            .is_ok_and(|recovered| recovered > 0)
        {
            if let Ok(sessions) = self.store.list_sessions() {
                for session in sessions {
                    self.publish(&session);
                }
            }
        }
        self.next_lease_renewal = Instant::now() + LEASE_RENEW_INTERVAL;
    }

    fn shutdown_workers(&mut self) {
        for active in self.active.values() {
            active.cancellation.cancel();
        }
        for worker in &self.workers {
            let _ = worker.sender.send(WorkerCommand::Shutdown);
        }
        for worker in &mut self.workers {
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
            worker.snapshot.state = WorkerState::Stopped;
            worker.snapshot.session_id = None;
        }
        let _ = self.store.abandon_owner(self.owner_id);
        self.active.clear();
    }
}

fn start_workers(
    scheduler: mpsc::Sender<SchedulerMessage>,
    executor: Arc<dyn TaskExecutor>,
) -> Result<Vec<WorkerSlot>, ExecutionEngineError> {
    let mut workers = Vec::with_capacity(MAX_EXECUTION_WORKERS);
    for worker_id in 1..=MAX_EXECUTION_WORKERS {
        let (sender, receiver) = mpsc::channel();
        let worker_scheduler = scheduler.clone();
        let worker_executor = executor.clone();
        let handle = match thread::Builder::new()
            .name(format!("spacesly-execution-worker-{worker_id}"))
            .spawn(move || {
                while let Ok(command) = receiver.recv() {
                    match command {
                        WorkerCommand::Execute(assignment) => {
                            let fence = assignment.assignment.fence;
                            let context = TaskExecutionContext {
                                session_id: fence.session_id,
                                worker_id,
                                attempt_id: fence.attempt_id,
                                fencing_token: fence.fencing_token,
                                request: assignment.assignment.request,
                                grants: assignment.assignment.grants,
                                cancellation: assignment.cancellation.clone(),
                                event_sink: assignment.event_sink,
                            };
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                worker_executor.execute(&context)
                            }));
                            let outcome = if context.cancellation.is_cancelled() {
                                WorkerOutcome::Cancelled
                            } else {
                                match result {
                                    Ok(Ok(())) => WorkerOutcome::Succeeded,
                                    Ok(Err(error)) if error.is_blocked() => {
                                        WorkerOutcome::Blocked(error.message().to_string())
                                    }
                                    Ok(Err(error)) => {
                                        WorkerOutcome::Failed(error.message().to_string())
                                    }
                                    Err(_) => WorkerOutcome::Failed(
                                        "Mock task executor panicked.".to_string(),
                                    ),
                                }
                            };
                            if worker_scheduler
                                .send(SchedulerMessage::WorkerFinished {
                                    worker_id,
                                    fence,
                                    outcome,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        WorkerCommand::Shutdown => break,
                    }
                }
            }) {
            Ok(handle) => handle,
            Err(_) => {
                stop_worker_slots(&mut workers);
                return Err(ExecutionEngineError::SchedulerUnavailable);
            }
        };
        workers.push(WorkerSlot {
            snapshot: WorkerSnapshot {
                id: worker_id,
                state: WorkerState::Idle,
                session_id: None,
                completed_sessions: 0,
            },
            sender,
            handle: Some(handle),
        });
    }
    Ok(workers)
}

fn stop_worker_slots(workers: &mut [WorkerSlot]) {
    for worker in workers.iter() {
        let _ = worker.sender.send(WorkerCommand::Shutdown);
    }
    for worker in workers.iter_mut() {
        if let Some(handle) = worker.handle.take() {
            let _ = handle.join();
        }
        worker.snapshot.state = WorkerState::Stopped;
        worker.snapshot.session_id = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task_session::{TaskSessionEnvelopeV1, TaskSessionKind};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::tempdir;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn pool_never_runs_more_than_five_tasks() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let executor = MockTaskExecutor::new({
            let active = active.clone();
            let maximum = maximum.clone();
            let release = release.clone();
            move |context| {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) && !context.cancellation().is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let engine = ExecutionEngine::new(executor).expect("engine starts");
        let sessions = submit_tasks(&engine, 10, "capacity");

        wait_until(|| maximum.load(Ordering::SeqCst) == 5);
        let snapshots = engine.sessions().expect("sessions listed");
        assert_eq!(
            snapshots
                .iter()
                .filter(|session| session.state == TaskSessionState::Running)
                .count(),
            5
        );
        assert_eq!(
            snapshots
                .iter()
                .filter(|session| session.state == TaskSessionState::Queued)
                .count(),
            5
        );
        assert_eq!(maximum.load(Ordering::SeqCst), MAX_EXECUTION_WORKERS);

        release.store(true, Ordering::SeqCst);
        wait_for_all(&engine, &sessions);
    }

    #[test]
    fn scheduler_assigns_sessions_in_fifo_order() {
        let engine = ExecutionEngine::new(MockTaskExecutor::succeeding(Duration::from_millis(10)))
            .expect("engine starts");
        let sessions = submit_tasks(&engine, 20, "fifo");
        wait_for_all(&engine, &sessions);

        let snapshots = engine.sessions().expect("sessions listed");
        let dispatch_order = snapshots
            .iter()
            .map(|session| session.dispatch_sequence.expect("session dispatched"))
            .collect::<Vec<_>>();
        assert_eq!(dispatch_order, (1..=20).collect::<Vec<_>>());
    }

    #[test]
    fn long_lived_workers_are_reused_across_batches() {
        let engine = ExecutionEngine::new(MockTaskExecutor::succeeding(Duration::from_millis(20)))
            .expect("engine starts");
        let first = submit_tasks(&engine, 5, "first");
        wait_for_all(&engine, &first);
        let first_workers = worker_ids(&engine, &first);

        let second = submit_tasks(&engine, 5, "second");
        wait_for_all(&engine, &second);
        let second_workers = worker_ids(&engine, &second);

        assert_eq!(first_workers, HashSet::from([1, 2, 3, 4, 5]));
        assert_eq!(second_workers, first_workers);
        assert!(engine
            .workers()
            .expect("workers listed")
            .iter()
            .all(|worker| worker.state == WorkerState::Idle && worker.completed_sessions == 2));
    }

    #[test]
    fn each_worker_executes_exactly_one_session_at_a_time() {
        let active_workers = Arc::new(Mutex::new(HashSet::new()));
        let engine = ExecutionEngine::new(MockTaskExecutor::new({
            let active_workers = active_workers.clone();
            move |context| {
                assert!(
                    active_workers
                        .lock()
                        .expect("active workers lock")
                        .insert(context.worker_id()),
                    "worker received overlapping sessions"
                );
                thread::sleep(Duration::from_millis(5));
                assert!(
                    active_workers
                        .lock()
                        .expect("active workers lock")
                        .remove(&context.worker_id()),
                    "worker assignment was not active"
                );
                Ok(())
            }
        }))
        .expect("engine starts");
        let sessions = submit_tasks(&engine, 30, "exclusive-worker");
        wait_for_all(&engine, &sessions);

        assert!(sessions.iter().all(|id| {
            engine
                .session(*id)
                .expect("session read")
                .is_some_and(|session| session.state == TaskSessionState::Succeeded)
        }));
        assert!(active_workers
            .lock()
            .expect("active workers lock")
            .is_empty());
    }

    #[test]
    fn failed_tasks_release_workers_for_later_sessions() {
        let engine = ExecutionEngine::new(MockTaskExecutor::new(|context| {
            if context.request().label.starts_with("fail") {
                return Err(TaskExecutionError::new("expected mock failure"));
            }
            Ok(())
        }))
        .expect("engine starts");
        let failed = submit_tasks(&engine, 5, "fail");
        wait_for_all(&engine, &failed);
        assert!(failed.iter().all(|id| {
            engine
                .session(*id)
                .expect("session read")
                .is_some_and(|session| session.state == TaskSessionState::Failed)
        }));

        let succeeded = submit_tasks(&engine, 5, "success");
        wait_for_all(&engine, &succeeded);
        assert!(succeeded.iter().all(|id| {
            engine
                .session(*id)
                .expect("session read")
                .is_some_and(|session| session.state == TaskSessionState::Succeeded)
        }));
        assert_eq!(
            engine
                .workers()
                .expect("workers listed")
                .iter()
                .map(|worker| worker.completed_sessions)
                .sum::<u64>(),
            10
        );
    }

    #[test]
    fn queued_cancellation_destroys_task_local_state_without_execution() {
        let release = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let engine = ExecutionEngine::new(MockTaskExecutor::new({
            let release = release.clone();
            let observed = observed.clone();
            move |context| {
                observed
                    .lock()
                    .expect("observed lock")
                    .push(context.request().label.clone());
                while !release.load(Ordering::SeqCst) && !context.cancellation().is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(())
            }
        }))
        .expect("engine starts");
        let blockers = submit_tasks(&engine, 5, "blocker");
        wait_until(|| running_count(&engine) == 5);
        let queued = engine
            .submit(TaskRequest::new("cancel-before-dispatch"))
            .expect("task submitted");
        assert!(engine.cancel(queued.id).expect("task cancelled"));
        assert_eq!(
            engine
                .session(queued.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Cancelled
        );

        release.store(true, Ordering::SeqCst);
        wait_for_all(&engine, &blockers);
        assert!(!observed
            .lock()
            .expect("observed lock")
            .iter()
            .any(|label| label == "cancel-before-dispatch"));
    }

    #[test]
    fn running_cancellation_releases_worker_for_next_task() {
        let engine = ExecutionEngine::new(MockTaskExecutor::new(|context| {
            if context.request().label == "cancel-running" {
                while !context.cancellation().is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
            }
            Ok(())
        }))
        .expect("engine starts");
        let cancelled = engine
            .submit(TaskRequest::new("cancel-running"))
            .expect("task submitted");
        wait_until(|| {
            engine
                .session(cancelled.id)
                .expect("session read")
                .is_some_and(|session| session.state == TaskSessionState::Running)
        });
        assert!(engine.cancel(cancelled.id).expect("task cancelled"));
        let cancelled = engine
            .wait_for_terminal(cancelled.id, TEST_TIMEOUT)
            .expect("cancel completes");
        assert_eq!(cancelled.state, TaskSessionState::Cancelled);

        let next = engine
            .submit(TaskRequest::new("after-cancel"))
            .expect("task submitted");
        let next = engine
            .wait_for_terminal(next.id, TEST_TIMEOUT)
            .expect("next task completes");
        assert_eq!(next.state, TaskSessionState::Succeeded);
        assert_eq!(next.worker_id, cancelled.worker_id);
    }

    #[test]
    fn terminal_sessions_can_be_removed_but_running_sessions_cannot() {
        let release = Arc::new(AtomicBool::new(false));
        let engine = ExecutionEngine::new(MockTaskExecutor::new({
            let release = release.clone();
            move |context| {
                while !release.load(Ordering::SeqCst) && !context.cancellation().is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(())
            }
        }))
        .expect("engine starts");
        let session = engine
            .submit(TaskRequest::new("lifecycle"))
            .expect("task submitted");
        wait_until(|| running_count(&engine) == 1);
        assert_eq!(
            engine.remove_session(session.id),
            Err(ExecutionEngineError::SessionNotTerminal(session.id))
        );

        release.store(true, Ordering::SeqCst);
        engine
            .wait_for_terminal(session.id, TEST_TIMEOUT)
            .expect("task completes");
        assert!(engine.remove_session(session.id).expect("session removed"));
        assert_eq!(engine.session(session.id).expect("session read"), None);
    }

    #[test]
    fn executor_panics_fail_only_the_current_task_and_worker_is_reused() {
        let engine = ExecutionEngine::new(MockTaskExecutor::new(|context| {
            if context.request().label == "panic" {
                panic!("expected mock panic");
            }
            Ok(())
        }))
        .expect("engine starts");
        let failed = engine
            .submit(TaskRequest::new("panic"))
            .expect("task submitted");
        assert_eq!(
            engine
                .wait_for_terminal(failed.id, TEST_TIMEOUT)
                .expect("panic task completes")
                .state,
            TaskSessionState::Failed
        );
        let next = engine
            .submit(TaskRequest::new("after-panic"))
            .expect("task submitted");
        assert_eq!(
            engine
                .wait_for_terminal(next.id, TEST_TIMEOUT)
                .expect("next task completes")
                .state,
            TaskSessionState::Succeeded
        );
    }

    #[test]
    fn engine_always_owns_exactly_five_workers() {
        let engine = ExecutionEngine::new(MockTaskExecutor::succeeding(Duration::ZERO))
            .expect("engine starts");
        let workers = engine.workers().expect("workers listed");
        assert_eq!(workers.len(), MAX_EXECUTION_WORKERS);
        assert_eq!(
            workers.iter().map(|worker| worker.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert!(workers
            .iter()
            .all(|worker| worker.state == WorkerState::Idle));
    }

    #[test]
    fn terminal_sessions_survive_persistent_engine_reopen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let session_id = {
            let engine = ExecutionEngine::open_persistent_at(
                MockTaskExecutor::succeeding(Duration::from_millis(5)),
                path.clone(),
            )
            .expect("persistent engine starts");
            let session = engine
                .submit(TaskRequest::new("persisted-terminal"))
                .expect("task submitted");
            engine
                .wait_for_terminal(session.id, TEST_TIMEOUT)
                .expect("task completes");
            session.id
        };

        let reopened =
            ExecutionEngine::open_persistent_at(MockTaskExecutor::succeeding(Duration::ZERO), path)
                .expect("persistent engine reopens");
        let restored = reopened
            .session(session_id)
            .expect("session read")
            .expect("session restored");
        assert_eq!(restored.state, TaskSessionState::Succeeded);
        assert_eq!(restored.attempt, 1);
        assert!(restored.fencing_token > 0);
    }

    #[test]
    fn graceful_restart_requeues_running_sessions_with_new_attempts() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let session_ids = {
            let engine = ExecutionEngine::open_persistent_at(
                MockTaskExecutor::new(|context| {
                    while !context.cancellation().is_cancelled() {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Ok(())
                }),
                path.clone(),
            )
            .expect("persistent engine starts");
            let sessions = submit_tasks(&engine, 7, "restart");
            wait_until(|| running_count(&engine) == 5);
            sessions
        };

        let reopened = ExecutionEngine::open_persistent_at(
            MockTaskExecutor::succeeding(Duration::from_millis(5)),
            path,
        )
        .expect("persistent engine reopens");
        wait_for_all(&reopened, &session_ids);
        let sessions = reopened.sessions().expect("sessions listed");
        assert!(sessions
            .iter()
            .all(|session| session.state == TaskSessionState::Succeeded));
        assert_eq!(
            sessions
                .iter()
                .filter(|session| session.attempt == 2)
                .count(),
            5
        );
        assert_eq!(
            sessions
                .iter()
                .filter(|session| session.attempt == 1)
                .count(),
            2
        );
    }

    #[test]
    fn versioned_envelope_reaches_the_assignment_unchanged() {
        let envelope = TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: Some("card-1".to_string()),
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "digest-1".to_string(),
            runtime_profile_id: "profile-1".to_string(),
            model: "model-1".to_string(),
            connector_ids: vec!["jira".to_string()],
            requested_capabilities: vec![
                "workspace_read".to_string(),
                "external_tools:jira".to_string(),
            ],
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: Some("context-v1".to_string()),
            rules_revision: Some("rules-v1".to_string()),
            skills_revision: Some("skills-v1".to_string()),
        });
        let expected = envelope.clone();
        let engine = ExecutionEngine::new(MockTaskExecutor::new(move |context| {
            assert_eq!(
                context.request().envelope().expect("envelope decoded"),
                Some(expected.clone())
            );
            Ok(())
        }))
        .expect("engine starts");
        let submitted = engine
            .submit_envelope("versioned", &envelope)
            .expect("envelope submitted");
        let completed = engine
            .wait_for_terminal(submitted.id, TEST_TIMEOUT)
            .expect("task completes");
        assert_eq!(
            completed.request.envelope().expect("envelope decoded"),
            Some(envelope)
        );
    }

    #[test]
    fn concurrent_sessions_keep_event_streams_isolated() {
        let engine = ExecutionEngine::new(MockTaskExecutor::new(|context| {
            let label = context.request().label.clone();
            context.emit_event(
                TaskSessionEventKind::Activity,
                serde_json::json!({ "label": label, "worker_id": context.worker_id() }),
            )?;
            thread::sleep(Duration::from_millis(2));
            context.report_progress(
                TaskProgress {
                    phase: "mock_execution".to_string(),
                    completed: 1,
                    total: Some(1),
                },
                serde_json::json!({ "label": context.request().label }),
            )?;
            Ok(())
        }))
        .expect("engine starts");
        let sessions = submit_tasks(&engine, 20, "isolated");
        wait_for_all(&engine, &sessions);

        for session_id in sessions {
            let snapshot = engine
                .session(session_id)
                .expect("session read")
                .expect("session exists");
            let events = engine.events_after(session_id, 0).expect("events read");
            assert_eq!(events.len(), 5);
            assert!(events.iter().all(|event| event.session_id == session_id));
            assert_eq!(
                events
                    .iter()
                    .map(|event| event.sequence)
                    .collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5]
            );
            let activity = events
                .iter()
                .find(|event| event.kind == TaskSessionEventKind::Activity)
                .expect("activity exists");
            assert_eq!(activity.payload["label"], snapshot.request.label);
            assert_eq!(
                snapshot.progress,
                Some(TaskProgress {
                    phase: "succeeded".to_string(),
                    completed: 1,
                    total: Some(1),
                })
            );
        }
    }

    #[test]
    fn durable_grants_reach_only_the_granted_assignment() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let engine = ExecutionEngine::new(MockTaskExecutor::new({
            let observed = observed.clone();
            move |context| {
                context.authorize_capability("workspace_read")?;
                assert!(context.authorize_capability("external_tools:jira").is_err());
                observed.lock().expect("observed lock").push((
                    context.runtime_attempt_id(),
                    context
                        .capability_grants()
                        .iter()
                        .map(|grant| grant.capability.clone())
                        .collect::<Vec<_>>(),
                ));
                Ok(())
            }
        }))
        .expect("engine starts");
        let envelope = test_envelope(vec![
            "workspace_read".to_string(),
            "external_tools:jira".to_string(),
        ]);
        let submitted = engine
            .submit_envelope_with_grants(
                "granted",
                &envelope,
                vec!["workspace_read".to_string()],
                "test-approval",
            )
            .expect("granted task submitted");
        assert_eq!(
            engine
                .wait_for_terminal(submitted.id, TEST_TIMEOUT)
                .expect("task completes")
                .state,
            TaskSessionState::Succeeded
        );
        let observed = observed.lock().expect("observed lock");
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].1, vec!["workspace_read"]);
        assert!(observed[0]
            .0
            .contains(&format!("-{}-attempt-", submitted.id.0)));

        assert!(matches!(
            engine.submit_envelope_with_grants(
                "over-granted",
                &envelope,
                vec!["shell".to_string()],
                "test-approval",
            ),
            Err(ExecutionEngineError::InvalidRequest(_))
        ));
        assert!(matches!(
            engine.submit_envelope_with_grants(
                "   ",
                &envelope,
                vec!["workspace_read".to_string()],
                "test-approval",
            ),
            Err(ExecutionEngineError::InvalidRequest(_))
        ));
    }

    #[test]
    fn cancellation_revokes_assignment_authority_before_executor_returns() {
        let entered = Arc::new(AtomicBool::new(false));
        let authority_revoked = Arc::new(AtomicBool::new(false));
        let engine = ExecutionEngine::new(MockTaskExecutor::new({
            let entered = entered.clone();
            let authority_revoked = authority_revoked.clone();
            move |context| {
                context.ensure_current()?;
                entered.store(true, Ordering::SeqCst);
                while !context.cancellation().is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                authority_revoked.store(context.ensure_current().is_err(), Ordering::SeqCst);
                Ok(())
            }
        }))
        .expect("engine starts");
        let session = engine
            .submit(TaskRequest::new("authority-cancellation"))
            .expect("task submitted");
        wait_until(|| entered.load(Ordering::SeqCst));
        assert!(engine.cancel(session.id).expect("task cancelled"));
        assert_eq!(
            engine
                .wait_for_terminal(session.id, TEST_TIMEOUT)
                .expect("task terminates")
                .state,
            TaskSessionState::Cancelled
        );
        assert!(authority_revoked.load(Ordering::SeqCst));
    }

    #[test]
    fn update_subscription_wakes_after_each_committed_event() {
        let engine = ExecutionEngine::new(MockTaskExecutor::new(|context| {
            context.emit_event(
                TaskSessionEventKind::Activity,
                serde_json::json!({ "message": "working" }),
            )?;
            context.report_progress(
                TaskProgress {
                    phase: "verifying".to_string(),
                    completed: 1,
                    total: Some(1),
                },
                serde_json::json!({ "message": "verified" }),
            )?;
            Ok(())
        }))
        .expect("engine starts");
        let updates = engine.subscribe_updates();
        let session = engine
            .submit(TaskRequest::new("live-progress"))
            .expect("task submitted");
        engine
            .wait_for_terminal(session.id, TEST_TIMEOUT)
            .expect("task completes");

        let received = (0..5)
            .map(|_| {
                updates
                    .recv_timeout(TEST_TIMEOUT)
                    .expect("post-commit update received")
            })
            .collect::<Vec<_>>();
        assert!(received
            .iter()
            .all(|update| update.session_id == session.id));
        assert_eq!(
            received
                .iter()
                .map(|update| update.latest_sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        for update in received {
            assert_eq!(
                engine
                    .events_after(session.id, update.latest_sequence - 1)
                    .expect("journal replayed")[0]
                    .sequence,
                update.latest_sequence
            );
        }
    }

    #[test]
    fn update_notifications_never_move_a_session_cursor_backward() {
        let notifier = TaskSessionNotifier::default();
        let updates = notifier.subscribe();
        let session_id = TaskSessionId(1);
        notifier.publish(TaskSessionUpdate {
            session_id,
            latest_sequence: 2,
        });
        notifier.publish(TaskSessionUpdate {
            session_id,
            latest_sequence: 1,
        });
        assert_eq!(
            updates.recv_timeout(TEST_TIMEOUT).expect("update received"),
            TaskSessionUpdate {
                session_id,
                latest_sequence: 2,
            }
        );
        assert!(matches!(updates.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn runtime_attempt_ids_are_namespaced_per_scheduler_database() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        for label in ["first-database", "second-database"] {
            let engine = ExecutionEngine::new(MockTaskExecutor::new({
                let observed = observed.clone();
                move |context| {
                    observed
                        .lock()
                        .expect("observed lock")
                        .push(context.runtime_attempt_id());
                    Ok(())
                }
            }))
            .expect("engine starts");
            let session = engine
                .submit(TaskRequest::new(label))
                .expect("task submitted");
            engine
                .wait_for_terminal(session.id, TEST_TIMEOUT)
                .expect("task completes");
        }
        let observed = observed.lock().expect("observed lock");
        assert_eq!(observed.len(), 2);
        assert_ne!(observed[0], observed[1]);
    }

    fn submit_tasks(engine: &ExecutionEngine, count: usize, prefix: &str) -> Vec<TaskSessionId> {
        (0..count)
            .map(|index| {
                engine
                    .submit(TaskRequest::new(format!("{prefix}-{index}")))
                    .expect("task submitted")
                    .id
            })
            .collect()
    }

    fn wait_for_all(engine: &ExecutionEngine, sessions: &[TaskSessionId]) {
        for id in sessions {
            engine
                .wait_for_terminal(*id, TEST_TIMEOUT)
                .expect("task reaches terminal state");
        }
    }

    fn running_count(engine: &ExecutionEngine) -> usize {
        engine
            .sessions()
            .expect("sessions listed")
            .iter()
            .filter(|session| session.state == TaskSessionState::Running)
            .count()
    }

    fn worker_ids(engine: &ExecutionEngine, sessions: &[TaskSessionId]) -> HashSet<usize> {
        sessions
            .iter()
            .map(|id| {
                engine
                    .session(*id)
                    .expect("session read")
                    .expect("session exists")
                    .worker_id
                    .expect("worker assigned")
            })
            .collect()
    }

    fn wait_until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while !condition() {
            assert!(Instant::now() < deadline, "condition timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn test_envelope(requested_capabilities: Vec<String>) -> TaskSessionEnvelope {
        let connector_ids = requested_capabilities
            .iter()
            .filter_map(|capability| capability.strip_prefix("external_tools:"))
            .map(str::to_string)
            .collect();
        TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: Some("run-1".to_string()),
            context_digest: "digest-1".to_string(),
            runtime_profile_id: "profile-1".to_string(),
            model: "model-1".to_string(),
            connector_ids,
            requested_capabilities,
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        })
    }
}
