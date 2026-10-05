//! Feishu: `feishu://<token>`, the custom bot of Feishu (the Chinese edition of
//! Lark; see `lark` for the international one).
//!
//! The text message of Feishu has no title, so the title goes in front of the
//! text.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// What Apprise accepts as a token: `^[A-Z0-9_-]+$` under `re.I`. Python's `re.I`
/// also takes U+0130 and U+0131 for an `i`, which the case folding of Rust does
/// not, so they are named.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9_\x{130}\x{131}-]+$").expect("static regex"));

/// A parsed `feishu://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Feishu {
    token: SecretString,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Feishu), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Text),
    )?;
    // `token=` beats the host unless it is empty.
    let text = raw
        .query
        .get("token")
        .filter(|value| !value.is_empty())
        .map_or_else(|| raw.options.host.clone(), |value| grammar::decode(value));
    let token = grammar::validate(&TOKEN, &text).ok_or(ParseError::InvalidToken)?;
    Ok((
        raw.options,
        Feishu {
            token: SecretString::new(token),
        },
    ))
}

impl Feishu {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let url = format!(
            "https://open.feishu.cn/open-apis/bot/v2/hook/{}/",
            grammar::quote(self.token.expose())
        );
        message::chunks(&text, 19985, options.overflow)
            .into_iter()
            .map(|piece| {
                let payload = json!({"msg_type": "text", "content": {"text": piece}});
                PreparedRequest::json(&url, &payload).with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        map.insert("token".to_owned(), json!(self.token.expose()));
    }
}
