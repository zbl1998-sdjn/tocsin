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
//! A bot can send files too, which Slack takes in three steps: ask for an upload
//! address (`files.getUploadURLExternal`), send the file there, and share it into
//! every channel the message went to (`files.completeUploadExternal`). The
//! channels are the ones Slack names in its answers to the messages, so this only
//! works through [`Service::plan`](crate::Service::plan), and the files go with
//! the first part of a long message. Unlike Apprise, the bot token is not sent
//! to the upload address.
//!
//! Slack answers `200` to a message it did not accept (`{"ok": false}` from the
//! Web API) and to a webhook that is wrong in some ways, so a plan reads the
//! answer too, as Apprise does: a webhook has to answer `ok`, the Web API has to
//! say `"ok": true`.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::LazyLock,
};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Attachment, Format, Kind, Method, Notification, ParseError, Plan, PreparedRequest,
    RequestPolicy, Response, SecretString, TransportError, grammar, message,
    multipart::Multipart,
    options::{FormatMode, Options},
    plan::{Check, Lookups, Next, Sequence, Single, Step},
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

/// The upload address answers `OK - <size>`.
fn upload_said_ok(response: &Response) -> bool {
    response.body.expose().windows(2).any(|pair| pair == b"OK") || api_said_ok(response)
}

/// `files.completeUploadExternal` has to say `ok` and name the files it shared.
fn shared_the_files(response: &Response) -> bool {
    let Ok(answer) = serde_json::from_slice::<Value>(response.body.expose()) else {
        return false;
    };
    answer.get("ok").and_then(Value::as_bool) == Some(true)
        && answer
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| !files.is_empty())
}

/// The channel or conversation that a message was posted to.
fn read_channel(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    let channel = answer.get("channel")?.as_str()?;
    (!channel.is_empty()).then(|| channel.to_owned())
}

/// The file id and the address to send the file to.
fn read_upload(response: &Response) -> Option<(String, String)> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    let id = answer.get("file_id")?.as_str()?;
    let address = answer.get("upload_url")?.as_str()?;
    (!id.is_empty() && !address.is_empty()).then(|| (id.to_owned(), address.to_owned()))
}

/// The user id in the answer to `users.lookupByEmail`.
fn read_user_id(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    Some(answer.get("user")?.get("id")?.as_str()?.to_owned())
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
        .map(|s| grammar::channel_list(&grammar::decode(s)))
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
        targets.extend(grammar::channel_list(&to));
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
        self.requests(options, notification).0
    }

    /// The requests, and how many of them belong to the first part of the
    /// message.
    fn requests(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> (Vec<PreparedRequest>, usize) {
        let url = self.url();
        let mut requests = Vec::new();
        let mut first_part = None;
        for (part, (title, body)) in message::parts(notification, 250, 35000, options.overflow)
            .into_iter()
            .enumerate()
        {
            if part == 1 {
                first_part = Some(requests.len());
            }
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
        let first_part = first_part.unwrap_or(requests.len());
        (requests, first_part)
    }

    /// How to tell from an answer that Slack accepted a message.
    fn check(&self) -> Option<Check> {
        match self.mode {
            Mode::Hook | Mode::GovHook => Some(webhook_said_ok),
            Mode::Bot => Some(api_said_ok),
            Mode::Workflow | Mode::Trigger => None,
        }
    }

    /// The posts to users that are only known by e-mail address, as the address,
    /// what is posted and whether it is part of the first message, and how many
    /// targets cannot be used at all.
    fn mail_targets(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> (Vec<(String, Value, bool)>, usize) {
        let mut by_mail = Vec::new();
        let mut unusable = 0;
        for (part, (title, body)) in message::parts(notification, 250, 35000, options.overflow)
            .into_iter()
            .enumerate()
        {
            for channel in self.channels.iter().flatten() {
                if let Some(address) = grammar::email_of(channel) {
                    if self.mode == Mode::Bot {
                        let payload = self.payload(options, notification, &title, &body);
                        by_mail.push((address, payload, part == 0));
                    } else {
                        // Only a bot can look a user up.
                        unusable += 1;
                    }
                } else if !CHANNEL.is_match(channel) {
                    unusable += 1;
                }
            }
        }
        (by_mail, unusable)
    }

    /// [`prepare`](Self::prepare) with every answer read, the failures Apprise
    /// reports for targets that cannot be used, and, for a bot, the lookup of
    /// the users that were given as e-mail addresses and the upload of files.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        let check = self.check();
        let (requests, first_part) = self.requests(options, notification);
        if matches!(self.mode, Mode::Workflow | Mode::Trigger) {
            return Plan::checked(requests, check);
        }
        // Only a bot can send files, and Apprise ignores them for a webhook.
        let files: Vec<Attachment> = if self.mode == Mode::Bot {
            notification.carried(options.overflow).to_vec()
        } else {
            Vec::new()
        };
        let (by_mail, unusable) = self.mail_targets(options, notification);
        // Without files every request is independent. With files, the posts
        // have to be a sequence, because their answers say where the files go.
        let (independent, sequenced) = if files.is_empty() {
            (requests, Vec::new())
        } else {
            (Vec::new(), requests)
        };
        let mut plan = Plan::checked(independent, check);
        for _ in 0..unusable {
            plan = plan.failed(TransportError::InvalidRequest);
        }
        let Some(token) = self.access_token.clone().filter(|_| self.mode == Mode::Bot) else {
            return plan;
        };
        if by_mail.is_empty() && files.is_empty() {
            return plan;
        }
        let url = self.url();
        let policy = options.policy();
        let bearer = BearerToken(token);
        // What goes out once every address is known.
        let deliveries = {
            let bearer = bearer.clone();
            move |found: &HashMap<String, String>| -> Vec<Box<dyn Sequence>> {
                let mail = by_mail
                    .into_iter()
                    .filter_map(|(address, mut payload, first)| {
                        payload["channel"] = json!(found.get(&address)?);
                        let mut request = PreparedRequest::json(&url, &payload).with_policy(policy);
                        request
                            .headers
                            .insert("Authorization".to_owned(), bearer.header());
                        Some((Step::delivery(request).checked(Some(api_said_ok)), first))
                    });
                if files.is_empty() {
                    return Single::each(mail.map(|(step, _)| step).collect());
                }
                // The first message's posts tell where the files go.
                let direct = sequenced.into_iter().enumerate().map(|(index, request)| {
                    (Step::delivery(request).checked(check), index < first_part)
                });
                let posts: VecDeque<(Step, bool)> = direct.chain(mail).collect();
                if posts.is_empty() {
                    return Vec::new();
                }
                vec![Box::new(Posts {
                    queue: posts,
                    collect: false,
                    channels: Vec::new(),
                    files,
                    bearer,
                    policy,
                })]
            }
        };
        let mut addresses: Vec<String> = Vec::new();
        // The deliveries are built for the addresses that were found, so the
        // list has to be taken before they are.
        for channel in self.channels.iter().flatten() {
            if let Some(address) = grammar::email_of(channel) {
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
        }
        if addresses.is_empty() || self.mode != Mode::Bot {
            for sequence in deliveries(&HashMap::new()) {
                plan = plan.then(sequence);
            }
            return plan;
        }
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
                    .insert("Authorization".to_owned(), bearer.header());
                Step::setup(request).checked(Some(api_said_ok))
            },
            read_user_id,
            deliveries,
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

/// A bot token, and the header that carries it.
#[derive(Clone)]
struct BearerToken(SecretString);

impl BearerToken {
    fn header(&self) -> SecretString {
        SecretString::new(format!("Bearer {}", self.0.expose()))
    }
}

/// Every message of a bot that sends files: the messages go out, the channels
/// Slack names in its answers to the first one's posts are remembered, and then
/// every file is sent to them.
struct Posts {
    queue: VecDeque<(Step, bool)>,
    /// Whether the answer to the request handed out last names a channel.
    collect: bool,
    channels: Vec<String>,
    files: Vec<Attachment>,
    bearer: BearerToken,
    policy: RequestPolicy,
}

impl Posts {
    fn pop(&mut self) -> Option<Step> {
        let (step, collect) = self.queue.pop_front()?;
        self.collect = collect;
        Some(step)
    }
}

impl Sequence for Posts {
    fn start(&mut self) -> Step {
        self.pop()
            .expect("posts are planned only when there is something to post")
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        let mut unidentified = false;
        if let (true, Ok(response)) = (self.collect, result) {
            match read_channel(response) {
                Some(channel) if !self.channels.contains(&channel) => self.channels.push(channel),
                Some(_) => {}
                // The message is out, but the files have nowhere to go.
                None => unidentified = true,
            }
        }
        let next = match self.pop() {
            Some(step) => Next::go(step),
            None => Next::then(self.uploads()),
        };
        if unidentified {
            next.with_failure(TransportError::InvalidResponse)
        } else {
            next
        }
    }
}

impl Posts {
    /// One sequence for each file, once the channels are known.
    fn uploads(&mut self) -> Vec<Box<dyn Sequence>> {
        if self.channels.is_empty() {
            return Vec::new();
        }
        self.files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                Box::new(Upload {
                    stage: UploadStage::Address,
                    name: file.name_or_default(index + 1),
                    file: file.clone(),
                    channels: self.channels.iter().cloned().collect(),
                    bearer: self.bearer.clone(),
                    policy: self.policy,
                    file_id: String::new(),
                }) as Box<dyn Sequence>
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum UploadStage {
    Address,
    Upload,
    Share,
}

/// One file: ask for an address, send it there, share it into every channel.
struct Upload {
    stage: UploadStage,
    name: String,
    file: Attachment,
    /// The channels it is not shared into yet.
    channels: VecDeque<String>,
    bearer: BearerToken,
    policy: RequestPolicy,
    file_id: String,
}

impl Upload {
    /// The request for the file's upload address.
    fn address(&self) -> Step {
        let query = format!(
            "filename={}&length={}",
            grammar::form_encode(&self.name),
            self.file.len()
        );
        let mut request = PreparedRequest::with_body(
            Method::Get,
            format!("https://slack.com/api/files.getUploadURLExternal?{query}"),
            None,
            Vec::<u8>::new(),
        )
        .with_policy(self.policy);
        request
            .headers
            .insert("Authorization".to_owned(), self.bearer.header());
        Step::setup(request).checked(Some(api_said_ok))
    }

    /// The upload itself. The address is Slack's, but it has no use for the
    /// token.
    fn upload(&self, address: &str) -> Step {
        let (content_type, body) = Multipart::new()
            .file("file", &self.name, None, self.file.data())
            .finish();
        let request = PreparedRequest::with_body(Method::Post, address, Some(&content_type), body)
            .with_policy(self.policy);
        Step::setup(request).checked(Some(upload_said_ok))
    }

    /// The next channel to share the file into, if there is one.
    fn share(&mut self) -> Next {
        let Some(channel) = self.channels.pop_front() else {
            return Next::done();
        };
        let payload = json!({
            "files": [{"id": self.file_id, "title": self.name}],
            "channel_id": channel,
        });
        let mut request = PreparedRequest::json(
            "https://slack.com/api/files.completeUploadExternal",
            &payload,
        )
        .with_policy(self.policy);
        request
            .headers
            .insert("Authorization".to_owned(), self.bearer.header());
        Next::go(Step::delivery(request).checked(Some(shared_the_files)))
    }
}

impl Sequence for Upload {
    fn start(&mut self) -> Step {
        self.address()
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        match (self.stage, result) {
            (UploadStage::Address, Ok(response)) => match read_upload(response) {
                Some((id, address)) => {
                    self.file_id = id;
                    self.stage = UploadStage::Upload;
                    Next::go(self.upload(&address))
                }
                None => Next::fail(TransportError::InvalidResponse),
            },
            (UploadStage::Upload, Ok(_)) => {
                self.stage = UploadStage::Share;
                self.share()
            }
            (UploadStage::Share, _) => self.share(),
            // A failed request is already a failure of the plan, and the file
            // cannot go on without it.
            (_, Err(_)) => Next::done(),
        }
    }
}
