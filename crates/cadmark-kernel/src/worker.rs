// The kernel worker: the application's side of the script-execution
// process boundary, and the body of the worker binary itself.
//
// One `KernelWorker` owns one child process (`cadmark-kernel-worker`) that
// stays warm across executions — the Python runtime takes seconds to
// import, and a fresh process per script would pay it every time. The
// child confines itself (`sandbox.rs`) before it initialises Python; the
// parent enforces the wall-clock and memory ceilings by watching the
// child while a request is in flight and killing it when a ceiling is
// crossed, or when the caller cancels. A killed worker is replaced
// transparently on the next request.
//
// Requests and replies are single JSON lines on the child's stdin and
// stdout (`protocol.rs`). The child's stderr is inherited so its logging
// reaches the same place as the application's.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cadmark_core::cancellation::CancelFlag;
use cadmark_core::export::ExportFormat;
use cadmark_core::limits::{ExecutionLimits, LimitHit};
use thiserror::Error;

use crate::protocol::{ExecutedModel, ModelFile, WorkerFailure, WorkerReply, WorkerRequest};

/// The worker binary's name, built alongside the application.
pub const WORKER_BINARY: &str = "cadmark-kernel-worker";

/// How often the supervisor samples the child's memory and the clock.
const SUPERVISION_INTERVAL: Duration = Duration::from_millis(50);

/// How long the worker may take to report itself ready (Python import).
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// The line the child prints once it is confined and Python is up.
const READY_LINE: &str = "ready";

#[derive(Debug, Error)]
pub enum WorkerError {
    /// The script failed; the message is the traceback or diagnostic the
    /// AI can act on.
    #[error("{0}")]
    Script(String),
    /// A ceiling was crossed and the script was stopped.
    #[error("{}", .hit.describe(.limits))]
    Limit { hit: LimitHit, limits: ExecutionLimits },
    /// The caller cancelled while the request was in flight.
    #[error("execution was cancelled")]
    Cancelled,
    /// CADmark's runtime failed: the worker could not start, died, or
    /// broke the protocol. Not the AI's to repair.
    #[error("kernel worker failure: {0}")]
    Runtime(String),
}

impl WorkerError {
    /// Whether handing the message back to the AI could plausibly fix it.
    pub fn is_script_fault(&self) -> bool {
        matches!(self, Self::Script(_) | Self::Limit { .. })
    }
}

/// Where the worker binary and the Python runtime live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLaunch {
    /// Path of the worker executable.
    pub binary: PathBuf,
    /// The project folder the worker may read and write.
    pub project_dir: PathBuf,
    /// The virtual environment holding build123d, or `None` to let the
    /// worker discover it as the tests do.
    pub venv: Option<PathBuf>,
}

impl WorkerLaunch {
    /// The worker binary next to the running executable, which is where
    /// Cargo puts both binaries of one build.
    pub fn beside_current_exe(project_dir: PathBuf) -> Result<Self, WorkerError> {
        let exe = std::env::current_exe()
            .map_err(|error| WorkerError::Runtime(format!("current executable unknown: {error}")))?;
        let binary = exe.with_file_name(WORKER_BINARY);
        if !binary.is_file() {
            return Err(WorkerError::Runtime(format!(
                "worker binary not found at {}",
                binary.display()
            )));
        }
        Ok(Self {
            binary,
            project_dir,
            venv: None,
        })
    }
}

/// A live worker process and the channels to it.
struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Held so the directory outlives the process; removed on drop.
    _scratch_dir: tempfile::TempDir,
}

/// The application's handle on script execution and export.
pub struct KernelWorker {
    launch: WorkerLaunch,
    process: Option<Process>,
}

impl KernelWorker {
    /// Create a handle; the process starts on the first request.
    pub fn new(launch: WorkerLaunch) -> Self {
        Self {
            launch,
            process: None,
        }
    }

    /// Execute the script at `script_path`, stopping it if it crosses
    /// either ceiling or the caller cancels.
    pub fn execute(
        &mut self,
        script_path: &Path,
        limits: ExecutionLimits,
        cancel: &CancelFlag,
    ) -> Result<ExecutedModel, WorkerError> {
        let request = WorkerRequest::Execute {
            script_path: script_path.to_path_buf(),
        };
        match self.request(&request, Some(limits), cancel)? {
            WorkerReply::Executed(model) => Ok(model),
            WorkerReply::Failed(failure) => Err(failure_error(failure)),
            WorkerReply::Exported => Err(WorkerError::Runtime(
                "worker replied to an execution with an export".to_string(),
            )),
        }
    }

    /// Write a kept model to `path`. Exports run under the wall-clock
    /// ceiling but no memory ceiling: writing a mesh is bounded work.
    pub fn export(
        &mut self,
        model: &ModelFile,
        format: ExportFormat,
        path: &Path,
        limits: ExecutionLimits,
    ) -> Result<(), WorkerError> {
        let request = WorkerRequest::Export {
            model: model.clone(),
            format,
            path: path.to_path_buf(),
        };
        let limits = ExecutionLimits {
            memory_bytes: u64::MAX,
            ..limits
        };
        match self.request(&request, Some(limits), &CancelFlag::new())? {
            WorkerReply::Exported => Ok(()),
            WorkerReply::Failed(failure) => Err(failure_error(failure)),
            WorkerReply::Executed(_) => Err(WorkerError::Runtime(
                "worker replied to an export with a model".to_string(),
            )),
        }
    }

    fn request(
        &mut self,
        request: &WorkerRequest,
        limits: Option<ExecutionLimits>,
        cancel: &CancelFlag,
    ) -> Result<WorkerReply, WorkerError> {
        if self.process.is_none() {
            self.process = Some(self.start()?);
        }
        let process = self.process.as_mut().expect("process started above");

        let mut line = serde_json::to_string(request)
            .map_err(|error| WorkerError::Runtime(format!("could not encode request: {error}")))?;
        line.push('\n');
        if let Err(error) = process.stdin.write_all(line.as_bytes()).and_then(|()| process.stdin.flush()) {
            self.discard();
            return Err(WorkerError::Runtime(format!("could not reach the worker: {error}")));
        }

        let outcome = supervise(process, limits, cancel);
        match outcome {
            Ok(reply) => Ok(reply),
            Err(error) => {
                // Anything but a clean script failure leaves the process in
                // an unknown state; the next request starts a fresh one.
                self.discard();
                Err(error)
            }
        }
    }

    fn start(&self) -> Result<Process, WorkerError> {
        let scratch_dir = tempfile::Builder::new()
            .prefix("cadmark-kernel-")
            .tempdir()
            .map_err(|error| WorkerError::Runtime(format!("could not create scratch directory: {error}")))?;
        let mut command = Command::new(&self.launch.binary);
        command
            .arg("--project-dir")
            .arg(&self.launch.project_dir)
            .arg("--scratch-dir")
            .arg(scratch_dir.path())
            // Relative paths in a script land in the project folder.
            .current_dir(&self.launch.project_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            // The child gets no environment at all: no credential, no
            // proxy, no shell state is reachable from executed code. What
            // it needs, it is told by argument.
            .env_clear();
        if let Some(venv) = &self.launch.venv {
            command.arg("--venv").arg(venv);
        }
        if let Some(level) = std::env::var_os("RUST_LOG") {
            command.env("RUST_LOG", level);
        }
        let mut child = command
            .spawn()
            .map_err(|error| WorkerError::Runtime(format!("could not start the worker: {error}")))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut process = Process {
            child,
            stdin,
            stdout,
            _scratch_dir: scratch_dir,
        };
        let ready = read_line_within(&mut process, START_TIMEOUT)?;
        if ready.trim() != READY_LINE {
            let _ = process.child.kill();
            return Err(WorkerError::Runtime(format!(
                "worker did not become ready: {}",
                ready.trim()
            )));
        }
        Ok(process)
    }

    fn discard(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.child.kill();
            let _ = process.child.wait();
        }
    }
}

impl Drop for KernelWorker {
    fn drop(&mut self) {
        self.discard();
    }
}

fn failure_error(failure: WorkerFailure) -> WorkerError {
    match failure {
        WorkerFailure::Script { message } => WorkerError::Script(message),
        WorkerFailure::Runtime { message } => WorkerError::Runtime(message),
    }
}

/// Wait for the child's reply line while watching the clock, its memory,
/// and the cancel flag. The read happens on a helper thread so the watch
/// can kill the child mid-request; the thread ends when the pipe closes.
fn supervise(
    process: &mut Process,
    limits: Option<ExecutionLimits>,
    cancel: &CancelFlag,
) -> Result<WorkerReply, WorkerError> {
    let started = Instant::now();
    let pid = process.child.id();
    let (reply_tx, reply_rx) = mpsc::channel();
    // The reader borrows the stdout handle for the duration of one request;
    // scoped threads make that borrow sound without moving the handle out.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut line = String::new();
            let read = process.stdout.read_line(&mut line);
            let _ = reply_tx.send(read.map(|_| line));
        });

        loop {
            match reply_rx.recv_timeout(SUPERVISION_INTERVAL) {
                Ok(Ok(line)) if line.is_empty() => {
                    return Err(WorkerError::Runtime("the worker exited without replying".to_string()));
                }
                Ok(Ok(line)) => {
                    return serde_json::from_str(&line).map_err(|error| {
                        WorkerError::Runtime(format!("unreadable worker reply: {error}"))
                    });
                }
                Ok(Err(error)) => {
                    return Err(WorkerError::Runtime(format!("lost the worker: {error}")));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(WorkerError::Runtime("the worker reader stopped".to_string()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }

            if cancel.is_cancelled() {
                kill(&mut process.child);
                return Err(WorkerError::Cancelled);
            }
            if let Some(limits) = limits {
                if started.elapsed() >= limits.wall_clock {
                    kill(&mut process.child);
                    return Err(WorkerError::Limit {
                        hit: LimitHit::WallClock,
                        limits,
                    });
                }
                if resident_memory_bytes(pid).is_some_and(|resident| resident > limits.memory_bytes) {
                    kill(&mut process.child);
                    return Err(WorkerError::Limit {
                        hit: LimitHit::Memory,
                        limits,
                    });
                }
            }
        }
    })
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Read the worker's first line, killing a worker that hangs on start-up
/// rather than waiting on it.
fn read_line_within(process: &mut Process, timeout: Duration) -> Result<String, WorkerError> {
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut line = String::new();
            let read = process.stdout.read_line(&mut line);
            let _ = tx.send(read.map(|_| line));
        });
        loop {
            match rx.recv_timeout(SUPERVISION_INTERVAL) {
                Ok(Ok(line)) => return Ok(line),
                Ok(Err(error)) => {
                    return Err(WorkerError::Runtime(format!("lost the worker: {error}")));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(WorkerError::Runtime("the worker reader stopped".to_string()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if started.elapsed() >= timeout {
                kill(&mut process.child);
                return Err(WorkerError::Runtime(format!(
                    "the worker took longer than {} s to start",
                    timeout.as_secs()
                )));
            }
        }
    })
}

/// Resident set size of a process, from procfs. `None` when the process
/// has already gone.
fn resident_memory_bytes(pid: u32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf with a valid name has no preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(resident_pages * u64::try_from(page_size).unwrap_or(4096))
}

// ── The worker binary's body ──────────────────────────────────────────

/// Command-line arguments of the worker binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerArgs {
    pub project_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub venv: Option<PathBuf>,
}

impl WorkerArgs {
    /// Parse `--project-dir <p> --scratch-dir <p> [--venv <p>]`.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut project_dir = None;
        let mut scratch_dir = None;
        let mut venv = None;
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--project-dir" => project_dir = Some(value),
                "--scratch-dir" => scratch_dir = Some(value),
                "--venv" => venv = Some(value),
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(Self {
            project_dir: project_dir.ok_or("--project-dir is required")?,
            scratch_dir: scratch_dir.ok_or("--scratch-dir is required")?,
            venv,
        })
    }
}

/// Run the worker: confine the process, start Python, report ready, then
/// serve requests until stdin closes. Returns the process exit code.
pub fn run_worker(args: WorkerArgs) -> i32 {
    let venv = args.venv.clone().or_else(crate::execution::discover_venv);
    let policy = crate::sandbox::SandboxPolicy {
        project_dir: args.project_dir.clone(),
        scratch_dir: args.scratch_dir.clone(),
        runtime_roots: runtime_roots(venv.as_deref()),
    };
    let confinement = match crate::sandbox::confine(&policy) {
        Ok(confinement) => confinement,
        Err(error) => {
            // Fail closed: an unconfined worker never runs a script.
            println!("refused: {error}");
            return 2;
        }
    };
    log::info!("kernel worker confined ({confinement:?}) to {}", policy.project_dir.display());

    crate::python_runtime::configure_python_home();
    if let Some(venv) = &venv
        && let Err(error) = crate::execution::activate_venv(venv)
    {
        println!("refused: could not activate the Python runtime: {error}");
        return 2;
    }
    println!("{READY_LINE}");

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let reply = match serde_json::from_str::<WorkerRequest>(&line) {
            Ok(request) => serve(request, &args.scratch_dir),
            Err(error) => WorkerReply::Failed(WorkerFailure::Runtime {
                message: format!("unreadable request: {error}"),
            }),
        };
        let mut encoded = serde_json::to_string(&reply).expect("replies serialise");
        encoded.push('\n');
        if stdout.write_all(encoded.as_bytes()).and_then(|()| stdout.flush()).is_err() {
            break;
        }
    }
    0
}

/// The directories the Python runtime reads: the interpreter home the
/// build baked in, and the virtual environment (with its symlink target
/// resolved, since Landlock rules follow the real path).
fn runtime_roots(venv: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = crate::python_runtime::embedded_python_home() {
        roots.push(home);
    }
    if let Some(venv) = venv {
        roots.push(venv.to_path_buf());
        if let Ok(real) = std::fs::canonicalize(venv.join("bin").join("python")) {
            if let Some(home) = real.parent().and_then(Path::parent) {
                roots.push(home.to_path_buf());
            }
        }
    }
    roots
}

fn serve(request: WorkerRequest, scratch_dir: &Path) -> WorkerReply {
    match request {
        WorkerRequest::Execute { script_path } => {
            match crate::execution::execute_script(&script_path, scratch_dir) {
                Ok(model) => WorkerReply::Executed(model),
                Err(error) => WorkerReply::Failed(classify(error)),
            }
        }
        WorkerRequest::Export {
            model,
            format,
            path,
        } => match crate::export::export_model(&model, format, &path) {
            Ok(()) => WorkerReply::Exported,
            Err(error) => WorkerReply::Failed(WorkerFailure::Runtime {
                message: error.to_string(),
            }),
        },
    }
}

/// Faults in the script itself are the AI's to fix; faults in CADmark's
/// runtime are not, and telling the AI about them would only make it
/// "repair" correct code.
fn classify(error: crate::execution::ExecutionError) -> WorkerFailure {
    use crate::execution::ExecutionError;
    let message = error.to_string();
    match error {
        ExecutionError::Script(_)
        | ExecutionError::Python(_)
        | ExecutionError::Tessellation(_)
        | ExecutionError::ScriptNotFound(_) => WorkerFailure::Script { message },
        ExecutionError::ModelFile(_) | ExecutionError::Provenance(_) => {
            WorkerFailure::Runtime { message }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_arguments_require_both_directories() {
        let parsed = WorkerArgs::parse(
            ["--project-dir", "/p", "--scratch-dir", "/s", "--venv", "/v"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(parsed.project_dir, PathBuf::from("/p"));
        assert_eq!(parsed.scratch_dir, PathBuf::from("/s"));
        assert_eq!(parsed.venv, Some(PathBuf::from("/v")));

        let missing = WorkerArgs::parse(["--project-dir", "/p"].into_iter().map(String::from));
        assert_eq!(missing.unwrap_err(), "--scratch-dir is required");
        let dangling = WorkerArgs::parse(["--venv"].into_iter().map(String::from));
        assert_eq!(dangling.unwrap_err(), "--venv needs a value");
    }

    #[test]
    fn script_faults_are_the_ai_s_and_runtime_faults_are_not() {
        assert!(WorkerError::Script("NameError".into()).is_script_fault());
        assert!(
            WorkerError::Limit {
                hit: LimitHit::WallClock,
                limits: ExecutionLimits::default()
            }
            .is_script_fault()
        );
        assert!(!WorkerError::Cancelled.is_script_fault());
        assert!(!WorkerError::Runtime("worker died".into()).is_script_fault());
    }

    #[test]
    fn resident_memory_of_this_process_is_readable() {
        let resident = resident_memory_bytes(std::process::id()).unwrap();
        assert!(resident > 0);
        assert_eq!(resident_memory_bytes(u32::MAX), None);
    }
}
