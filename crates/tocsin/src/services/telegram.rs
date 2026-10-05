//! Telegram bot messages: `tgram://<bot_token>/<chat_id>[/<chat_id>...]`.
//!
//! Without a chat id the message goes to whoever last wrote to the bot, which
//! Telegram only tells through `getUpdates`. That is a request whose answer the
//! next one needs, so it works through [`Service::plan`](crate::Service::plan);
//! [`Service::prepare`](crate::Service::prepare) returns nothing for it. Send the
//! bot a message first, as Apprise asks you to.
//!
//! Files are uploaded with `sendPhoto`, `sendVideo`, `sendAudio`, `sendVoice`,
//! `sendAnimation` or `sendDocument`, whichever fits their media type (a photo
//! over 10 MB goes as a document, a file over 50 MB is refused). The caption and
//! the files' placement follow Apprise; `album=` is ignored, so every file is a
//! message of its own. A refused file is a failure in the report of
//! [`Service::plan`](crate::Service::plan).

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;
use serde_json::{Number, Value, json};

use crate::{
    Attachment, Format, Method, Notification, ParseError, Plan, PreparedRequest, Response,
    SecretString, TransportError, grammar, message,
    multipart::Multipart,
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

/// The longest text that can be the caption of a file.
const CAPTION_MAX: usize = 1024;
/// The largest photo that is sent as a photo; Telegram takes larger ones as
/// documents.
const PHOTO_MAX_BYTES: usize = 10_000_000;
/// The largest file a bot can upload.
const FILE_MAX_BYTES: usize = 50_000_000;

/// The text that goes with the first file instead of a message of its own.
struct Caption<'a> {
    text: &'a str,
    /// Whether it is shown above the file.
    above: bool,
    parse_mode: Option<&'static str>,
}

/// The Bot API method that sends a file, and the name of its file field.
fn media_method(attachment: &Attachment) -> (&'static str, &'static str) {
    let mime = attachment.mime_type().to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| mime.starts_with(prefix));
    if starts(&["image/gif", "video/h264"]) {
        ("sendAnimation", "animation")
    } else if starts(&["image/"]) {
        if attachment.len() > PHOTO_MAX_BYTES {
            ("sendDocument", "document")
        } else {
            ("sendPhoto", "photo")
        }
    } else if starts(&["video/mp4"]) {
        ("sendVideo", "video")
    } else if starts(&["application/ogg", "audio/ogg"]) {
        ("sendVoice", "voice")
    } else if starts(&["audio/mpeg", "audio/mp4a-latm"]) {
        ("sendAudio", "audio")
    } else {
        ("sendDocument", "document")
    }
}

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
    /// The `parse_mode` that goes with a text in this format.
    fn parse_mode(&self, format: Format) -> Option<&'static str> {
        match format {
            Format::Html => Some("HTML"),
            Format::Markdown => Some(match self.markdown {
                MarkdownVersion::V2 => "MarkdownV2",
                MarkdownVersion::V1 => "Markdown",
            }),
            _ => None,
        }
    }

    /// One `sendMessage` request.
    fn message(
        &self,
        options: &Options,
        target: &Target,
        text: &str,
        format: Format,
    ) -> PreparedRequest {
        let mut payload = json!({
            "chat_id": target.chat.to_json(),
            "text": text,
            "disable_notification": self.silent,
            "link_preview_options": {"is_disabled": !self.preview},
        });
        if let Some(topic) = target.topic {
            payload["message_thread_id"] = json!(topic);
        }
        if let Some(mode) = self.parse_mode(format) {
            payload["parse_mode"] = json!(mode);
        }
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token.expose()
        );
        PreparedRequest::json(url, &payload).with_policy(options.policy())
    }

    /// The upload of one file, with `caption` if it has one, or `None` when
    /// Telegram would refuse a file of this size.
    fn media(
        &self,
        options: &Options,
        target: &Target,
        (no, attachment): (usize, &Attachment),
        caption: Option<&Caption<'_>>,
    ) -> Option<PreparedRequest> {
        if attachment.len() > FILE_MAX_BYTES {
            return None;
        }
        let (function, key) = media_method(attachment);
        let name = attachment.name_or_default(no);
        let mut fields: Vec<(&str, String)> = Vec::new();
        if let Some(caption) = caption {
            fields.push(("caption", caption.text.to_owned()));
            fields.push(("show_caption_above_media", caption.above.to_string()));
            if let Some(mode) = caption.parse_mode {
                fields.push(("parse_mode", mode.to_owned()));
            }
        }
        fields.push(("title", name.clone()));
        fields.push((
            "chat_id",
            match &target.chat {
                Chat::Id(number) => number.to_string(),
                Chat::Name(name) => name.clone(),
            },
        ));
        if let Some(topic) = target.topic {
            fields.push(("message_thread_id", topic.to_string()));
        }
        let mut multipart = Multipart::new();
        for (field, value) in &fields {
            multipart = multipart.field(field, value);
        }
        // Apprise gives the file no media type of its own.
        let (content_type, body) = multipart.file(key, &name, None, attachment.data()).finish();
        let url = format!(
            "https://api.telegram.org/bot{}/{function}",
            self.bot_token.expose()
        );
        Some(
            PreparedRequest::with_body(Method::Post, url, Some(&content_type), body)
                .with_policy(options.policy()),
        )
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        self.requests(options, notification).0
    }

    /// The requests, and how many files Telegram would refuse.
    ///
    /// The files go with one part of the message: the last when the text comes
    /// first (`content=before`, the default), the first otherwise. A text of
    /// less than 1024 characters is their caption instead of a message of its
    /// own, as Apprise does.
    fn requests(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> (Vec<PreparedRequest>, usize) {
        let format = message::format(options, notification);
        let body = message::merged(notification, format);
        let chunks = message::chunks(&body, 4096, options.overflow);
        let carried = notification.carried(options.overflow);
        let attach_at = if self.content == Content::Before {
            chunks.len() - 1
        } else {
            0
        };
        let mut requests = Vec::new();
        let mut refused = 0;
        for target in &self.targets {
            for (index, chunk) in chunks.iter().enumerate() {
                let files = if index == attach_at { carried } else { &[] };
                let has_body = !chunk.is_empty();
                let caption = (!files.is_empty()
                    && has_body
                    && chunk.chars().count() < CAPTION_MAX)
                    .then(|| Caption {
                        text: chunk,
                        above: self.content == Content::Before,
                        parse_mode: self.parse_mode(format),
                    });
                let text_with_files = has_body && caption.is_none() && !files.is_empty();
                if files.is_empty() || (text_with_files && self.content == Content::Before) {
                    requests.push(self.message(options, target, chunk, format));
                }
                for (index, attachment) in files.iter().enumerate() {
                    let caption = if index == 0 { caption.as_ref() } else { None };
                    match self.media(options, target, (index + 1, attachment), caption) {
                        Some(request) => requests.push(request),
                        None => refused += 1,
                    }
                }
                if text_with_files && self.content == Content::After {
                    requests.push(self.message(options, target, chunk, format));
                }
            }
        }
        (requests, refused)
    }

    /// [`prepare`](Self::prepare), plus the files Telegram would refuse, which
    /// Apprise reports as failures, and the detection of the chat to write to
    /// when no chat id was given.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        if !(self.targets.is_empty() && self.detect) {
            let (requests, refused) = self.requests(options, notification);
            return Self::refusals(Plan::requests(requests), refused);
        }
        let too_large = notification
            .carried(options.overflow)
            .iter()
            .filter(|file| file.len() > FILE_MAX_BYTES)
            .count();
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates",
            self.bot_token.expose()
        );
        let policy = options.policy();
        let (telegram, options, notification) =
            (self.clone(), options.clone(), notification.clone());
        Self::refusals(Plan::requests(Vec::new()), too_large).then(Lookups::new(
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

    /// `count` failures, for the files that were not sent.
    fn refusals(mut plan: Plan, count: usize) -> Plan {
        for _ in 0..count {
            plan = plan.failed(TransportError::InvalidRequest);
        }
        plan
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
