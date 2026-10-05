//! Shared harness: runs the real executable against a generated config, with fake executors on
//! `PATH`, an in-process stub of the Sylvode API that records every request, and an in-process
//! WebSocket control plane for the tunnel.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-webhook");
pub const LEGACY: &str = env!("CARGO_BIN_EXE_openpr-webhook");
pub const SECRET: &str = "e2e-webhook-secret";

/// A directory removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-webhook-e2e-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running server process. Dropping it kills and reaps the process.
pub struct Server {
    child: Option<Child>,
    pub addr: SocketAddr,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    stderr: Option<std::thread::JoinHandle<Vec<String>>>,
    _dir: TempDir,
}

/// What a process that did not come up wrote before it exited.
#[derive(Debug)]
pub struct Refused {
    pub code: Option<i32>,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

fn spawn_line_reader(stream: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Server {
    /// Starts `binary` with `config` (which must listen on `127.0.0.1:0`) and waits until it logs
    /// the address it bound. `Err` carries the output of a process that exited instead.
    pub fn start(binary: &str, config: &str, env: &[(&str, &str)]) -> Result<Self, Refused> {
        let dir = TempDir::new("server");
        let config_path = dir.file("config.toml");
        std::fs::write(&config_path, config).expect("write config");
        let mut command = Command::new(binary);
        command
            .arg(&config_path)
            .env_remove("RUST_LOG")
            .env_remove("SYLVODE_WEBHOOK_SAFE_MODE")
            .env_remove("OPENPR_WEBHOOK_SAFE_MODE")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn server");
        let lines = spawn_line_reader(child.stdout.take().expect("piped stdout"));
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let stderr = std::thread::spawn(move || {
            BufReader::new(stderr_pipe)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>()
        });
        let mut server = Self {
            child: Some(child),
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            lines,
            seen: Vec::new(),
            stderr: Some(stderr),
            _dir: dir,
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match server.lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let bound = line
                        .split_once(" listening on ")
                        .and_then(|(_, addr)| addr.trim().parse::<SocketAddr>().ok());
                    server.seen.push(line);
                    if let Some(addr) = bound {
                        server.addr = addr;
                        return Ok(server);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if server.child_mut().try_wait().expect("try_wait").is_some() {
                        return Err(server.into_refused());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(server.into_refused()),
            }
        }
        panic!("{binary} did not report a listening address: {:?}", server.seen);
    }

    const fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("server process already reaped")
    }

    fn into_refused(mut self) -> Refused {
        let mut child = self.child.take().expect("server process already reaped");
        let status = child.wait().expect("wait for refused server");
        self.seen.extend(self.lines.try_iter());
        let stderr = self
            .stderr
            .take()
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        Refused {
            code: status.code(),
            stdout: std::mem::take(&mut self.seen),
            stderr,
        }
    }

    /// Every stdout line written so far.
    pub fn stdout(&mut self) -> &[String] {
        self.seen.extend(self.lines.try_iter());
        &self.seen
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
