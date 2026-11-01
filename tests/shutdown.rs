#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Kills the child on drop so a failing test never leaks a process.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts the server, sends `signum` once it logs `listening`, and checks the graceful exit.
fn shuts_down_on(signum: &str, name: &str) {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sous"));
    cmd.env("SOUS_DATABASE", dir.path().join("sous.db"))
        .env("SOUS_ADDR", "127.0.0.1:0")
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .env_remove("APP_TIMEZONE")
        .stdout(Stdio::piped());
    let mut child = Guard(cmd.spawn().unwrap());
    let stdout = child.0.stdout.take().unwrap();

    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut rest = String::new();
        let mut listening = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if listening {
                rest.push_str(&line);
                rest.push('\n');
            } else if line.contains("listening") {
                listening = true;
                tx.send(()).unwrap();
            }
        }
        rest
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("server did not log `listening` within 30 s");

    let pid = child.0.id().to_string();
    let sent = Command::new("kill").args([signum, &pid]).status().unwrap();
    assert!(sent.success());

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "server did not exit within 5 s");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "exit status: {status}");
    assert_eq!(status.code(), Some(0));

    let output = reader.join().unwrap();
    assert!(output.contains("shutting down"), "output: {output}");
    assert!(output.contains(name), "output: {output}");
    assert!(output.contains("stopped"), "output: {output}");
}

#[test]
fn sigterm_shuts_down_gracefully_with_status_0() {
    shuts_down_on("-TERM", "SIGTERM");
}

#[test]
fn sigint_shuts_down_gracefully_with_status_0() {
    shuts_down_on("-INT", "SIGINT");
}
