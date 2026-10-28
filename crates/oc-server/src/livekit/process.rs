//! One run of livekit-server: started with its config in the environment, its log
//! forwarded, ready once signalling answers, stopped on request.
//!
//! It must never outlive the camp, even one killed without cleanup (a crash, Task
//! Manager, `kill -9`): a leftover LiveKit holds the media ports and breaks the next
//! start. Windows: a job object that kills it when our handle closes, which the OS does
//! for a dead process. Unix: a tiny `sh` guardian that starts LiveKit and holds a pipe
//! whose other end only we have; the pipe closing — stop or death — makes the
//! guardian end LiveKit.
//!
//! LiveKit also runs in its own process group: Ctrl+C in the terminal reaches only the
//! camp, which then stops LiveKit itself, in order, instead of LiveKit quitting first
//! and looking like a crash.

use std::collections::VecDeque;
use std::io;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// Everything a (re)start needs.
pub(super) struct Launch {
    pub binary: PathBuf,
    /// `LIVEKIT_CONFIG`: YAML.
    pub config: String,
    /// `LIVEKIT_KEYS`: `key: secret`.
    pub keys: String,
    pub signal_port: u16,
    /// (TCP, UDP), for the "ports are taken" hint.
    pub media_ports: (u16, u16),
}

/// Lines of LiveKit's log kept to explain a failed start.
const RECENT_LINES: usize = 30;
/// How long a stopped LiveKit gets to exit before it is killed (unix: the guardian
/// signals it; Windows has no signals and stops it at once).
#[cfg(unix)]
const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long the log readers get to drain after LiveKit exits.
const LOG_DRAIN: Duration = Duration::from_secs(1);

/// Variables LiveKit reads on its own (`livekit-server --help`). Inherited ones would
/// override parts of the camp's config, so the child does not get them.
fn inherited_livekit_setting(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    name.starts_with("LIVEKIT_")
        || ["NODE_IP", "UDP_PORT", "REDIS_HOST", "REDIS_PASSWORD"].contains(&name.as_str())
}

/// Unix guardian. `$1` is livekit-server. fd 3 keeps our pipe: `read` returns at EOF,
/// which happens only when the camp closes it or dies. LiveKit is then asked twice
/// (the second SIGTERM skips its drain) and killed if still there. The first line
/// tells the camp LiveKit's own pid.
#[cfg(unix)]
const GUARDIAN: &str = r#"
exec 3<&0
"$1" </dev/null &
child=$!
echo "oc-guardian-pid $child"
{
  while read -r _ <&3; do :; done
  kill -TERM "$child" 2>/dev/null
  sleep 2
  kill -TERM "$child" 2>/dev/null
  sleep 3
  kill -KILL "$child" 2>/dev/null
} &
watcher=$!
wait "$child"
status=$?
kill "$watcher" 2>/dev/null
exit "$status"
"#;
#[cfg(unix)]
const GUARDIAN_PID: &str = "oc-guardian-pid ";

pub(super) struct Process {
    child: Child,
    /// LiveKit's own pid (on unix not the guardian's); 0 until known.
    pid: u32,
    #[cfg(unix)]
    guardian_pid: Option<tokio::sync::oneshot::Receiver<u32>>,
    /// Our end of the guardian's pipe; dropping it stops LiveKit.
    #[cfg(unix)]
    lifeline: Option<tokio::process::ChildStdin>,
    /// Closing it kills LiveKit; held for as long as the process runs.
    #[cfg(windows)]
    _job: super::job::Job,
    recent: Arc<Mutex<VecDeque<String>>>,
    readers: Vec<JoinHandle<()>>,
    launch_ports: (u16, u16),
    signal_port: u16,
}

impl Process {
    pub fn spawn(launch: &Launch) -> io::Result<Self> {
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(GUARDIAN)
                .arg("oc-livekit-guardian")
                .arg(&launch.binary)
                .stdin(Stdio::piped())
                .process_group(0);
            command
        };
        #[cfg(windows)]
        let mut command = {
            use windows_sys::Win32::System::Threading::{
                CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
            };
            let mut command = Command::new(&launch.binary);
            // No console window either: the server window (step 6) has none to share.
            command
                .stdin(Stdio::null())
                .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
            command
        };
        for (name, _) in std::env::vars_os() {
            if inherited_livekit_setting(&name) {
                command.env_remove(name);
            }
        }
        command
            .env("LIVEKIT_CONFIG", &launch.config)
            .env("LIVEKIT_KEYS", &launch.keys)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Belt and braces for paths that drop a `Process` without `stop`.
            .kill_on_drop(true);
        let mut child = command.spawn()?;

        // The window between spawn and assignment is a few instructions; LiveKit
        // starts no processes of its own that could escape it.
        #[cfg(windows)]
        let job = {
            let job = super::job::Job::new()?;
            let handle = child
                .raw_handle()
                .ok_or_else(|| io::Error::other("LiveKit exited at once"))?;
            job.assign(handle)?;
            job
        };

        let recent = Arc::new(Mutex::new(VecDeque::with_capacity(RECENT_LINES)));
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        #[cfg(unix)]
        let (pid_tx, guardian_pid) = tokio::sync::oneshot::channel();
        #[cfg(unix)]
        let first_line = Some(pid_tx);
        #[cfg(not(unix))]
        let first_line = None;
        let readers = vec![
            tokio::spawn(forward(stdout, Arc::clone(&recent), first_line)),
            tokio::spawn(forward(stderr, Arc::clone(&recent), None)),
        ];

        Ok(Self {
            #[cfg(windows)]
            pid: child.id().unwrap_or_default(),
            #[cfg(unix)]
            pid: 0,
            #[cfg(unix)]
            guardian_pid: Some(guardian_pid),
            #[cfg(unix)]
            lifeline: child.stdin.take(),
            #[cfg(windows)]
            _job: job,
            child,
            recent,
            readers,
            launch_ports: launch.media_ports,
            signal_port: launch.signal_port,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Waits until signalling accepts connections. On failure LiveKit is stopped and
    /// the error carries the end of its log.
    pub async fn ready(mut self, timeout: Duration) -> Result<Self, String> {
        let deadline = tokio::time::Instant::now() + timeout;
        #[cfg(unix)]
        if let Some(pid) = self.guardian_pid.take() {
            // The outcome first, then act: a `select!` handler cannot touch `self`
            // while another branch still borrows `self.child`.
            let exited = tokio::select! {
                pid = pid => {
                    if let Ok(pid) = pid {
                        self.pid = pid;
                    }
                    None
                }
                status = self.child.wait() => Some(describe(status)),
                () = tokio::time::sleep_until(deadline) => None,
            };
            if let Some(status) = exited {
                return Err(self.failure(&status).await);
            }
        }
        loop {
            if TcpStream::connect((Ipv4Addr::LOCALHOST, self.signal_port))
                .await
                .is_ok()
            {
                return Ok(self);
            }
            let exited = tokio::select! {
                status = self.child.wait() => Some(describe(status)),
                () = tokio::time::sleep(Duration::from_millis(100)) => None,
            };
            if let Some(status) = exited {
                return Err(self.failure(&status).await);
            }
            if tokio::time::Instant::now() >= deadline {
                let last = self.last_line();
                self.stop().await;
                return Err(format!(
                    "LiveKit did not answer within {} s; its last words: {last}",
                    timeout.as_secs()
                ));
            }
        }
    }

    /// Until LiveKit exits on its own. Cancel-safe.
    pub async fn wait(&mut self) -> String {
        let status = describe(self.child.wait().await);
        self.drain().await;
        status
    }

    /// Asks LiveKit to exit, kills it after [`STOP_GRACE`], waits until it is gone.
    pub async fn stop(mut self) {
        #[cfg(unix)]
        {
            drop(self.lifeline.take()); // the guardian ends LiveKit
            if tokio::time::timeout(STOP_GRACE, self.child.wait())
                .await
                .is_err()
            {
                let _ = self.child.kill().await;
            }
        }
        #[cfg(windows)]
        {
            // No signals on Windows; LiveKit keeps nothing that a hard stop loses.
            let _ = self.child.kill().await;
        }
        self.drain().await;
    }

    async fn drain(&mut self) {
        for reader in self.readers.drain(..) {
            let _ = tokio::time::timeout(LOG_DRAIN, reader).await;
        }
    }

    /// The whole log was forwarded as it came; errors quote only its last line, where
    /// LiveKit says why it gave up.
    fn last_line(&self) -> String {
        let recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
        recent.back().cloned().unwrap_or_else(|| "(nothing)".into())
    }

    /// LiveKit exited before it was ready: why, in words a host can act on.
    async fn failure(&mut self, status: &str) -> String {
        self.drain().await;
        let taken = {
            let recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
            recent.iter().any(|line| ports_taken(line))
        };
        if taken {
            let (tcp, udp) = self.launch_ports;
            return format!(
                "LiveKit could not take its ports {tcp}/tcp and {udp}/udp — is another \
                 LiveKit running? (`pnpm livekit:down` stops the Docker one)"
            );
        }
        format!("LiveKit exited at start ({status}): {}", self.last_line())
    }
}

fn describe(status: io::Result<std::process::ExitStatus>) -> String {
    match status {
        Ok(status) => status.to_string(),
        Err(e) => format!("unknown status: {e}"),
    }
}

/// Go's bind error on unix and its Windows wording (WSAEADDRINUSE).
fn ports_taken(log: &str) -> bool {
    log.contains("address already in use") || log.contains("Only one usage of each socket address")
}

/// Copies LiveKit's output to ours, line by line, keeping the last lines. Bytes, not
/// `lines()`: invalid UTF-8 must not stop the reader, or the pipe fills and LiveKit
/// blocks on its next log line.
async fn forward(
    stream: impl AsyncRead + Unpin,
    recent: Arc<Mutex<VecDeque<String>>>,
    mut first_line: Option<tokio::sync::oneshot::Sender<u32>>,
) {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end();
        #[cfg(unix)]
        if let Some(pid) = first_line.take() {
            if let Some(n) = text.strip_prefix(GUARDIAN_PID).and_then(|n| n.parse().ok()) {
                let _ = pid.send(n);
                continue;
            }
        }
        #[cfg(not(unix))]
        let _ = first_line.take();
        eprintln!("[livekit] {text}");
        let mut recent = recent.lock().unwrap_or_else(PoisonError::into_inner);
        if recent.len() == RECENT_LINES {
            recent.pop_front();
        }
        recent.push_back(text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_livekits_own_variables_are_withheld() {
        for name in ["LIVEKIT_KEYS", "livekit_config", "NODE_IP", "REDIS_HOST"] {
            assert!(inherited_livekit_setting(name.as_ref()), "{name}");
        }
        for name in ["PATH", "SystemRoot", "HOME", "NODE_OPTIONS"] {
            assert!(!inherited_livekit_setting(name.as_ref()), "{name}");
        }
    }

    #[test]
    fn a_bind_failure_is_recognised_on_every_os() {
        assert!(ports_taken(
            "listen udp4 0.0.0.0:7882: bind: address already in use"
        ));
        assert!(ports_taken(
            "listen tcp :7881: bind: Only one usage of each socket address \
             (protocol/network address/port) is normally permitted."
        ));
        assert!(!ports_taken("starting LiveKit server"));
    }
}
