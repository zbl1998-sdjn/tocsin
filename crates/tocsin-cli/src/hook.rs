//! The messages of `--hook`: what a coding agent's hook sends to its command,
//! turned into a title and a body.
//!
//! Claude Code runs a hook command with a JSON object on standard input
//! (`hook_event_name`, `cwd`, and `message` or `last_assistant_message`
//! depending on the event) and Codex runs the `notify` program with one JSON
//! argument (`type`, `cwd`, `last-assistant-message`, ...). Both are described
//! in their documentation, linked from the README.

use clap::ValueEnum;
use serde_json::Value;
use tocsin::Kind;

/// The longest message that is added to a notification, in characters.
const MAX_MESSAGE_CHARS: usize = 280;

/// The agent whose hook payload is read.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Agent {
    /// A Claude Code command hook: JSON on standard input.
    ClaudeCode,
    /// The Codex `notify` program: JSON as the last argument.
    Codex,
}

/// What to send for one hook event.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Described {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) kind: Kind,
}

/// The title, body and kind of the notification for `payload`. The agent's own
/// text (`last_assistant_message`) is left out unless `include_message` is set,
/// because it can hold code or secrets and notification services are not
/// private.
pub(crate) fn describe(
    agent: Agent,
    payload: &str,
    include_message: bool,
) -> Result<Described, String> {
    let value: Value =
        serde_json::from_str(payload).map_err(|_| "the hook payload is not JSON".to_owned())?;
    if !value.is_object() {
        return Err("the hook payload is not a JSON object".to_owned());
    }
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let project = text("cwd").and_then(project_name);
    let with_project = |message: Option<String>| match (project, message) {
        (Some(project), Some(message)) => format!("{project}: {message}"),
        (Some(project), None) => project.to_owned(),
        (None, Some(message)) => message,
        (None, None) => "Turn complete".to_owned(),
    };
    let last = |key: &str| {
        text(key)
            .filter(|_| include_message)
            .map(|message| one_line(message, MAX_MESSAGE_CHARS))
            .filter(|message| !message.is_empty())
    };

    match agent {
        Agent::ClaudeCode => {
            let event = text("hook_event_name")
                .ok_or_else(|| "the hook payload has no hook_event_name".to_owned())?;
            let (title, kind, body) = match event {
                "Stop" => (
                    "Claude Code finished".to_owned(),
                    Kind::Success,
                    with_project(last("last_assistant_message")),
                ),
                "SubagentStop" => (
                    "Claude Code subagent finished".to_owned(),
                    Kind::Success,
                    with_project(last("last_assistant_message")),
                ),
                "StopFailure" => (
                    "Claude Code stopped on an error".to_owned(),
                    Kind::Failure,
                    with_project(None),
                ),
                // The text of this one is meant for the person, so it is sent.
                "Notification" => (
                    "Claude Code needs you".to_owned(),
                    Kind::Warning,
                    with_project(text("message").map(|m| one_line(m, MAX_MESSAGE_CHARS))),
                ),
                "SessionEnd" => (
                    "Claude Code session ended".to_owned(),
                    Kind::Info,
                    with_project(None),
                ),
                other => (
                    format!("Claude Code: {other}"),
                    Kind::Info,
                    with_project(None),
                ),
            };
            Ok(Described { title, body, kind })
        }
        Agent::Codex => {
            let kind = text("type").ok_or_else(|| "the hook payload has no type".to_owned())?;
            let (title, level) = if kind == "agent-turn-complete" {
                ("Codex finished".to_owned(), Kind::Success)
            } else {
                (format!("Codex: {kind}"), Kind::Info)
            };
            Ok(Described {
                title,
                body: with_project(last("last-assistant-message")),
                kind: level,
            })
        }
    }
}

/// The last part of a working directory, from a Unix or a Windows path.
fn project_name(cwd: &str) -> Option<&str> {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
}

/// The text on one line, cut to `max` characters with an ellipsis.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(payload: &str, include: bool) -> Described {
        describe(Agent::ClaudeCode, payload, include).expect("described")
    }

    #[test]
    fn a_finished_turn_names_the_project_and_keeps_the_agents_text_out() {
        let payload = r#"{"hook_event_name":"Stop","cwd":"/home/me/tocsin","last_assistant_message":"secret sk-123"}"#;
        assert_eq!(
            claude(payload, false),
            Described {
                title: "Claude Code finished".to_owned(),
                body: "tocsin".to_owned(),
                kind: Kind::Success,
            }
        );
        assert_eq!(claude(payload, true).body, "tocsin: secret sk-123");
    }

    #[test]
    fn a_notification_carries_its_own_message() {
        let payload = r#"{"hook_event_name":"Notification","cwd":"C:\\Users\\me\\my app\\","message":"Claude needs your permission to use Bash","notification_type":"permission_prompt"}"#;
        assert_eq!(
            claude(payload, false),
            Described {
                title: "Claude Code needs you".to_owned(),
                body: "my app: Claude needs your permission to use Bash".to_owned(),
                kind: Kind::Warning,
            }
        );
    }

    #[test]
    fn other_events_are_named_and_a_failure_is_a_failure() {
        assert_eq!(
            claude(r#"{"hook_event_name":"StopFailure"}"#, false),
            Described {
                title: "Claude Code stopped on an error".to_owned(),
                body: "Turn complete".to_owned(),
                kind: Kind::Failure,
            }
        );
        assert_eq!(
            claude(r#"{"hook_event_name":"PreCompact","cwd":"/x/y"}"#, false).title,
            "Claude Code: PreCompact"
        );
    }

    #[test]
    fn codex_gives_its_turn_in_one_argument() {
        let payload = r#"{"type":"agent-turn-complete","thread-id":"t","turn-id":"1","cwd":"/work/api","input-messages":["x"],"last-assistant-message":"Done.\nAll tests pass."}"#;
        let got = describe(Agent::Codex, payload, false).expect("described");
        assert_eq!(got.title, "Codex finished");
        assert_eq!(got.body, "api");
        let got = describe(Agent::Codex, payload, true).expect("described");
        assert_eq!(got.body, "api: Done. All tests pass.");
    }

    #[test]
    fn a_payload_that_is_no_event_is_an_error() {
        assert!(describe(Agent::ClaudeCode, "nope", false).is_err());
        assert!(describe(Agent::ClaudeCode, "[1]", false).is_err());
        assert!(describe(Agent::ClaudeCode, "{}", false).is_err());
        assert!(describe(Agent::Codex, r#"{"cwd":"/x"}"#, false).is_err());
    }

    #[test]
    fn a_long_message_is_cut_on_a_character_boundary() {
        let long = "é".repeat(400);
        let cut = one_line(&long, MAX_MESSAGE_CHARS);
        assert_eq!(cut.chars().count(), MAX_MESSAGE_CHARS);
        assert!(cut.ends_with('…'));
        assert_eq!(one_line("a  b\n\tc", 10), "a b c");
    }
}
