//! Backend-owned FIFO Scheduler and fixed-size reusable Worker Pool.
//!
//! `ExecutionEngine` owns one Scheduler thread. The Scheduler exclusively owns all queue,
//! session, assignment, and Worker lifecycle state. Five Worker threads are created once, execute
//! one mock Task Session at a time, reset their task-local context, and return to idle until the
//! engine is dropped.

use crate::domain::task_session::{
    TaskRequest, TaskSession, TaskSessionId, TaskSessionSnapshot, TaskSessionState,
};
use std::collections::{HashMap, VecDeque};
use std::fmt::{Display, Formatter};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Fixed number of long-lived workers owned by one execution engine.
pub const MAX_EXECUTION_WORKERS: usize = 5;

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

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Immutable context supplied to the mock task executor.
pub struct TaskExecutionContext {
    session_id: TaskSessionId,
    worker_id: usize,
    request: TaskRequest,
    cancellation: TaskCancellation,
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

    /// Returns the immutable request submitted for this session.
    pub fn request(&self) -> &TaskRequest {
        &self.request
    }

    /// Returns the cooperative cancellation handle for this assignment.
    pub fn cancellation(&self) -> &TaskCancellation {
        &self.cancellation
    }
}

/// Failure returned by a task executor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskExecutionError {
    message: String,
}

impl TaskExecutionError {
    /// Creates an execution failure with a human-readable description.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the failure description.
    pub fn message(&self) -> &str {
        &self.message
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
    scheduler: Option<JoinHandle<()>>,
}

impl ExecutionEngine {
    /// Starts a Scheduler and exactly five reusable Workers using the supplied mock executor.
    pub fn new(executor: MockTaskExecutor) -> Result<Self, ExecutionEngineError> {
        let (sender, receiver) = mpsc::channel();
        let (startup, startup_result) = mpsc::channel();
        let scheduler_sender = sender.clone();
        let scheduler = thread::Builder::new()
            .name("spacesly-execution-scheduler".to_string())
            .spawn(move || run_scheduler(receiver, scheduler_sender, Arc::new(executor), startup))
            .map_err(|_| ExecutionEngineError::SchedulerUnavailable)?;
        match startup_result.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
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
        receive(response)
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
        receive(response)
    }

    /// Returns every session currently owned by the Scheduler, ordered by session ID.
    pub fn sessions(&self) -> Result<Vec<TaskSessionSnapshot>, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::ListSessions { reply })?;
        receive(response)
    }

    /// Returns all five Worker projections ordered by worker ID.
    pub fn workers(&self) -> Result<Vec<WorkerSnapshot>, ExecutionEngineError> {
        let (reply, response) = mpsc::channel();
        self.send(SchedulerCommand::ListWorkers { reply })?;
        receive(response)
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
        session_id: TaskSessionId,
        outcome: WorkerOutcome,
    },
}

enum SchedulerCommand {
    Submit {
        request: TaskRequest,
        reply: mpsc::Sender<TaskSessionSnapshot>,
    },
    Cancel {
        id: TaskSessionId,
        reply: mpsc::Sender<Result<bool, ExecutionEngineError>>,
    },
    GetSession {
        id: TaskSessionId,
        reply: mpsc::Sender<Option<TaskSessionSnapshot>>,
    },
    ListSessions {
        reply: mpsc::Sender<Vec<TaskSessionSnapshot>>,
    },
    ListWorkers {
        reply: mpsc::Sender<Vec<WorkerSnapshot>>,
    },
    RemoveSession {
        id: TaskSessionId,
        reply: mpsc::Sender<Result<bool, ExecutionEngineError>>,
    },
    Shutdown {
        reply: mpsc::Sender<()>,
    },
}

// Scheduler-owned state destroyed only after explicit removal of a terminal session.
struct SessionEntry {
    session: TaskSession,
    cancellation: TaskCancellation,
}

// Scheduler-owned channel and join handle for one long-lived Worker.
struct WorkerSlot {
    snapshot: WorkerSnapshot,
    sender: mpsc::Sender<WorkerCommand>,
    handle: Option<JoinHandle<()>>,
}

struct TaskAssignment {
    session_id: TaskSessionId,
    request: TaskRequest,
    cancellation: TaskCancellation,
}

enum WorkerCommand {
    Execute(TaskAssignment),
    Shutdown,
}

enum WorkerOutcome {
    Succeeded,
    Failed(String),
    Cancelled,
}

// Single owner of queue, session, assignment, and Worker lifecycle mutations.
struct Scheduler {
    sessions: HashMap<TaskSessionId, SessionEntry>,
    queue: VecDeque<TaskSessionId>,
    workers: Vec<WorkerSlot>,
    next_session_id: u64,
    next_dispatch_sequence: u64,
}

fn run_scheduler(
    receiver: mpsc::Receiver<SchedulerMessage>,
    sender: mpsc::Sender<SchedulerMessage>,
    executor: Arc<dyn TaskExecutor>,
    startup: mpsc::Sender<Result<(), ExecutionEngineError>>,
) {
    let workers = match start_workers(sender, executor) {
        Ok(workers) => workers,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let mut scheduler = Scheduler {
        sessions: HashMap::new(),
        queue: VecDeque::new(),
        workers,
        next_session_id: 1,
        next_dispatch_sequence: 1,
    };
    let _ = startup.send(Ok(()));

    while let Ok(message) = receiver.recv() {
        match message {
            SchedulerMessage::Command(SchedulerCommand::Submit { request, reply }) => {
                let id = TaskSessionId(scheduler.next_session_id);
                scheduler.next_session_id = scheduler.next_session_id.saturating_add(1);
                let entry = SessionEntry {
                    session: TaskSession::new(id, request),
                    cancellation: TaskCancellation::default(),
                };
                let snapshot = entry.session.snapshot();
                scheduler.sessions.insert(id, entry);
                scheduler.queue.push_back(id);
                let _ = reply.send(snapshot);
                scheduler.dispatch();
            }
            SchedulerMessage::Command(SchedulerCommand::Cancel { id, reply }) => {
                let result = scheduler.cancel(id);
                let _ = reply.send(result);
                scheduler.dispatch();
            }
            SchedulerMessage::Command(SchedulerCommand::GetSession { id, reply }) => {
                let _ = reply.send(
                    scheduler
                        .sessions
                        .get(&id)
                        .map(|entry| entry.session.snapshot()),
                );
            }
            SchedulerMessage::Command(SchedulerCommand::ListSessions { reply }) => {
                let mut sessions = scheduler
                    .sessions
                    .values()
                    .map(|entry| entry.session.snapshot())
                    .collect::<Vec<_>>();
                sessions.sort_by_key(|session| session.id.0);
                let _ = reply.send(sessions);
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
            SchedulerMessage::Command(SchedulerCommand::RemoveSession { id, reply }) => {
                let result = scheduler.remove_session(id);
                let _ = reply.send(result);
            }
            SchedulerMessage::Command(SchedulerCommand::Shutdown { reply }) => {
                scheduler.shutdown_workers();
                let _ = reply.send(());
                break;
            }
            SchedulerMessage::WorkerFinished {
                worker_id,
                session_id,
                outcome,
            } => {
                scheduler.finish(worker_id, session_id, outcome);
                scheduler.dispatch();
            }
        }
    }
}

impl Scheduler {
    fn dispatch(&mut self) {
        loop {
            let Some(worker_index) = self
                .workers
                .iter()
                .position(|worker| worker.snapshot.state == WorkerState::Idle)
            else {
                return;
            };
            let Some(session_id) = self.next_queued_session() else {
                return;
            };
            let dispatch_sequence = self.next_dispatch_sequence;
            self.next_dispatch_sequence = self.next_dispatch_sequence.saturating_add(1);

            let Some(entry) = self.sessions.get_mut(&session_id) else {
                continue;
            };
            let worker_id = self.workers[worker_index].snapshot.id;
            entry.session.assign(worker_id, dispatch_sequence);
            let assignment = TaskAssignment {
                session_id,
                request: entry.session.request().clone(),
                cancellation: entry.cancellation.clone(),
            };
            self.workers[worker_index].snapshot.state = WorkerState::Running;
            self.workers[worker_index].snapshot.session_id = Some(session_id);

            if self.workers[worker_index]
                .sender
                .send(WorkerCommand::Execute(assignment))
                .is_err()
            {
                entry
                    .session
                    .fail("Worker channel closed before dispatch.".to_string());
                self.workers[worker_index].snapshot.state = WorkerState::Stopped;
                self.workers[worker_index].snapshot.session_id = None;
            }
        }
    }

    fn next_queued_session(&mut self) -> Option<TaskSessionId> {
        while let Some(id) = self.queue.pop_front() {
            if self
                .sessions
                .get(&id)
                .is_some_and(|entry| entry.session.state() == TaskSessionState::Queued)
            {
                return Some(id);
            }
        }
        None
    }

    fn cancel(&mut self, id: TaskSessionId) -> Result<bool, ExecutionEngineError> {
        let entry = self
            .sessions
            .get_mut(&id)
            .ok_or(ExecutionEngineError::SessionNotFound(id))?;
        match entry.session.state() {
            TaskSessionState::Queued => {
                entry.cancellation.cancel();
                entry.session.cancel_queued();
                Ok(true)
            }
            TaskSessionState::Running => {
                entry.cancellation.cancel();
                entry.session.request_cancellation();
                Ok(true)
            }
            TaskSessionState::Cancelling => Ok(false),
            TaskSessionState::Succeeded
            | TaskSessionState::Failed
            | TaskSessionState::Cancelled => Ok(false),
        }
    }

    fn finish(&mut self, worker_id: usize, session_id: TaskSessionId, outcome: WorkerOutcome) {
        let Some(worker) = self.workers.iter_mut().find(|worker| {
            worker.snapshot.id == worker_id && worker.snapshot.session_id == Some(session_id)
        }) else {
            return;
        };
        worker.snapshot.state = WorkerState::Idle;
        worker.snapshot.session_id = None;
        worker.snapshot.completed_sessions = worker.snapshot.completed_sessions.saturating_add(1);

        let Some(entry) = self.sessions.get_mut(&session_id) else {
            return;
        };
        if entry.session.state() == TaskSessionState::Cancelling
            || entry.cancellation.is_cancelled()
        {
            entry.session.cancel();
            return;
        }
        match outcome {
            WorkerOutcome::Succeeded => entry.session.succeed(),
            WorkerOutcome::Failed(error) => entry.session.fail(error),
            WorkerOutcome::Cancelled => entry.session.cancel(),
        }
    }

    fn remove_session(&mut self, id: TaskSessionId) -> Result<bool, ExecutionEngineError> {
        let Some(entry) = self.sessions.get(&id) else {
            return Ok(false);
        };
        if !entry.session.state().is_terminal() {
            return Err(ExecutionEngineError::SessionNotTerminal(id));
        }
        self.sessions.remove(&id);
        Ok(true)
    }

    fn shutdown_workers(&mut self) {
        for entry in self.sessions.values_mut() {
            if !entry.session.state().is_terminal() {
                entry.cancellation.cancel();
            }
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
                            let context = TaskExecutionContext {
                                session_id: assignment.session_id,
                                worker_id,
                                request: assignment.request,
                                cancellation: assignment.cancellation.clone(),
                            };
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                worker_executor.execute(&context)
                            }));
                            let outcome = if context.cancellation.is_cancelled() {
                                WorkerOutcome::Cancelled
                            } else {
                                match result {
                                    Ok(Ok(())) => WorkerOutcome::Succeeded,
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
                                    session_id: assignment.session_id,
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
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

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
}
