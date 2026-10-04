//! Gotify: `gotify://<host>[:<port>]/[<path>/]<token>` and `gotifys://...`.

use std::fmt::Write as _;

use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Priority {
    Low,
    Moderate,
    Normal,
    High,
    Emergency,
}

impl Priority {
    /// The value Gotify's API takes.
    fn value(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Moderate => 3,
            Self::Normal => 5,
            Self::High => 8,
            Self::Emergency => 10,
        }
    }
}

/// Apprise takes the first entry whose key starts the lower-cased value, so
/// `10` has to come before `1`. Anything else is `Normal`.
fn priority_from(wanted: &str) -> Priority {
    [
        ("l", Priority::Low),
        ("m", Priority::Moderate),
        ("n", Priority::Normal),
        ("h", Priority::High),
        ("e", Priority::Emergency),
        ("10", Priority::Emergency),
        ("0", Priority::Low),
        ("1", Priority::Low),
        ("2", Priority::Low),
        ("3", Priority::Moderate),
        ("4", Priority::Moderate),
        ("5", Priority::Normal),
        ("6", Priority::Normal),
        ("7", Priority::Normal),
        ("8", Priority::High),
        ("9", Priority::High),
    ]
    .into_iter()
    .find(|(prefix, _)| wanted.starts_with(prefix))
    .map_or(Priority::Normal, |(_, priority)| priority)
}

/// A parsed `gotify://` or `gotifys://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Gotify {
    token: SecretString,
    priority: Priority,
    /// The server's base path, `/` or `/a/b/`.
    path: String,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Gotify), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Fixed(Format::Text),
    )?;
    let mut segments = raw.paths.clone();
    let token = segments
        .pop()
        .and_then(|segment| grammar::token(&segment))
        .ok_or(ParseError::InvalidToken)?;
    let path = if segments.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}/", segments.join("/"))
    };
    let priority = raw
        .query
        .get("priority")
        .filter(|s| !s.is_empty())
        .map_or(Priority::Normal, |s| {
            priority_from(&grammar::decode(s).to_lowercase())
        });
    let gotify = Gotify {
        token: SecretString::new(token),
        priority,
        path,
    };
    Ok((raw.options, gotify))
}

impl Gotify {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let mut url = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port {
            write!(url, ":{port}").expect("writing to a String works");
        }
        url.push_str(&self.path);
        url.push_str("message");
        let markdown = matches!(options.format, FormatMode::Fixed(Format::Markdown));
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let mut payload = json!({"priority": self.priority.value(), "message": body});
                if !title.is_empty() {
                    payload["title"] = json!(title);
                }
                if markdown {
                    payload["extras"] =
                        json!({"client::display": {"contentType": "text/markdown"}});
                }
                let mut request =
                    PreparedRequest::json(&url, &payload).with_policy(options.policy());
                request
                    .headers
                    .insert("X-Gotify-Key".to_owned(), self.token.clone());
                request
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("token", json!(self.token.expose())),
            ("priority", json!(self.priority.value())),
            ("path", json!(self.path)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
