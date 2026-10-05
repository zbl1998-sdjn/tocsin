//! Slack: `slack://<token a>/<token b>/<token c>[/<channel>...]` for an incoming
//! webhook, `slack://xoxb-.../<channel>...` for a bot token, and
//! `slack://<id>/<id>/<id>[/<id>]?mode=workflow|trigger` for workflow webhooks.
//!
//! Apprise's `template` option (a file of Slack blocks) is not supported. A bot
//! can also be given an e-mail address as a target: the user it belongs to is
//! looked up first (once for each address, before the first message), which is
//! why that only works through [`Service::plan`](crate::Service::plan);
//! [`Service::prepare`](crate::Service::prepare) skips those targets.
//!
//! Slack answers `200` to a message it did not accept (`{"ok": false}` from the
//! Web API) and to a webhook that is wrong in some ways, so a plan reads the
//! answer too, as Apprise does: a webhook has to answer `ok`, the Web API has to
//! say `"ok": true`.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Format, Kind, Method, Notification, ParseError, Plan, PreparedRequest, Response, SecretString,
    TransportError, grammar, message,
    options::{FormatMode, Options},
    plan::{Check, Lookups, Single, Step},
};

static TOKEN_A: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9]+$").expect("static regex"));
static TOKEN_C: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Za-z0-9]+$").expect("static regex"));
static ACCESS_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:xoxe\.)?xox[abp]-[A-Z0-9-]+$").expect("static regex"));
static CHANNEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<channel>[+#@]?[A-Z0-9_-]{1,32})(?::(?P<thread>[0-9.]+))?$")
        .expect("static regex")
});

/// Apprise's check for an e-mail address, which also reads `Name <address>` and
/// `label+address`. It only looks at the start of the text, like Python's
/// `re.match`. The group `full` is the address that is looked up.
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)^(?:(?:[\s"']{0,32})?(?P<name>[^:<'"]{0,128})?[:<\s'"]{1,32})?(?P<full>(?:(?P<label>[^+\s]{1,128})\+)?(?P<email>(?P<userid>[a-z0-9_!#$%&*/=?%`{|}~^-]+(?:\.[a-z0-9_!#$%&'*/=?%`{|}~^-]+)*)@(?P<domain>(?:(?:[a-z0-9](?:[a-z0-9_-]*[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9_-]*[a-z0-9]))|[a-z0-9][a-z0-9_-]{5,})))\s*>?"#,
    )
    .expect("static regex")
});

/// The ways Slack can be reached, in the order Apprise tries a prefix against.
const MODES: [Mode; 5] = [
    Mode::Hook,
    Mode::GovHook,
    Mode::Bot,
    Mode::Workflow,
    Mode::Trigger,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Hook,
    GovHook,
    Bot,
    Workflow,
    Trigger,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::GovHook => "gov-hook",
            Self::Bot => "bot",
            Self::Workflow => "workflow",
            Self::Trigger => "trigger",
        }
    }

    /// Apprise accepts any beginning of a mode name.
    fn from_prefix(prefix: &str) -> Option<Self> {
        MODES
            .into_iter()
            .find(|mode| mode.as_str().starts_with(prefix))
    }
}

/// The address to look up, if `target` is an e-mail address.
fn email_of(target: &str) -> Option<String> {
    EMAIL.captures(target).map(|found| found["full"].to_owned())
}

/// A webhook answers `ok` when it took the message.
fn webhook_said_ok(response: &Response) -> bool {
    response.body.expose() == b"ok"
}

/// The Web API answers `200` to everything and says whether it did what it was
/// asked in `"ok"`.
fn api_said_ok(response: &Response) -> bool {
    serde_json::from_slice::<Value>(response.body.expose())
        .ok()
        .and_then(|answer| answer.get("ok")?.as_bool())
        .unwrap_or(false)
}

/// The user id in the answer to `users.lookupByEmail`.
fn read_user_id(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    Some(answer.get("user")?.get("id")?.as_str()?.to_owned())
}

/// Apprise's `CHANNEL_LIST_DELIM`: also splits on `#`.
fn channel_list(text: &str) -> Vec<String> {
    text.split([' ', '\t', '\r', '\n', ',', '#', '\\', '/'])
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The text of a token, if there is one.
#[cfg(feature = "compat")]
fn exposed(token: Option<&SecretString>) -> Option<&str> {
    token.map(SecretString::expose)
}

/// A parsed `slack://` URL.
///
/// `image` and `timestamp` are parsed and reported, but the message carries no
/// picture (Apprise links one in its own repository) and no time stamp.
// One bool per URL flag, mirroring Apprise's option set.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Slack {
    mode: Mode,
    access_token: Option<SecretString>,
    token_a: Option<SecretString>,
    token_b: Option<SecretString>,
    token_c: Option<SecretString>,
    workflow_path: Vec<String>,
    /// `None` means the webhook's own channel.
    channels: Vec<Option<String>>,
    use_blocks: bool,
    image: bool,
    footer: bool,
    timestamp: bool,
    tokens: BTreeMap<String, String>,
}

/// The credentials a URL gave, before they are checked against the mode.
struct Found {
    access_token: Option<String>,
    token_a: Option<String>,
    token_b: Option<String>,
    token_c: Option<String>,
    workflow_path: Option<String>,
}

/// The credentials a mode uses; the others are cleared.
struct Checked {
    access_token: Option<String>,
    token_a: Option<String>,
    token_b: Option<String>,
    token_c: Option<String>,
    path: Vec<String>,
}

fn validated(value: Option<String>, pattern: &Regex) -> Result<String, ParseError> {
    value
        .and_then(|value| grammar::validate(pattern, &value))
        .ok_or(ParseError::InvalidToken)
}

fn check(mode: Mode, found: Found) -> Result<Checked, ParseError> {
    let mut checked = Checked {
        access_token: None,
        token_a: None,
        token_b: None,
        token_c: None,
        path: Vec::new(),
    };
    match mode {
        Mode::Workflow | Mode::Trigger => {
            checked.path = found
                .workflow_path
                .iter()
                .flat_map(|path| path.split('/'))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let wanted = if mode == Mode::Workflow { 4 } else { 3 };
            if checked.path.len() != wanted {
                return Err(ParseError::InvalidOption);
            }
        }
        Mode::Hook | Mode::GovHook => {
            checked.token_a = Some(validated(found.token_a, &TOKEN_A)?);
            checked.token_b = Some(validated(found.token_b, &TOKEN_A)?);
            checked.token_c = Some(validated(found.token_c, &TOKEN_C)?);
        }
        Mode::Bot => {
            checked.access_token = Some(validated(found.access_token, &ACCESS_TOKEN)?);
        }
    }
    Ok(checked)
}

/// Read the credentials and the targets from the host, the path and the
/// `token` argument, the way Apprise does.
fn locate(raw: &grammar::Raw) -> (Found, Vec<String>) {
    let token = raw.options.host.clone();
    let mut entries = raw.paths.clone().into_iter();
    let detected = raw
        .query
        .get("mode")
        .filter(|s| !s.is_empty())
        .and_then(|prefix| Mode::from_prefix(prefix));
    let mut found = Found {
        access_token: None,
        token_a: None,
        token_b: None,
        token_c: None,
        workflow_path: None,
    };
    let targets: Vec<String> = if matches!(detected, Some(Mode::Workflow | Mode::Trigger)) {
        found.workflow_path = Some(
            std::iter::once(token)
                .chain(entries)
                .collect::<Vec<_>>()
                .join("/"),
        );
        Vec::new()
    } else if token.starts_with("xo") {
        found.access_token = Some(token);
        entries.collect()
    } else {
        found.token_a = Some(token);
        found.token_b = entries.next();
        found.token_c = entries.next();
        entries.collect()
    };
    if let Some(list) = raw
        .query
        .get("token")
        .filter(|s| !s.is_empty())
        .map(|s| channel_list(&grammar::decode(s)))
    {
        let mut items = list.into_iter();
        let first = items.next();
        if first.as_deref().is_some_and(|f| f.starts_with("xo")) {
            found.access_token = first;
            (found.token_a, found.token_b, found.token_c) = (None, None, None);
        } else {
            found.access_token = None;
            found.token_a = first;
            found.token_b = items.next();
            found.token_c = items.next();
        }
    }
    (found, targets)
}

pub(crate) fn parse(input: &str) -> Result<(Options, Slack), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Supported(&[Format::Markdown, Format::Text]),
    )?;
    let argument = |key: &str| {
        raw.query
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| grammar::decode(s))
    };
    let (found, mut targets) = locate(&raw);
    if let Some(to) = argument("to") {
        targets.extend(channel_list(&to));
    }
    if raw.query.get("template").is_some_and(|s| !s.is_empty()) {
        return Err(ParseError::InvalidOption);
    }
    let explicit = argument("mode");
    let mode = match explicit.as_deref() {
        Some(prefix) => Mode::from_prefix(prefix).ok_or(ParseError::InvalidOption)?,
        None if found.workflow_path.is_some() => Mode::Workflow,
        None if found.access_token.is_some() => Mode::Bot,
        None => Mode::Hook,
    };
    let Checked {
        access_token,
        token_a,
        token_b,
        token_c,
        path,
    } = check(mode, found)?;
    let mut channels: Vec<Option<String>> = grammar::list(&targets.join(" "))
        .into_iter()
        .map(Some)
        .collect();
    if channels.is_empty() {
        channels.push((mode == Mode::Bot).then(|| "#general".to_owned()));
    }
    let blocks = raw.query.get("blocks").filter(|s| !s.is_empty());
    let slack = Slack {
        mode,
        access_token: access_token.map(SecretString::new),
        token_a: token_a.map(SecretString::new),
        token_b: token_b.map(SecretString::new),
        token_c: token_c.map(SecretString::new),
        workflow_path: path,
        channels,
        use_blocks: blocks.is_some_and(|value| grammar::bool_value(value)),
        image: raw.flag("image", true),
        footer: raw.flag("footer", true),
        timestamp: raw.flag("timestamp", true),
        tokens: raw.payload.to_map(),
    };
    Ok((raw.options, slack))
}

/// Python's `html.escape(text, quote=False)`.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Apprise's colors for each kind of notification.
fn color(kind: Kind) -> &'static str {
    match kind {
        Kind::Info => "#3AA3E3",
        Kind::Success => "#3AA337",
        Kind::Warning => "#CACF29",
        Kind::Failure => "#A32037",
    }
}

impl Slack {
    fn url(&self) -> String {
        let secret = |token: &Option<SecretString>| {
            token
                .as_ref()
                .map(SecretString::expose)
                .unwrap_or_default()
                .to_owned()
        };
        match self.mode {
            Mode::Hook => format!(
                "https://hooks.slack.com/services/{}/{}/{}",
                secret(&self.token_a),
                secret(&self.token_b),
                secret(&self.token_c)
            ),
            Mode::GovHook => format!(
                "https://hooks.slack-gov.com/services/{}/{}/{}",
                secret(&self.token_a),
                secret(&self.token_b),
                secret(&self.token_c)
            ),
            Mode::Bot => "https://slack.com/api/chat.postMessage".to_owned(),
            Mode::Workflow => {
                format!(
                    "https://hooks.slack.com/workflows/{}",
                    self.workflow_path.join("/")
                )
            }
            Mode::Trigger => {
                format!(
                    "https://hooks.slack.com/triggers/{}",
                    self.workflow_path.join("/")
                )
            }
        }
    }

    fn payload(
        &self,
        options: &Options,
        notification: &Notification,
        title: &str,
        body: &str,
    ) -> Value {
        let markdown = message::format(options, notification) == Format::Markdown;
        // Plain text must not be read as Slack markup; Markdown is sent as
        // written, without Apprise's conversion to Slack's own dialect.
        let body = if markdown {
            body.to_owned()
        } else {
            escape(body)
        };
        let mut payload = if self.use_blocks {
            let kind = if markdown { "mrkdwn" } else { "plain_text" };
            let mut blocks = Vec::new();
            if !title.is_empty() {
                blocks.push(json!({
                    "type": "header",
                    "text": {"type": "plain_text", "text": escape(title), "emoji": true},
                }));
            }
            blocks.push(json!({"type": "section", "text": {"type": kind, "text": body}}));
            if self.footer {
                blocks.push(json!({
                    "type": "context",
                    "elements": [{"type": kind, "text": "tocsin"}],
                }));
            }
            json!({"attachments": [{"blocks": blocks, "color": color(notification.kind)}]})
        } else {
            let mut attachment = json!({
                "title": escape(title),
                "text": body,
                "color": color(notification.kind),
            });
            if self.footer {
                attachment["footer"] = json!("tocsin");
            }
            json!({"mrkdwn": markdown, "attachments": [attachment]})
        };
        if let Some(user) = options.user.as_deref().filter(|u| !u.is_empty()) {
            payload["username"] = json!(user);
        }
        payload
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let url = self.url();
        let mut requests = Vec::new();
        for (title, body) in message::parts(notification, 250, 35000, options.overflow) {
            if matches!(self.mode, Mode::Workflow | Mode::Trigger) {
                let markdown = message::format(options, notification) == Format::Markdown;
                let body = if markdown { body } else { escape(&body) };
                let text = if title.is_empty() {
                    body
                } else {
                    format!("{title}: {body}")
                };
                requests.push(
                    PreparedRequest::json(&url, &json!({"text": text}))
                        .with_policy(options.policy()),
                );
                continue;
            }
            for channel in &self.channels {
                let mut payload = self.payload(options, notification, &title, &body);
                if let Some(channel) = channel {
                    let Some(found) = CHANNEL.captures(channel) else {
                        continue;
                    };
                    let name = &found["channel"];
                    payload["channel"] = json!(match name.chars().next() {
                        Some('+') => name[1..].to_owned(),
                        Some('@' | '#') => name.to_owned(),
                        _ => format!("#{name}"),
                    });
                    if let Some(thread) = found.name("thread") {
                        payload["thread_ts"] = json!(thread.as_str());
                    }
                }
                let mut request =
                    PreparedRequest::json(&url, &payload).with_policy(options.policy());
                if let Some(token) = &self.access_token {
                    request.headers.insert(
                        "Authorization".to_owned(),
                        SecretString::new(format!("Bearer {}", token.expose())),
                    );
                }
                requests.push(request);
            }
        }
        requests
    }

    /// How to tell from an answer that Slack accepted a message.
    fn check(&self) -> Option<Check> {
        match self.mode {
            Mode::Hook | Mode::GovHook => Some(webhook_said_ok),
            Mode::Bot => Some(api_said_ok),
            Mode::Workflow | Mode::Trigger => None,
        }
    }

    /// [`prepare`](Self::prepare) with every answer read, the failures Apprise
    /// reports for targets that cannot be used, and, for a bot, the lookup of
    /// the users that were given as e-mail addresses.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        let check = self.check();
        let mut plan = Plan::checked(self.prepare(options, notification), check);
        if matches!(self.mode, Mode::Workflow | Mode::Trigger) {
            return plan;
        }
        // Posts to a user that is only known by e-mail address, and who it is.
        let mut by_mail: Vec<(String, Value)> = Vec::new();
        for (title, body) in message::parts(notification, 250, 35000, options.overflow) {
            for channel in self.channels.iter().flatten() {
                if let Some(address) = email_of(channel) {
                    if self.mode == Mode::Bot {
                        let payload = self.payload(options, notification, &title, &body);
                        by_mail.push((address, payload));
                    } else {
                        // Only a bot can look a user up.
                        plan = plan.failed(TransportError::InvalidRequest);
                    }
                } else if !CHANNEL.is_match(channel) {
                    plan = plan.failed(TransportError::InvalidRequest);
                }
            }
        }
        let (Some(token), false) = (self.access_token.clone(), by_mail.is_empty()) else {
            return plan;
        };
        let mut addresses: Vec<String> = Vec::new();
        for (address, _) in &by_mail {
            if !addresses.contains(address) {
                addresses.push(address.clone());
            }
        }
        let url = self.url();
        let policy = options.policy();
        let bearer = move || SecretString::new(format!("Bearer {}", token.expose()));
        let lookup_bearer = bearer.clone();
        plan.then(Lookups::new(
            addresses,
            move |address| {
                let url = format!(
                    "https://slack.com/api/users.lookupByEmail?email={}",
                    grammar::form_encode(address)
                );
                let mut request =
                    PreparedRequest::with_body(Method::Get, url, None, Vec::<u8>::new())
                        .with_policy(policy);
                request
                    .headers
                    .insert("Authorization".to_owned(), lookup_bearer());
                Step::setup(request).checked(Some(api_said_ok))
            },
            read_user_id,
            move |found| {
                Single::each(
                    by_mail
                        .into_iter()
                        .filter_map(|(address, mut payload)| {
                            payload["channel"] = json!(found.get(&address)?);
                            let mut request =
                                PreparedRequest::json(&url, &payload).with_policy(policy);
                            request.headers.insert("Authorization".to_owned(), bearer());
                            Some(Step::delivery(request).checked(Some(api_said_ok)))
                        })
                        .collect(),
                )
            },
        ))
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
        for (key, value) in [
            ("mode", json!(self.mode.as_str())),
            ("access_token", json!(exposed(self.access_token.as_ref()))),
            ("token_a", json!(exposed(self.token_a.as_ref()))),
            ("token_b", json!(exposed(self.token_b.as_ref()))),
            ("token_c", json!(exposed(self.token_c.as_ref()))),
            ("workflow_path", json!(self.workflow_path)),
            ("channels", json!(self.channels)),
            ("use_blocks", json!(self.use_blocks)),
            ("image", json!(self.image)),
            ("footer", json!(self.footer)),
            ("timestamp", json!(self.timestamp)),
            ("tokens", json!(self.tokens)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::email_of;

    #[test]
    fn an_email_address_is_found_the_way_apprise_finds_it() {
        for (target, address) in [
            ("bob@example.com", Some("bob@example.com")),
            ("label+bob@example.com", Some("label+bob@example.com")),
            ("Bob <bob@example.com>", Some("bob@example.com")),
            ("user@localhost", Some("user@localhost")),
            ("#general", None),
            ("@bob", None),
            ("general", None),
            ("+C123", None),
            ("bob@x", None),
        ] {
            assert_eq!(email_of(target).as_deref(), address, "target: {target}");
        }
    }
}
