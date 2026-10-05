//! Telegram bot messages: `tgram://<bot_token>/<chat_id>[/<chat_id>...]`.
//!
//! Without a chat id the message goes to whoever last wrote to the bot, which
//! Telegram only tells through `getUpdates`. That is a request whose answer the
//! next one needs, so it works through [`Service::plan`](crate::Service::plan);
//! [`Service::prepare`](crate::Service::prepare) returns nothing for it. Send the
//! bot a message first, as Apprise asks you to.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;
use serde_json::{Number, Value, json};

use crate::{
    Format, Method, Notification, ParseError, Plan, PreparedRequest, Response, SecretString,
    grammar, message,
    options::{FormatMode, Options},
    plan::{Lookups, Step},
};

/// `tgram://[bot]<id>:<secret>/...` is rewritten to the generic grammar by
/// moving the bot id into the host, as Apprise does.
static REWRITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^tgram://(?:bot)?(?P<prefix>[a-z0-9_-]+(?::[a-z0-9_-]+)?@)?(?P<id>[0-9]+)(?::|%3A)+(?P<tail>.*)$",
    )
    .expect("static regex")
});
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[0-9]+:[a-z0-9_-]+$").expect("static regex"));
static TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(?P<number>-?[0-9]{1,32})|@?(?P<name>[a-z_-][a-z0-9_-]+))(?::(?P<topic>[0-9]+))?$",
    )
    .expect("static regex")
});

/// The name the detected chat is looked up under.
const OWNER: &str = "owner";

/// The id of the first user who wrote to the bot, in the answer to `getUpdates`.
/// Like Apprise, this looks at the first update that has a sender, and gives up
/// when that one has no usable id.
fn read_owner(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    if !answer.get("ok")?.as_bool()? {
        return None;
    }
    let sender = answer
        .get("result")?
        .as_array()?
        .iter()
        .find_map(|update| update.get("message")?.get("from"))?;
    let id = sender.get("id")?.as_number()?.to_string();
    (id != "0").then_some(id)
}

#[derive(Clone)]
enum Chat {
    /// A numeric chat id, kept as the exact digits.
    Id(Number),
    /// A public channel such as `@name`.
    Name(String),
}

impl Chat {
    fn to_json(&self) -> Value {
        match self {
            Self::Id(number) => Value::Number(number.clone()),
            Self::Name(name) => Value::String(name.clone()),
        }
    }
}

#[derive(Clone)]
struct Target {
    chat: Chat,
    topic: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkdownVersion {
    V1,
    V2,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Content {
    Before,
    After,
}

/// A parsed `tgram://` URL.
///
/// Options such as `detect`, `image`, `album` and `content` are parsed and
/// validated like Apprise does, but only the ones that shape the request are
/// applied.
// One bool per URL flag, mirroring Apprise's option set.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Telegram {
    bot_token: SecretString,
    targets: Vec<Target>,
    detect: bool,
    image: bool,
    silent: bool,
    preview: bool,
    album: bool,
    topic: Option<i64>,
    markdown: MarkdownVersion,
    content: Content,
    payload: BTreeMap<String, String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Telegram), ParseError> {
    let caps = REWRITE.captures(input).ok_or(ParseError::InvalidToken)?;
    let rewritten = format!(
        "tgram://{}{}/{}",
        caps.name("prefix").map_or("", |m| m.as_str()),
        &caps["id"],
        &caps["tail"]
    );
    let raw = grammar::parse(
        &rewritten,
        grammar::Hosts::Any,
        FormatMode::Supported(&[Format::Html, Format::Markdown]),
    )?;
    let token = format!(
        "{}:{}",
        &caps["id"],
        raw.paths.first().ok_or(ParseError::InvalidToken)?
    );
    if !TOKEN.is_match(&token) {
        return Err(ParseError::InvalidToken);
    }
    let topic = raw
        .query
        .get("topic")
        .or_else(|| raw.query.get("thread"))
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<i64>().map_err(|_| ParseError::InvalidOption))
        .transpose()?;
    let content = match raw
        .query
        .get("content")
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        None | Some("before") => Content::Before,
        Some("after") => Content::After,
        Some(_) => return Err(ParseError::InvalidOption),
    };
    let mut candidates: Vec<String> = raw.paths.iter().skip(1).cloned().collect();
    if let Some(to) = raw.query.get("to") {
        candidates.extend(grammar::list(to));
    }
    let candidates = grammar::list(&candidates.join(" "));
    let mut detect = raw.flag("detect", candidates.is_empty());
    let mut targets = Vec::new();
    for item in candidates {
        match parse_target(&item, topic)? {
            Some(target) => targets.push(target),
            None => detect = false,
        }
    }
    let mdv = raw
        .query
        .get("mdv")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let markdown = if mdv.starts_with("v2") || mdv.starts_with('2') || mdv.starts_with("default") {
        MarkdownVersion::V2
    } else {
        MarkdownVersion::V1
    };
    let telegram = Telegram {
        bot_token: SecretString::new(token),
        targets,
        detect,
        image: raw.flag("image", false),
        silent: raw.flag("silent", false),
        preview: raw.flag("preview", false),
        album: raw.flag("album", false),
        topic,
        markdown,
        content,
        payload: raw.payload.to_map(),
    };
    Ok((raw.options, telegram))
}

/// One chat target: `<id>`, `-<id>`, `@name`, optionally followed by `:<topic>`.
/// `None` means the text is not a valid target and is skipped.
fn parse_target(item: &str, default_topic: Option<i64>) -> Result<Option<Target>, ParseError> {
    let Some(caps) = TARGET.captures(item) else {
        return Ok(None);
    };
    let chat = if let Some(number) = caps.name("number") {
        let negative = number.as_str().starts_with('-');
        let digits = number
            .as_str()
            .trim_start_matches('-')
            .trim_start_matches('0');
        let normalized = if digits.is_empty() {
            "0".to_owned()
        } else {
            format!("{}{digits}", if negative { "-" } else { "" })
        };
        Chat::Id(
            normalized
                .parse::<Number>()
                .map_err(|_| ParseError::InvalidOption)?,
        )
    } else {
        Chat::Name(format!("@{}", caps.name("name").map_or("", |m| m.as_str())))
    };
    let topic = caps
        .name("topic")
        .map(|m| {
            m.as_str()
                .parse::<i64>()
                .map_err(|_| ParseError::InvalidOption)
        })
        .transpose()?
        .or(default_topic);
    Ok(Some(Target { chat, topic }))
}

impl Telegram {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let format = message::format(options, notification);
        let body = message::merged(notification, format);
        let chunks = message::chunks(&body, 4096, options.overflow);
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token.expose()
        );
        let mut requests = Vec::new();
        for target in &self.targets {
            for chunk in &chunks {
                let mut payload = json!({
                    "chat_id": target.chat.to_json(),
                    "text": chunk,
                    "disable_notification": self.silent,
                    "link_preview_options": {"is_disabled": !self.preview},
                });
                if let Some(topic) = target.topic {
                    payload["message_thread_id"] = json!(topic);
                }
                match format {
                    Format::Html => payload["parse_mode"] = json!("HTML"),
                    Format::Markdown => {
                        payload["parse_mode"] = json!(match self.markdown {
                            MarkdownVersion::V2 => "MarkdownV2",
                            MarkdownVersion::V1 => "Markdown",
                        });
                    }
                    _ => {}
                }
                requests.push(PreparedRequest::json(&url, &payload).with_policy(options.policy()));
            }
        }
        requests
    }

    /// [`prepare`](Self::prepare), plus the detection of the chat to write to
    /// when no chat id was given.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        if !(self.targets.is_empty() && self.detect) {
            return Plan::requests(self.prepare(options, notification));
        }
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates",
            self.bot_token.expose()
        );
        let policy = options.policy();
        let (telegram, options, notification) =
            (self.clone(), options.clone(), notification.clone());
        Plan::requests(Vec::new()).then(Lookups::new(
            vec![OWNER.to_owned()],
            move |_| {
                // Apprise sends this as a POST without a body.
                Step::setup(
                    PreparedRequest::with_body(
                        Method::Post,
                        url.as_str(),
                        Some("application/json"),
                        Vec::<u8>::new(),
                    )
                    .with_policy(policy),
                )
            },
            read_owner,
            move |found| {
                let Some(id) = found.get(OWNER).and_then(|id| id.parse::<Number>().ok()) else {
                    return Vec::new();
                };
                let mut detected = telegram;
                detected.targets = vec![Target {
                    chat: Chat::Id(id),
                    topic: detected.topic,
                }];
                detected
                    .prepare(&options, &notification)
                    .into_iter()
                    .map(Step::delivery)
                    .collect()
            },
        ))
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
        let targets: Vec<Value> = self
            .targets
            .iter()
            .map(|t| json!([t.chat.to_json(), t.topic]))
            .collect();
        for (key, value) in [
            ("targets", Value::Array(targets)),
            ("bot_token", json!(self.bot_token.expose())),
            ("detect", json!(self.detect)),
            ("image", json!(self.image)),
            ("silent", json!(self.silent)),
            ("preview", json!(self.preview)),
            ("album", json!(self.album)),
            ("topic", json!(self.topic)),
            (
                "mdv",
                json!(match self.markdown {
                    MarkdownVersion::V2 => "MarkdownV2",
                    MarkdownVersion::V1 => "MARKDOWN",
                }),
            ),
            (
                "content",
                json!(match self.content {
                    Content::Before => "before",
                    Content::After => "after",
                }),
            ),
            ("payload", json!(self.payload)),
            ("headers", json!({})),
            ("query", json!({})),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
