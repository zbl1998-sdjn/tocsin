//! `DingTalk`: `dingtalk://[<secret>@]<token>[/<phone>...][?to=<phone>,...]`.
//!
//! The phone numbers are the people the message mentions. A secret turns on the
//! signature of `DingTalk`: the request carries the time and an HMAC of it, so
//! this is the one service whose request depends on the clock.

use std::{
    fmt::Write as _,
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use regex::Regex;
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, hmac, message,
    options::{FormatMode, Options},
};

/// What Apprise accepts as a token or a secret: `^[a-z0-9]+$` under `re.I`.
/// Python's `re.I` also takes U+0130 and U+0131 for an `i`, which the case
/// folding of Rust does not, so they are named.
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9\x{130}\x{131}]+$").expect("static regex"));

/// What Apprise takes for a phone number before it counts the digits.
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\+?(?P<phone>[0-9\s)(+-]+)\s*$").expect("static regex"));

/// A parsed `dingtalk://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct DingTalk {
    token: SecretString,
    secret: Option<SecretString>,
    targets: Vec<String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, DingTalk), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Supported(&[Format::Text, Format::Markdown]),
    )?;
    // An argument beats the URL unless it is empty.
    let argument = |key: &str| {
        raw.query
            .get(key)
            .filter(|value| !value.is_empty())
            .map(|value| grammar::decode(value))
    };
    let token_text = argument("token").unwrap_or_else(|| raw.options.host.clone());
    let token = grammar::validate(&WORD, &token_text).ok_or(ParseError::InvalidToken)?;

    // The user of the URL is the secret, as written: only the base class of
    // Apprise decodes the user, and the secret does not come from there.
    let secret_text =
        argument("secret").or_else(|| raw.original_user.clone().filter(|user| !user.is_empty()));
    let secret = secret_text
        .map(|text| grammar::validate(&WORD, &text).ok_or(ParseError::InvalidToken))
        .transpose()?;

    // The path and `to=`, as one list that is then split, sorted and made
    // unique again, as `__init__` does with what `parse_url` hands it.
    let mut entries = raw.paths.clone();
    if let Some(to) = argument("to") {
        entries.extend(grammar::list(&to));
    }
    let targets = grammar::list(&entries.join(","))
        .into_iter()
        .filter_map(|target| phone_digits(&target))
        .collect();

    let dingtalk = DingTalk {
        token: SecretString::new(token),
        secret: secret.map(SecretString::new),
        targets,
    };
    Ok((raw.options, dingtalk))
}

/// The digits of a phone number, or nothing when there are too few or too many.
fn phone_digits(target: &str) -> Option<String> {
    let found = PHONE.captures(target)?;
    let digits: String = found["phone"]
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    (11..=14).contains(&digits.len()).then_some(digits)
}

impl DingTalk {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        self.prepare_at(options, notification, now)
    }

    /// The same at a given time, in milliseconds since the epoch.
    fn prepare_at(
        &self,
        options: &Options,
        notification: &Notification,
        timestamp: u128,
    ) -> Vec<PreparedRequest> {
        let format = message::format(options, notification);
        let mut url = format!(
            "https://oapi.dingtalk.com/robot/send?access_token={}",
            grammar::form_encode(self.token.expose())
        );
        if let Some(secret) = &self.secret {
            let signed = format!("{timestamp}\n{}", secret.expose());
            let digest = hmac::hmac_sha256(secret.expose().as_bytes(), signed.as_bytes());
            let signature = base64::engine::general_purpose::STANDARD.encode(digest);
            let _ = write!(
                url,
                "&timestamp={timestamp}&sign={}",
                grammar::quote(&signature)
            );
        }
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let mut payload = json!({
                    "msgtype": "text",
                    "at": {"atMobiles": self.targets, "isAtAll": false},
                });
                if format == Format::Markdown {
                    // Apprise takes the words of the title and drops the marks
                    // that would make it a heading or a list item.
                    let words = title.split_whitespace().collect::<Vec<_>>().join(" ");
                    let title = words.trim_start_matches(['#', '-', ' ']);
                    payload["msgtype"] = json!("markdown");
                    payload["markdown"] = json!({
                        "title": if title.is_empty() { "tocsin" } else { title },
                        "text": if title.is_empty() { body } else { format!("# {title}\n{body}") },
                    });
                } else {
                    let content = if title.is_empty() {
                        body
                    } else {
                        format!("{title}\r\n{body}")
                    };
                    payload["text"] = json!({"content": content});
                }
                PreparedRequest::json(&url, &payload).with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("token", json!(self.token.expose())),
            (
                "secret",
                json!(self.secret.as_ref().map(SecretString::expose)),
            ),
            ("targets", json!(self.targets)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::Notification;

    #[test]
    fn the_signature_is_the_one_python_computes() {
        // hmac.new(b"SECabc", b"1700000000000\nSECabc", sha256), base64, quoted.
        let (options, dingtalk) = parse("dingtalk://SECabc@tok123").expect("parse");
        let requests = dingtalk.prepare_at(&options, &Notification::new("body"), 1_700_000_000_000);
        let url = requests[0].url.expose();
        assert!(
            url.starts_with("https://oapi.dingtalk.com/robot/send?access_token=tok123&timestamp=1700000000000&sign="),
            "{requests:?}"
        );
        assert!(
            url.ends_with("jcUpW0QmtKduN03n4JqQ0PBosVjqnM8gU7fIIvsDmCM%3D"),
            "{requests:?}"
        );
    }

    #[test]
    fn phone_numbers_keep_their_digits() {
        assert_eq!(
            super::phone_digits("+86-138-0013-8000").as_deref(),
            Some("8613800138000")
        );
        assert_eq!(super::phone_digits("1234567890"), None);
        assert_eq!(super::phone_digits("123456789012345"), None);
    }
}
