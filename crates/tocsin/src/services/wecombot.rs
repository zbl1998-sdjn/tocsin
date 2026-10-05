//! `WeCom` bot: `wecombot://<key>` and the web address of the bot,
//! `https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=<key>`.
//!
//! The text message has no title, so the title goes in front of the text.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// What Apprise accepts as a key: `^[a-z0-9_-]+$` under `re.I`. Python's `re.I`
/// also takes U+0130 and U+0131 for an `i`, which the case folding of Rust does
/// not, so they are named.
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9_\x{130}\x{131}-]+$").expect("static regex"));

/// A parsed `wecombot://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct WeComBot {
    key: SecretString,
}

pub(crate) fn parse(input: &str) -> Result<(Options, WeComBot), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Text),
    )?;
    // `key=` beats the host unless it is empty.
    let text = raw
        .query
        .get("key")
        .filter(|value| !value.is_empty())
        .map_or_else(|| raw.options.host.clone(), |value| grammar::decode(value));
    let key = grammar::validate(&KEY, &text).ok_or(ParseError::InvalidToken)?;
    Ok((
        raw.options,
        WeComBot {
            key: SecretString::new(key),
        },
    ))
}

impl WeComBot {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key={}",
            grammar::form_encode(self.key.expose())
        );
        message::chunks(&text, 32768, options.overflow)
            .into_iter()
            .map(|piece| {
                let payload = json!({"msgtype": "text", "text": {"content": piece}});
                PreparedRequest::json(&url, &payload).with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        map.insert("key".to_owned(), json!(self.key.expose()));
    }
}
