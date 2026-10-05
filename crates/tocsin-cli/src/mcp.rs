//! `--mcp`: tocsin as an MCP server on standard input and output.
//!
//! The agent gets a `notify` tool to tell the user that a task is done or that
//! it is stuck. What it can choose is the title, the message and the kind. It
//! cannot choose where the notification goes: the destinations are the URLs the
//! user gave, which hold tokens and are never shown to the agent, so an agent
//! that was led astray by what it read cannot send anything anywhere else. The
//! number of notifications is limited too, so that it cannot flood a phone.
//!
//! The protocol is the part of MCP that a server of tools needs: JSON-RPC 2.0,
//! one message per line, `initialize`, `ping`, `tools/list` and `tools/call`.

use std::{
    collections::VecDeque,
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tocsin::{Kind, Notification, Notifier, Outcome, Transport, TransportError};

/// The protocol versions this server speaks, newest first. A client that asks
/// for another one is answered with the first.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The longest title and message, in characters.
const MAX_TITLE_CHARS: usize = 200;
const MAX_BODY_CHARS: usize = 4000;

/// At most this many notifications in this time.
const RATE: usize = 10;
const WINDOW: Duration = Duration::from_secs(60);

/// The longest message that is read from the client, in bytes.
const MAX_LINE_BYTES: usize = 1 << 20;

const INSTRUCTIONS: &str = "Sends a notification to the services that the user has set up (a \
phone, a chat). Use `notify` when a long task is done or when you are stuck and need the user. \
Keep it short and never put secrets or code in it: notification services are not private. You \
cannot choose where it goes.";

/// The state of one MCP session.
pub(crate) struct Server<'a, T> {
    notifier: &'a Notifier,
    transport: &'a mut T,
    sent: VecDeque<Instant>,
}

/// What reading one line gave.
enum Line {
    Eof,
    Message,
    TooLong,
}

impl<'a, T: Transport> Server<'a, T> {
    pub(crate) fn new(notifier: &'a Notifier, transport: &'a mut T) -> Self {
        Self {
            notifier,
            transport,
            sent: VecDeque::new(),
        }
    }

    /// Answer every line of `input` on `output` until the input ends.
    pub(crate) fn run(
        &mut self,
        mut input: impl BufRead,
        mut output: impl Write,
    ) -> io::Result<()> {
        let mut line = Vec::new();
        loop {
            let reply = match read_line(&mut input, &mut line)? {
                Line::Eof => return Ok(()),
                Line::TooLong => Some(error(&Value::Null, -32600, "The message is too long")),
                Line::Message => match std::str::from_utf8(&line) {
                    Ok(text) if text.trim().is_empty() => None,
                    Ok(text) => self.handle(text, Instant::now()),
                    Err(_) => Some(error(&Value::Null, -32700, "Parse error")),
                },
            };
            if let Some(reply) = reply {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
    }

    /// The reply to one message, if it needs one.
    pub(crate) fn handle(&mut self, text: &str, now: Instant) -> Option<Value> {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return Some(error(&Value::Null, -32700, "Parse error"));
        };
        // A batch is an array. The current protocol has none.
        let Some(object) = message.as_object() else {
            return Some(error(&Value::Null, -32600, "Batches are not supported"));
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // An answer to something this server never asked.
            return None;
        };
        let Some(id) = object.get("id") else {
            // A notification gets no answer.
            return None;
        };
        if !(id.is_string() || id.is_number()) {
            return Some(error(
                &Value::Null,
                -32600,
                "The id is neither a string nor a number",
            ));
        }
        let params = object.get("params").unwrap_or(&Value::Null);
        Some(match method {
            "initialize" => ok(id, &initialize(params)),
            "ping" => ok(id, &json!({})),
            "tools/list" => ok(id, &tools()),
            "tools/call" => match self.call(params, now) {
                Ok(result) => ok(id, &result),
                Err(message) => error(id, -32602, &message),
            },
            _ => error(id, -32601, "Method not found"),
        })
    }

    /// `tools/call`: a protocol error for a tool that does not exist, and a
    /// result with `isError` for a call that failed.
    fn call(&mut self, params: &Value, now: Instant) -> Result<Value, String> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = params.get("arguments").unwrap_or(&Value::Null);
        match name {
            "notify" => Ok(self.notify(arguments, now)),
            "destinations" => {
                let names: Vec<_> = self
                    .notifier
                    .services()
                    .iter()
                    .map(tocsin::Service::name)
                    .collect();
                Ok(text_result(&names.join(", "), false))
            }
            _ => Err(format!("Unknown tool: {name}")),
        }
    }

    fn notify(&mut self, arguments: &Value, now: Instant) -> Value {
        let notification = match notification_from(arguments) {
            Ok(notification) => notification,
            Err(message) => return text_result(&message, true),
        };
        while self
            .sent
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= WINDOW)
        {
            self.sent.pop_front();
        }
        if self.sent.len() >= RATE {
            return text_result(
                &format!(
                    "Too many notifications: at most {RATE} a minute. Wait before sending more, \
                     and put what is left in one message."
                ),
                true,
            );
        }
        self.sent.push_back(now);

        let report = self.notifier.send(&notification, self.transport);
        let mut failures = Vec::new();
        for receipt in report.failures() {
            // The reasons never hold the URL, only what went wrong.
            let reason = match &receipt.outcome {
                Outcome::Failed(error) => error.to_string(),
                _ => TransportError::InvalidRequest.to_string(),
            };
            failures.push(format!("{}: {reason}", receipt.service));
        }
        let total = report.receipts().len();
        if total == 0 {
            return text_result("Nothing was sent: the destinations have no target.", true);
        }
        if failures.is_empty() {
            return text_result(&format!("Sent {total} message(s)."), false);
        }
        text_result(
            &format!(
                "Sent {} of {total} message(s). Failed: {}",
                total - failures.len(),
                failures.join("; ")
            ),
            true,
        )
    }
}

/// The notification the arguments of a `notify` call describe, or what is wrong.
fn notification_from(arguments: &Value) -> Result<Notification, String> {
    let text = |key: &str| arguments.get(key).and_then(Value::as_str);
    let body = text("body")
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .ok_or("`body` is required and must be a non-empty string")?;
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!("`body` is longer than {MAX_BODY_CHARS} characters"));
    }
    let kind = match text("kind") {
        None | Some("info") => Kind::Info,
        Some("success") => Kind::Success,
        Some("warning") => Kind::Warning,
        Some("failure") => Kind::Failure,
        Some(_) => return Err("`kind` is one of info, success, warning, failure".to_owned()),
    };
    let mut notification = Notification::new(body).kind(kind);
    if let Some(title) = text("title")
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(format!(
                "`title` is longer than {MAX_TITLE_CHARS} characters"
            ));
        }
        notification = notification.title(title);
    }
    Ok(notification)
}

fn initialize(params: &Value) -> Value {
    let wanted = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOLS
        .iter()
        .find(|version| Some(**version) == wanted)
        .unwrap_or(&PROTOCOLS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "tocsin", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

fn tools() -> Value {
    json!({"tools": [
        {
            "name": "notify",
            "title": "Send a notification",
            "description": "Send a notification to the services the user has set up (a phone, a \
    chat). Use it when a long task is done or when you are stuck and need the user. Keep it short and \
    never include secrets or code: notification services are not private. You cannot choose where it \
    goes, and at most 10 notifications a minute are sent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "body": {"type": "string", "description": "The message."},
                    "title": {"type": "string", "description": "A short title."},
                    "kind": {
                        "type": "string",
                        "enum": ["info", "success", "warning", "failure"],
                        "description": "What the message is about. Info when it is not given.",
                    },
                },
                "required": ["body"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true,
            },
        },
        {
            "name": "destinations",
            "title": "List the destinations",
            "description": "The kinds of service the notifications go to, such as telegram or \
    ntfy. Only the names: the addresses and tokens are never shown.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "openWorldHint": false},
        },
    ]})
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn ok(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Read up to the next newline into `line`. A line longer than
/// [`MAX_LINE_BYTES`] is read to its end and thrown away.
fn read_line(input: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Line> {
    line.clear();
    let mut too_long = false;
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if line.is_empty() {
                Line::Eof
            } else {
                Line::Message
            });
        }
        let end = chunk.iter().position(|byte| *byte == b'\n');
        let taken = end.unwrap_or(chunk.len());
        if !too_long {
            if line.len() + taken > MAX_LINE_BYTES {
                too_long = true;
                line.clear();
            } else {
                line.extend_from_slice(&chunk[..taken]);
            }
        }
        input.consume(end.map_or(taken, |at| at + 1));
        if end.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Message
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use tocsin::MockTransport;

    use super::*;

    fn notifier() -> Notifier {
        let mut notifier = Notifier::new();
        notifier.add("json://127.0.0.1:1/hook").expect("a URL");
        notifier.add("ntfy://topic").expect("a URL");
        notifier
    }

    fn ask(server: &mut Server<'_, MockTransport>, request: &Value, now: Instant) -> Value {
        server.handle(&request.to_string(), now).expect("a reply")
    }

    fn call(name: &str, arguments: &Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
               "params": {"name": name, "arguments": arguments}})
    }

    #[test]
    fn the_handshake_names_a_known_version_and_the_tools() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let now = Instant::now();
        let reply = ask(
            &mut server,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": "2025-03-26"}}),
            now,
        );
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(reply["result"]["serverInfo"]["name"], "tocsin");
        // A version this server does not know is answered with its own newest.
        let reply = ask(
            &mut server,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
                    "params": {"protocolVersion": "1999-01-01"}}),
            now,
        );
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOLS[0]);
        let tools = ask(
            &mut server,
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
            now,
        );
        let names: Vec<_> = tools["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, ["notify", "destinations"]);
        // Nothing in the schema lets the agent name a destination.
        let schema =
            serde_json::to_string(&tools["result"]["tools"][0]["inputSchema"]).expect("json");
        assert!(!schema.contains("url"), "{schema}");
    }

    #[test]
    fn notifications_and_answers_get_no_reply() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let now = Instant::now();
        assert!(
            server
                .handle(
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                    now
                )
                .is_none()
        );
        assert!(
            server
                .handle(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#, now)
                .is_none()
        );
    }

    #[test]
    fn bad_messages_get_protocol_errors() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let now = Instant::now();
        let mut code =
            |text: &str| server.handle(text, now).expect("a reply")["error"]["code"].clone();
        assert_eq!(code("not json"), -32700);
        assert_eq!(
            code(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#),
            -32600
        );
        assert_eq!(code(r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#), -32600);
        assert_eq!(code(r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#), -32601);
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rm"}}"#),
            -32602
        );
    }

    #[test]
    fn notify_sends_to_every_destination_and_says_so() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let reply = ask(
            &mut server,
            &call(
                "notify",
                &json!({"body": "Done", "title": "Build", "kind": "success"}),
            ),
            Instant::now(),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        assert_eq!(reply["result"]["content"][0]["text"], "Sent 2 message(s).");
        assert_eq!(transport.requests.len(), 2);
        let sent = transport.requests[0].body.text().into_owned();
        assert!(sent.contains("Done") && sent.contains("Build"), "{sent}");
    }

    #[test]
    fn a_call_with_wrong_arguments_is_an_error_result_and_sends_nothing() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let now = Instant::now();
        for arguments in [
            json!({}),
            json!({"body": "   "}),
            json!({"body": 5}),
            json!({"body": "x", "kind": "panic"}),
            json!({"body": "x".repeat(MAX_BODY_CHARS + 1)}),
            json!({"body": "x", "title": "t".repeat(MAX_TITLE_CHARS + 1)}),
        ] {
            let reply = ask(&mut server, &call("notify", &arguments), now);
            assert_eq!(reply["result"]["isError"], true, "{arguments}");
        }
        // An address the agent made up is not a field, so it is not read.
        let reply = ask(
            &mut server,
            &call(
                "notify",
                &json!({"body": "x", "url": "json://attacker.example/x"}),
            ),
            now,
        );
        assert_eq!(reply["result"]["isError"], false);
        assert!(
            transport
                .requests
                .iter()
                .all(|request| !request.url.expose().contains("attacker"))
        );
    }

    #[test]
    fn at_most_ten_notifications_go_out_in_a_minute() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let start = Instant::now();
        for _ in 0..RATE {
            let reply = ask(&mut server, &call("notify", &json!({"body": "x"})), start);
            assert_eq!(reply["result"]["isError"], false);
        }
        let reply = ask(&mut server, &call("notify", &json!({"body": "x"})), start);
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .starts_with("Too many")
        );
        // A minute later the window has moved on.
        let reply = ask(
            &mut server,
            &call("notify", &json!({"body": "x"})),
            start + WINDOW,
        );
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(transport.requests.len(), (RATE + 1) * 2);
    }

    #[test]
    fn destinations_name_the_services_and_never_the_addresses() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let reply = ask(
            &mut server,
            &call("destinations", &json!({})),
            Instant::now(),
        );
        assert_eq!(reply["result"]["content"][0]["text"], "json, ntfy");
    }

    #[test]
    fn a_long_line_is_thrown_away_and_the_next_one_is_read() {
        let notifier = notifier();
        let mut transport = MockTransport::default();
        let mut server = Server::new(&notifier, &mut transport);
        let mut input = vec![b'x'; MAX_LINE_BYTES + 10];
        input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        server.run(&input[..], &mut output).expect("run");
        let lines: Vec<Value> = String::from_utf8(output)
            .expect("utf-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32600);
        assert_eq!(lines[1]["result"], json!({}));
    }
}
