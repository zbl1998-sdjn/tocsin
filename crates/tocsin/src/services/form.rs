//! A form webhook: `form://<host>[:<port>][/<path>]` and `forms://...`.
//!
//! The message is sent as `application/x-www-form-urlencoded` fields named
//! `version`, `title`, `message` and `type`; with `method=get` they go in the
//! query string instead. See [`Webhook`] for the `+`, `-` and `:` arguments.
//! Attachments are not supported, so `attach-as` is read and reported only.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;

use super::webhook::Webhook;
use crate::{
    Format, Method, Notification, ParseError, PreparedRequest, grammar,
    grammar::Pairs,
    message,
    options::{FormatMode, Options},
};

/// How a file name pattern such as `file*`, `file*name` or `?file` is read.
static ATTACH_AS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(?P<match1>(?P<id1a>[a-z0-9_-]+)?(?P<wc1>[*?+$:.%]+)(?P<id1b>[a-z0-9_-]+))|(?P<match2>(?P<id2>[a-z0-9_-]+)(?P<wc2>[*?+$:.%]?)))",
    )
    .expect("static regex")
});

/// The counter that stands for `*` in a file name pattern.
const COUNT: &str = "{:02d}";

/// The names of the four fields Apprise sends.
const FIELDS: [&str; 4] = ["version", "title", "message", "type"];

/// A parsed `form://` or `forms://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Form {
    webhook: Webhook,
    /// What each field is called in the request; empty drops it.
    names: BTreeMap<String, String>,
    /// The `:key=value` arguments that are not renames.
    extras: Pairs,
    attach_as: String,
    attach_multiple: bool,
}

/// Apprise's reading of `attach-as`, as the name and whether it counts files.
fn attach_as(pattern: &str) -> Result<(String, bool), ParseError> {
    let found = ATTACH_AS
        .captures(grammar::strip(pattern))
        .ok_or(ParseError::InvalidOption)?;
    let group = |name: &str| found.name(name).map(|m| m.as_str());
    if group("match1").is_some() {
        Ok((
            format!(
                "{}{COUNT}{}",
                group("id1a").unwrap_or(""),
                group("id1b").unwrap_or("")
            ),
            true,
        ))
    } else {
        let mut name = group("id2").unwrap_or("").to_owned();
        let counted = group("wc2").is_some_and(|wildcard| !wildcard.is_empty());
        if counted {
            name.push_str(COUNT);
        }
        Ok((name, counted))
    }
}

pub(crate) fn parse(input: &str) -> Result<(Options, Form), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Supported(&[Format::Text, Format::Html, Format::Markdown]),
    )?;
    let webhook = Webhook::from_raw(&raw)?;
    let (attach_as, attach_multiple) = match raw.query.get("attach-as").filter(|s| !s.is_empty()) {
        Some(pattern) => attach_as(pattern)?,
        None => (format!("file{COUNT}"), true),
    };
    let mut names: BTreeMap<String, String> = FIELDS
        .iter()
        .map(|field| ((*field).to_owned(), (*field).to_owned()))
        .collect();
    let mut extras = Pairs::default();
    for (key, value) in webhook.payload.iter() {
        if let Some(name) = names.get_mut(key) {
            value.clone_into(name);
        } else {
            extras.set(key, value);
        }
    }
    let form = Form {
        webhook,
        names,
        extras,
        attach_as,
        attach_multiple,
    };
    Ok((raw.options, form))
}

impl Form {
    /// One request per message part.
    ///
    /// A `:key=value` argument adds a field. When `key` is one of the four
    /// fields it renames that field to `value`, or drops it when `value` is
    /// empty. With `method=get` the fields and the `-` parameters share the
    /// query string and there is no body.
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let origin = self.webhook.origin(options);
        let kind = notification.kind.to_string();
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let mut fields = Pairs::default();
                for (field, value) in [
                    ("version", "1.0"),
                    ("title", title.as_str()),
                    ("message", body.as_str()),
                    ("type", kind.as_str()),
                ] {
                    let name = self.names.get(field).map_or(field, String::as_str);
                    if !name.is_empty() {
                        fields.set(name, value);
                    }
                }
                for (key, value) in self.extras.iter() {
                    fields.set(key, value);
                }
                let request = if self.webhook.method == Method::Get {
                    for (key, value) in self.webhook.params.iter() {
                        fields.set(key, value);
                    }
                    let query = grammar::encode_pairs(&fields);
                    let url = if query.is_empty() {
                        origin.clone()
                    } else {
                        format!("{origin}?{query}")
                    };
                    PreparedRequest::with_body(Method::Get, url, None, String::new())
                } else {
                    let url = if self.webhook.params.is_empty() {
                        origin.clone()
                    } else {
                        format!("{origin}?{}", grammar::encode_pairs(&self.webhook.params))
                    };
                    PreparedRequest::with_body(
                        Method::Post,
                        url,
                        Some("application/x-www-form-urlencoded"),
                        grammar::encode_pairs(&fields),
                    )
                };
                self.webhook.finish(request, options)
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;

        // Apprise reports the arguments that are not renames as the payload.
        let mut report = self.webhook.clone();
        report.payload = self.extras.clone();
        report.compat(map);
        for (key, value) in [
            ("payload_map", json!(self.names)),
            ("attach_as", json!(self.attach_as)),
            ("attach_multi", json!(self.attach_multiple)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
