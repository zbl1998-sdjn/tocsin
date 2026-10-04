//! Discord webhooks: `discord://<webhook_id>/<webhook_token>`.

use std::collections::BTreeMap;

use serde_json::json;

use crate::{
    Format, Kind, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// A parsed `discord://` URL.
///
/// Options such as `image`, `footer_logo`, `fields` and `batch` are parsed and
/// validated like Apprise does, but only the ones that shape the request are
/// applied.
// One bool per URL flag, mirroring Apprise's option set.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Discord {
    webhook_id: String,
    webhook_token: SecretString,
    tts: bool,
    avatar: bool,
    image: bool,
    footer: bool,
    footer_logo: bool,
    fields: bool,
    flags: Option<u64>,
    thread: Option<String>,
    avatar_url: Option<String>,
    href: Option<String>,
    ping: Vec<String>,
    batch: bool,
    payload: BTreeMap<String, String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Discord), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Any,
        FormatMode::Supported(&[Format::Text, Format::Markdown]),
    )?;
    let mut options = raw.options.clone();
    let id = grammar::token(options.host.trim());
    let token = raw.paths.first().and_then(|s| grammar::token(s.trim()));
    let (Some(id), Some(token)) = (id, token) else {
        return Err(ParseError::InvalidToken);
    };
    let flags = raw
        .query
        .get("flags")
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().map_err(|_| ParseError::InvalidOption))
        .transpose()?;
    let href = raw
        .query
        .get("href")
        .or_else(|| raw.query.get("url"))
        .map(|s| grammar::decode(s));
    if raw.query.contains_key("url") || raw.query.contains_key("thread") {
        options.format_override = Some(Format::Markdown);
    }
    if let Some(botname) = raw.query.get("botname") {
        options.user = Some(grammar::decode(botname));
    }
    let ping = raw
        .query
        .get("ping")
        .map(|s| grammar::list(&grammar::decode(s)))
        .unwrap_or_default();
    let discord = Discord {
        webhook_id: id,
        webhook_token: SecretString::new(token),
        tts: raw.flag("tts", false),
        avatar: raw.flag("avatar", true),
        image: raw.flag("image", false),
        footer: raw.flag("footer", false),
        footer_logo: raw.flag("footer_logo", true),
        fields: raw.flag("fields", true),
        flags,
        thread: raw.optional("thread"),
        avatar_url: raw.optional("avatar_url"),
        href,
        ping,
        batch: raw.flag("batch", true),
        payload: raw.payload.to_map(),
    };
    Ok((options, discord))
}

impl Discord {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let embeds = options.format_override == Some(Format::Markdown);
        let pieces = message::parts(notification, 250, 2000, options.overflow);
        // `wait` is a query parameter in Discord's API, not a body field.
        let mut url = format!(
            "https://discord.com/api/webhooks/{}/{}?wait={}",
            grammar::encode(&self.webhook_id),
            grammar::encode(self.webhook_token.expose()),
            !self.tts
        );
        if let Some(thread) = self.thread.as_deref().filter(|t| !t.is_empty()) {
            url.push_str("&thread_id=");
            url.push_str(&grammar::encode(thread));
        }
        let mut requests = Vec::new();
        for (title, chunk) in pieces {
            let mut payload = json!({"tts": self.tts});
            if let Some(user) = options.user.as_deref().filter(|u| !u.is_empty()) {
                payload["username"] = json!(user);
            }
            let flags = self.flags.unwrap_or(0) & (4 | 4096);
            if flags > 0 {
                payload["flags"] = json!(flags);
            }
            if self.avatar {
                if let Some(avatar_url) = self.avatar_url.as_deref().filter(|u| !u.is_empty()) {
                    payload["avatar_url"] = json!(avatar_url);
                }
            }
            if embeds {
                let color = match notification.kind {
                    Kind::Success => 0x22_C55E,
                    Kind::Warning => 0xF5_9E0B,
                    Kind::Failure => 0xEF_4444,
                    Kind::Info => 0x3B_82F6,
                };
                let mut embed = json!({"title": title, "description": chunk, "color": color});
                if let Some(href) = self.href.as_deref().filter(|h| !h.is_empty()) {
                    embed["url"] = json!(href);
                }
                if self.footer {
                    embed["footer"] = json!({"text": "tocsin"});
                }
                payload["embeds"] = json!([embed]);
            } else {
                let content = if title.is_empty() {
                    chunk
                } else {
                    format!("{title}\r\n{chunk}")
                };
                // Discord's content cap includes the CRLF join. `overflow=upstream`
                // deliberately stays untouched.
                payload["content"] = json!(if options.overflow == crate::Overflow::Upstream {
                    content
                } else {
                    message::truncate(&content, 2000)
                });
            }
            requests.push(PreparedRequest::json(&url, &payload).with_policy(options.policy()));
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("targets", json!([])),
            ("webhook_id", json!(self.webhook_id)),
            ("webhook_token", json!(self.webhook_token.expose())),
            ("tts", json!(self.tts)),
            ("avatar", json!(self.avatar)),
            ("image", json!(self.image)),
            ("footer", json!(self.footer)),
            ("footer_logo", json!(self.footer_logo)),
            ("fields", json!(self.fields)),
            ("flags", json!(self.flags)),
            ("thread", json!(self.thread)),
            ("avatar_url", json!(self.avatar_url)),
            ("href", json!(self.href)),
            ("ping", json!(self.ping)),
            ("batch", json!(self.batch)),
            ("payload", json!(self.payload)),
            ("headers", json!({})),
            ("query", json!({})),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
