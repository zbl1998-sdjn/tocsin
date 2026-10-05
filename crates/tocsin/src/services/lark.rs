//! Lark: `lark://<token>` and the web address of the bot,
//! `https://open.larksuite.com/open-apis/bot/v2/hook/<token>`.
//!
//! The title goes in front of the text, on a line of its own.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// What Apprise accepts as a token: `^[a-z0-9-]+$` under `re.I`. Python's `re.I`
/// also takes U+0130 and U+0131 for an `i`, which the case folding of Rust does
/// not, so they are named.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9\x{130}\x{131}-]+$").expect("static regex"));

/// A parsed `lark://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Lark {
    token: SecretString,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Lark), ParseError> {
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
        Lark {
            token: SecretString::new(token),
        },
    ))
}

impl Lark {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let url = format!(
            "https://open.larksuite.com/open-apis/bot/v2/hook/{}",
            grammar::quote(self.token.expose())
        );
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let text = if title.is_empty() {
                    body
                } else {
                    format!("{title}\n{body}")
                };
                let payload = json!({"msg_type": "text", "content": {"text": text}});
                PreparedRequest::json(&url, &payload).with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        map.insert("token".to_owned(), json!(self.token.expose()));
    }
}
