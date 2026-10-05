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
    for flag in [
        "--body",
        "--title",
        "--notification-type",
        "--attach",
        "--dry-run",
        "--mcp",
    ] {
        assert!(help.contains(flag), "{flag} missing from help");
    }
    assert!(help.contains("TOCSIN_URLS"));
}

/// A file with a name of its own, removed when it goes out of scope.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, content: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("tocsin-{}-{name}", std::process::id()));
        std::fs::write(&path, content).expect("write the file");
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn an_attached_file_is_sent_with_the_message() {
    let (port, worker) = loopback(200);
    let file = TempFile::new("note.txt", b"hello-attachment");
    let url = format!("ntfy://127.0.0.1:{port}/alerts");
    let output = tocsin()
        .args(["-b", "disk is full", "-a"])
        .arg(&file.0)
        .arg(&url)
        .output()
        .expect("run");
    let received = worker.join().expect("worker");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    let name = file
        .0
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(
        received.starts_with(&format!(
            "POST /alerts?filename={name}&message=disk+is+full HTTP/1.1\r\n"
        )),
        "{received}"
    );
    assert!(received.ends_with("hello-attachment"), "{received}");
}

#[test]
fn a_file_can_be_the_whole_message() {
    let file = TempFile::new("only.txt", b"x");
    let output = tocsin()
        .args(["--dry-run", "-a"])
        .arg(&file.0)
        .arg("ntfy://my-topic")
        .output()
        .expect("run");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "POST https://ntfy.sh\n");
}

#[test]
fn a_file_that_cannot_be_read_is_a_usage_error() {
    let output = tocsin()
        .args([
            "--dry-run",
            "-b",
            "hi",
            "-a",
            "no-such-file.bin",
            "ntfy://my-topic",
        ])
        .output()
        .expect("run");
    assert_eq!(code(&output), 2);
    assert!(
        text(&output.stderr).contains("cannot read no-such-file.bin"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_dry_run_is_not_a_failure_for_services_that_read_answers() {
    // Slack answers 200 to a message it did not accept, so tocsin reads the
    // answer; a dry run has none.
    let output = tocsin()
        .args(["--dry-run", "-b", "hi", "slack://xoxb-1234-1234-abc124/ops"])
        .output()
        .expect("run");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "POST https://slack.com\n");
    assert!(text(&output.stderr).contains("dry run"));
}

/// Run `command` with `input` on its standard input.
fn run_with_stdin(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write");
    child.wait_with_output().expect("wait")
}

const CLAUDE_STOP: &str = r#"{"hook_event_name":"Stop","cwd":"/home/me/proj","last_assistant_message":"FAKE_agent_text"}"#;

#[test]
fn a_claude_code_hook_is_read_from_standard_input() {
    let (port, worker) = loopback(200);
    let url = format!("json://127.0.0.1:{port}/hook");
    let mut command = tocsin();
    command.args(["--hook", "claude-code", &url]);
    let output = run_with_stdin(command, CLAUDE_STOP);
    let received = worker.join().expect("worker");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert!(received.contains("Claude Code finished"), "{received}");
    assert!(received.contains("proj"), "{received}");
    // What the agent wrote stays out unless it is asked for.
    assert!(!received.contains("FAKE_agent_text"), "{received}");

    let (port, worker) = loopback(200);
    let url = format!("json://127.0.0.1:{port}/hook");
    let mut command = tocsin();
    command.args(["--hook", "claude-code", "--include-message", &url]);
    let output = run_with_stdin(command, CLAUDE_STOP);
    let received = worker.join().expect("worker");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert!(received.contains("FAKE_agent_text"), "{received}");
}

#[test]
fn a_codex_hook_is_the_last_argument_and_explicit_options_win() {
    let (port, worker) = loopback(200);
    let url = format!("json://127.0.0.1:{port}/hook");
    let payload =
        r#"{"type":"agent-turn-complete","cwd":"/work/api","last-assistant-message":"x"}"#;
    let output = tocsin()
        .args(["--hook", "codex", "-t", "mine", &url, payload])
        .output()
        .expect("run");
    let received = worker.join().expect("worker");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert!(received.contains("mine"), "{received}");
    assert!(received.contains("api"), "{received}");
    assert!(!received.contains("Codex finished"), "{received}");
}

#[test]
fn a_hook_never_exits_with_two() {
    // A bad URL and a bad payload are usage errors (2) outside a hook, and
    // 2 blocks the action in a Claude Code hook.
    let mut command = tocsin();
    command.args(["--hook", "claude-code", "nosuch://x"]);
    let output = run_with_stdin(command, CLAUDE_STOP);
    assert_eq!(code(&output), 1);
    assert!(text(&output.stderr).contains("URL 1"));

    let mut command = tocsin();
    command.args(["--hook", "claude-code", "-d", "ntfy://my-topic"]);
    let output = run_with_stdin(command, "not json");
    assert_eq!(code(&output), 1);
    assert!(text(&output.stderr).contains("not JSON"));

    // A dry run shows the host and sends nothing.
    let mut command = tocsin();
    command.args(["--hook", "claude-code", "-d", "ntfy://my-topic"]);
    let output = run_with_stdin(command, CLAUDE_STOP);
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "POST https://ntfy.sh\n");
}

#[test]
fn the_agents_text_cannot_be_asked_for_without_a_hook() {
    let output = tocsin()
        .args(["--include-message", "-b", "x", "ntfy://my-topic"])
        .output()
        .expect("run");
    assert_eq!(code(&output), 2);
}

#[test]
fn mcp_mode_notifies_only_through_the_urls_it_was_given() {
    let (port, worker) = loopback(200);
    let url = format!("json://127.0.0.1:{port}/hook");
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        // An address in the arguments is not a field of the tool, so it is not read.
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"notify","arguments":{"body":"Deploy finished","title":"CI","kind":"success","url":"json://attacker.example/x"}}}"#,
        "",
    ]
    .join("\n");
    let output = run_with_stdin(
        {
            let mut command = tocsin();
            command.args(["--mcp", &url]);
            command
        },
        &input,
    );
    let received = worker.join().expect("worker");
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert!(
        received.starts_with("POST /hook HTTP/1.1\r\n"),
        "{received}"
    );
    assert!(received.contains("Deploy finished"));
    assert!(!received.contains("attacker"));
    let replies: Vec<serde_json::Value> = text(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON message per line"))
        .collect();
    // The notification `initialized` gets no answer.
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "tocsin");
    assert_eq!(replies[1]["id"], 2);
    assert_eq!(replies[1]["result"]["isError"], false, "{}", replies[1]);
    // Nothing but protocol goes to standard output, and the URL goes nowhere.
    let shown = format!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(!shown.contains(&url));
}

#[test]
fn mcp_mode_needs_urls_and_takes_no_message() {
    let output = tocsin().arg("--mcp").output().expect("run");
    assert_eq!(code(&output), 2);
    assert!(text(&output.stderr).contains("no URL"));
    let output = tocsin()
        .args(["--mcp", "-b", "hello", "ntfy://my-topic"])
        .output()
        .expect("run");
    assert_eq!(code(&output), 2);
}
