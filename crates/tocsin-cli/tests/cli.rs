//! End-to-end tests of the `tocsin` binary. Nothing leaves the machine: dry
//! runs do no I/O and real sends go to a server on the loopback interface.

use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Output, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_tocsin");

/// A `tocsin` command with a clean environment, so a developer's proxy or
/// `TOCSIN_URLS` cannot change the outcome.
fn tocsin() -> Command {
    let mut command = Command::new(BIN);
    command.stdin(Stdio::null());
    for name in [
        "TOCSIN_URLS",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

/// Answer one request with `status` and return the request as received.
fn loopback(status: u16) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    listener.set_nonblocking(true).expect("nonblocking");
    let worker = std::thread::spawn(move || {
        let started = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        started.elapsed() < Duration::from_secs(10),
                        "no loopback request"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        // On Windows the accepted stream inherits the listener's non-blocking mode.
        stream.set_nonblocking(false).expect("blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut data = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let count = stream.read(&mut buffer).expect("request read");
            assert!(count > 0, "early EOF");
            data.extend_from_slice(&buffer[..count]);
            if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break;
                }
            }
            assert!(data.len() < 16384, "request too large");
        }
        write!(
            stream,
            "HTTP/1.1 {status} Result\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("response");
        String::from_utf8(data).expect("request UTF-8")
    });
    (port, worker)
}

#[test]
fn dry_run_prints_the_host_and_sends_nothing() {
    let output = tocsin()
        .args(["--dry-run", "-b", "hello", "ntfy://my-topic"])
        .output()
        .expect("run");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "POST https://ntfy.sh\n");
}

#[test]
fn the_message_can_come_from_standard_input() {
    let mut child = tocsin()
        .args(["-d", "ntfy://my-topic"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"from a pipe")
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "POST https://ntfy.sh\n");
}

#[test]
fn urls_can_come_from_the_environment() {
    let output = tocsin()
        .env("TOCSIN_URLS", "ntfy://one ntfy://two")
        .args(["-d", "-b", "hello"])
        .output()
        .expect("run");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout).lines().count(), 2);
}

#[test]
fn missing_url_or_message_is_a_usage_error() {
    let output = tocsin().args(["-b", "hello"]).output().expect("run");
    assert_eq!(code(&output), 2);
    assert!(text(&output.stderr).contains("no URL"));

    // Standard input is empty here. An interactive terminal, which cannot be
    // simulated in a test, gets "no message" instead.
    let output = tocsin().args(["ntfy://my-topic"]).output().expect("run");
    assert_eq!(code(&output), 2);
    assert!(text(&output.stderr).contains("empty"));

    let output = tocsin()
        .args(["ntfy://my-topic", "-b", "  "])
        .output()
        .expect("run");
    assert_eq!(code(&output), 2);
    assert!(text(&output.stderr).contains("empty"));
}

#[test]
fn a_bad_url_is_rejected_without_echoing_it() {
    let secret = "FAKE_secret_token";
    for url in [format!("nosuch://{secret}"), format!("not a url {secret}")] {
        let output = tocsin()
            .args(["-d", "-b", "hello", &url])
            .output()
            .expect("run");
        assert_eq!(code(&output), 2);
        let shown = format!("{}{}", text(&output.stdout), text(&output.stderr));
        assert!(shown.contains("URL 1"), "{shown}");
        assert!(!shown.contains(secret), "{shown}");
    }
}

#[test]
fn a_url_with_nothing_to_send_fails() {
    let output = tocsin()
        .args(["-d", "-b", "hello", "ntfy://127.0.0.1/topic?auth=token"])
        .output()
        .expect("run");
    assert_eq!(code(&output), 1);
    assert!(text(&output.stderr).contains("ntfy: nothing to send"));
}

#[test]
fn a_delivery_reaches_the_server() {
    let (port, worker) = loopback(200);
    let secret = "FAKE_query_token";
    let url = format!("ntfy://127.0.0.1:{port}/alerts?token={secret}");
    let output = tocsin()
        .args(["-b", "disk is full", "-t", "server", "-n", "warning", &url])
        .output()
        .expect("run");
    let received = worker.join().expect("worker");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert!(received.starts_with("POST / HTTP/1.1\r\n"), "{received}");
    assert!(received.contains("disk is full"));
    assert!(received.contains(&format!("Bearer {secret}")));
    let shown = format!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(!shown.contains(secret));
}

#[test]
fn a_rejected_delivery_exits_with_one_and_names_the_status() {
    let (port, worker) = loopback(500);
    let secret = "FAKE_query_token";
    let url = format!("ntfy://127.0.0.1:{port}/alerts?token={secret}");
    let output = tocsin().args(["-b", "hi", &url]).output().expect("run");
    worker.join().expect("worker");
    assert_eq!(code(&output), 1);
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("ntfy: notification HTTP status 500"),
        "{stderr}"
    );
    assert!(!stderr.contains(secret));
}

#[test]
fn help_lists_the_options() {
    let output = tocsin().arg("--help").output().expect("run");
    assert_eq!(code(&output), 0);
    let help = text(&output.stdout);
    for flag in ["--body", "--title", "--notification-type", "--dry-run"] {
        assert!(help.contains(flag), "{flag} missing from help");
    }
    assert!(help.contains("TOCSIN_URLS"));
}
