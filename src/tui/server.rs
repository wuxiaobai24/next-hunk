//! Optional server-mode plumbing for `next-hunk serve`.
//!
//! When a human runs `next-hunk serve`, a persistent TUI stays open and also
//! listens on a Unix socket. A separate CLI process (`next-hunk push` /
//! `next-hunk decision`) connects and exchanges newline-delimited JSON
//! messages. The protocol is **private** — agents and humans only ever touch
//! the CLI surface, never this protocol directly.
//!
//! Architecture mirrors [`crate::tui::watch`]: a background thread owns the
//! blocking I/O (here `UnixListener::accept`) and forwards parsed work over an
//! `mpsc` channel that the main run loop drains non-blockingly each frame.
//! This keeps the main loop synchronous and short — no async runtime.
//!
//! Request/response: each connection sends one command and expects one reply.
//! For commands that need the main thread's `App` state (e.g. `Decision`),
//! the accept thread pairs the command with a `oneshot` sender; the main loop
//! processes it and fulfills the reply, which the accept thread writes back.

#![cfg(all(feature = "serve", unix))]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result};

use crate::tui::app::{FocusTarget, Note, Selections};

/// A request from a CLI client (`push` / `decision`), paired with the channel
/// the main loop uses to send back the [`ServerReply`].
#[derive(Debug)]
pub struct ServerRequest {
    pub command: ServerCommand,
    pub reply: std::sync::mpsc::Sender<ServerReply>,
}

/// Client → server message. Newline-delimited JSON; `#[serde(tag = "cmd")]`
/// makes the wire format self-describing and forward-compatible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd")]
pub enum ServerCommand {
    /// `next-hunk push`: append notes, replace the focus target.
    Push {
        focus: Option<FocusTarget>,
        notes: Vec<Note>,
    },
    /// `next-hunk decision`: request the human's accumulated decisions.
    Decision,
    /// `next-hunk get` / `next-hunk list`: request session metadata.
    Info,
}

/// Server → client reply. Same wire format.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "reply")]
pub enum ServerReply {
    /// Command applied successfully.
    Ok,
    /// Response to `Decision`: the current per-hunk decisions.
    Decisions(Selections),
    /// Response to `Info`: session metadata.
    Info {
        /// Repo root path (as reported by the server).
        repo_path: String,
        /// Number of files in the current review.
        file_count: usize,
    },
    /// Server-side error (e.g. malformed command).
    Error(String),
}

/// Handle to the running accept thread. Mirrors [`crate::tui::watch::Watcher`]:
/// holds the channel receiver plus the accept-thread join handle. The bound
/// `UnixListener` is moved into the accept thread (its lifetime is tied to the
/// thread); dropping this handle lets the thread observe the closed listener
/// and exit, and `Drop` unlinks the socket file.
#[derive(Debug)]
pub struct ServerListener {
    rx: mpsc::Receiver<ServerRequest>,
    /// Kept (not joined) so the accept thread stays alive for the listener's
    /// lifetime; the thread exits on its own when the listener starts erroring
    /// (process exit reclaims the fd).
    _thread: thread::JoinHandle<()>,
    socket_path: PathBuf,
}

impl ServerListener {
    /// Bind `socket_path`, spawn the accept thread, and return a handle. If the
    /// path is occupied, probes it: a successful connect means another server
    /// is already running (error out), a failed connect means a stale socket
    /// file (unlink and retry).
    pub fn spawn(socket_path: PathBuf) -> Result<Self> {
        let listener = bind_with_stale_probe(&socket_path)?;
        // Non-blocking so the accept thread can poll-and-sleep and exit when
        // the (dropped) listener starts erroring.
        listener
            .set_nonblocking(true)
            .context("set socket non-blocking")?;

        let (tx, rx) = mpsc::channel::<ServerRequest>();
        let _thread = thread::spawn(move || accept_loop(listener, tx));
        Ok(Self {
            rx,
            _thread,
            socket_path,
        })
    }

    /// Non-blocking drain of pending requests (mirror of
    /// [`crate::tui::watch::Watcher::drain`]). Returns all requests queued since
    /// the last drain; the main loop fulfills each reply.
    pub fn drain(&self) -> Vec<ServerRequest> {
        let mut out = Vec::new();
        while let Ok(req) = self.rx.try_recv() {
            out.push(req);
        }
        out
    }
}

impl Drop for ServerListener {
    fn drop(&mut self) {
        // Best-effort cleanup so a crashed/quit serve doesn't strand a socket.
        // The accept thread observes the closed listener (non-blocking accept
        // errors once the fd is reclaimed by the OS as the process exits); we
        // don't forcefully join to avoid blocking drop on a hung thread.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Bind the socket, reclaiming a stale leftover file if present. If the path
/// already hosts a *live* server (connect succeeds), refuse.
fn bind_with_stale_probe(socket_path: &Path) -> Result<UnixListener> {
    match UnixListener::bind(socket_path) {
        Ok(l) => Ok(l),
        Err(first) => {
            // Path exists. Live server, or stale leftover?
            if UnixStream::connect(socket_path).is_ok() {
                anyhow::bail!(
                    "a next-hunk server is already running for this repo (socket: {})",
                    socket_path.display()
                );
            }
            let _ = std::fs::remove_file(socket_path);
            UnixListener::bind(socket_path)
                .with_context(|| format!("bind socket {}", socket_path.display()))
                .map_err(|e| anyhow::anyhow!("{first}; retry also failed: {e}"))
        }
    }
}

/// The accept loop runs on its own thread. Because the listener is
/// non-blocking, we poll with a short sleep between attempts; when the listener
/// is dropped (serve quits), `accept` returns an error and the loop exits.
fn accept_loop(listener: UnixListener, tx: mpsc::Sender<ServerRequest>) {
    use std::time::Duration;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = handle_connection(stream, &tx) {
                    // Per-connection failures shouldn't kill the accept loop;
                    // log to stderr for debuggability.
                    eprintln!("next-hunk serve: connection error: {e}");
                }
            }
            Err(ref e) if would_block(e) => {
                // No pending connection; back off briefly and retry. The sleep
                // keeps CPU low; waking promptly isn't critical for the
                // low-frequency push/decision traffic.
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                // Listener closed (serve quitting) or fatal — stop the loop.
                break;
            }
        }
    }
}

/// Read one newline-delimited JSON command, forward it to the main loop, await
/// the reply, and write it back as one newline-delimited JSON line.
fn handle_connection(mut stream: UnixStream, tx: &mpsc::Sender<ServerRequest>) -> Result<()> {
    // The listener is non-blocking (so the accept loop can poll-and-sleep),
    // and on *some* platforms (notably macOS) that flag is inherited by the
    // accepted stream. A non-blocking stream makes the blocking `read_line` /
    // `write_all` below return WouldBlock immediately, which surfaces to the
    // client as ENOTCONN — observed flakily in `decision_returns_selections`
    // on the macOS CI runner. Restore blocking I/O here so this connection
    // does a straightforward blocking request/response exchange. (Linux does
    // not inherit the flag, so this is a no-op there.)
    stream
        .set_nonblocking(false)
        .context("set accepted stream blocking")?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("read command line from socket")?;
    let command: ServerCommand =
        serde_json::from_str(line.trim()).map_err(|e| anyhow::anyhow!("parse command: {e}"))?;

    // Pair with a reply channel and ship to the main loop.
    let (reply_tx, reply_rx) = mpsc::channel::<ServerReply>();
    tx.send(ServerRequest {
        command,
        reply: reply_tx,
    })
    .map_err(|_| anyhow::anyhow!("main loop gone (serve quitting?)"))?;

    let reply = reply_rx
        .recv()
        .unwrap_or_else(|_| ServerReply::Error("serve shutting down".into()));
    let mut json = serde_json::to_string(&reply).context("serialize reply")?;
    json.push('\n');
    stream.write_all(json.as_bytes()).context("write reply")?;
    stream.flush().context("flush reply")?;
    Ok(())
}

fn would_block(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock
}

/// Client side: connect to a running server, send one command, read one reply.
/// Used by `next-hunk push` and `next-hunk decision`.
pub fn send_command(socket_path: &Path, command: &ServerCommand) -> Result<ServerReply> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect to server socket {}", socket_path.display()))?;
    let mut json = serde_json::to_string(command).context("serialize command")?;
    json.push('\n');
    stream.write_all(json.as_bytes()).context("send command")?;
    stream.flush().context("flush command")?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).context("read reply")?;
    let reply: ServerReply =
        serde_json::from_str(line.trim()).map_err(|e| anyhow::anyhow!("parse reply: {e}"))?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::NoteTarget;

    /// A unique temp socket path per test, cleaned up on drop.
    struct TempSocket {
        path: PathBuf,
    }
    impl TempSocket {
        fn new(label: &str) -> Self {
            // Unique-ish: counter + pid + label. Avoids pulling in tempfile.
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "nh-test-{}-{}-{}.sock",
                n,
                std::process::id(),
                label
            ));
            let _ = std::fs::remove_file(&path);
            Self { path }
        }
    }
    impl Drop for TempSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    impl AsRef<Path> for TempSocket {
        fn as_ref(&self) -> &Path {
            &self.path
        }
    }

    #[test]
    fn spawn_then_push_gets_ok_reply() {
        let sock = TempSocket::new("push");
        let listener = ServerListener::spawn(sock.path.clone()).unwrap();
        // Simulate the main loop: drain the request and reply Ok.
        let drainer = std::thread::spawn(move || {
            let reqs = listener.drain();
            // loop until one arrives (drain is non-blocking)
            let mut reqs = reqs;
            while reqs.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                reqs = listener.drain();
            }
            for r in reqs {
                let _ = r.reply.send(ServerReply::Ok);
            }
        });

        let reply = send_command(
            &sock.path,
            &ServerCommand::Push {
                focus: None,
                notes: vec![Note {
                    target: NoteTarget::Banner,
                    text: "hi".into(),
                }],
            },
        )
        .unwrap();
        assert!(matches!(reply, ServerReply::Ok));
        drainer.join().unwrap();
    }

    #[test]
    fn decision_returns_selections() {
        let sock = TempSocket::new("decision");
        let listener = ServerListener::spawn(sock.path.clone()).unwrap();
        let selections = Selections {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec![],
            undecided: vec!["b.rs:h1".into()],
        };
        let want = selections.clone();
        let drainer = std::thread::spawn(move || {
            let mut got = listener.drain();
            while got.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                got = listener.drain();
            }
            for r in got {
                let _ = r.reply.send(ServerReply::Decisions(want.clone()));
            }
        });

        let reply = send_command(&sock.path, &ServerCommand::Decision).unwrap();
        match reply {
            ServerReply::Decisions(s) => {
                assert_eq!(s.accepted, selections.accepted);
                assert_eq!(s.undecided, selections.undecided);
            }
            other => panic!("expected Decisions, got {other:?}"),
        }
        drainer.join().unwrap();
    }

    #[test]
    fn stale_socket_is_reclaimed() {
        // A leftover file at the path must not block spawn; it should be
        // detected as stale (connect fails) and unlinked.
        let sock = TempSocket::new("stale");
        std::fs::write(&sock.path, b"not a socket").unwrap();
        // connect to a non-socket file errors → treated as stale.
        let _listener =
            ServerListener::spawn(sock.path.clone()).expect("spawn should reclaim the stale file");
    }

    #[test]
    fn second_spawn_errors_already_running() {
        let sock = TempSocket::new("double");
        let _first = ServerListener::spawn(sock.path.clone()).unwrap();
        // A second bind at the same path: connect succeeds (live server), so
        // spawn must error with "already running".
        let err = ServerListener::spawn(sock.path.clone()).unwrap_err();
        assert!(
            err.to_string().contains("already running"),
            "expected already-running error, got: {err}"
        );
    }

    #[test]
    fn drop_unlinks_socket() {
        let sock = TempSocket::new("drop");
        let path = sock.path.clone();
        {
            let _listener = ServerListener::spawn(path.clone()).unwrap();
            assert!(path.exists(), "socket should exist while listening");
        }
        // After the listener drops, the file should be gone.
        assert!(
            !path.exists(),
            "socket should be unlinked after drop, but still exists"
        );
    }

    #[test]
    fn info_returns_metadata() {
        let sock = TempSocket::new("info");
        let listener = ServerListener::spawn(sock.path.clone()).unwrap();
        let drainer = std::thread::spawn(move || {
            let mut got = listener.drain();
            while got.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                got = listener.drain();
            }
            for r in got {
                let _ = r.reply.send(ServerReply::Info {
                    repo_path: "/tmp/repo".into(),
                    file_count: 3,
                });
            }
        });

        let reply = send_command(&sock.path, &ServerCommand::Info).unwrap();
        match reply {
            ServerReply::Info {
                repo_path,
                file_count,
            } => {
                assert_eq!(repo_path, "/tmp/repo");
                assert_eq!(file_count, 3);
            }
            other => panic!("expected Info, got {other:?}"),
        }
        drainer.join().unwrap();
    }
}
