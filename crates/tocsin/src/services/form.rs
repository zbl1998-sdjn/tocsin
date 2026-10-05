//! A form webhook: `form://<host>[:<port>][/<path>]` and `forms://...`.
//!
//! The message is sent as `application/x-www-form-urlencoded` fields named
//! `version`, `title`, `message` and `type`; with `method=get` they go in the
//! query string instead. See [`Webhook`] for the `+`, `-` and `:` arguments.
//!
//! With attachments the body is `multipart/form-data`: the fields, then one file
//! for every attachment, named by `attach-as` (`file01`, `file02`, ... when it is
//! left out or has a `*`, so that every file has a field of its own; one fixed
//! name otherwise). The files go with the first part of a long message. Like
//! Apprise, a `method=get` request carries only the files in its body and the
//! fields in the query string.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;

use super::webhook::Webhook;
use crate::{
    Attachment, Format, Method, Notification, ParseError, PreparedRequest, grammar,
    grammar::Pairs,
    message,
    multipart::Multipart,
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

/// The files of a notification, after the fields that are already in `multipart`.
fn with_files<'a>(
    mut multipart: Multipart<'a>,
    names: &'a [(String, String)],
    attachments: &'a [Attachment],
) -> Multipart<'a> {
    for ((field, filename), attachment) in names.iter().zip(attachments) {
        multipart = multipart.file(
            field,
            filename,
            Some(attachment.mime_type()),
            attachment.data(),
        );
    }
    multipart
}

impl Form {
    /// The form field and the file name of every attachment, in order.
    fn file_names(&self, files: &[Attachment]) -> Vec<(String, String)> {
        files
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                let no = index + 1;
                let field = if self.attach_multiple {
                    self.attach_as.replace(COUNT, &format!("{no:02}"))
                } else {
                    self.attach_as.clone()
                };
                (field, attachment.name_or_default(no))
            })
            .collect()
    }

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
        let files = notification.carried(options.overflow);
        let names = self.file_names(files);
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .enumerate()
            .map(|(index, (title, body))| {
                // The files go with the first part of a long message only.
                let (names, files): (&[_], &[_]) = if index == 0 {
                    (&names, files)
                } else {
                    (&[], &[])
                };
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
                    if names.is_empty() {
                        PreparedRequest::with_body(Method::Get, url, None, String::new())
                    } else {
                        let (content_type, body) =
                            with_files(Multipart::new(), names, files).finish();
                        PreparedRequest::with_body(Method::Get, url, Some(&content_type), body)
                    }
                } else {
                    let url = if self.webhook.params.is_empty() {
                        origin.clone()
                    } else {
                        format!("{origin}?{}", grammar::encode_pairs(&self.webhook.params))
                    };
                    if names.is_empty() {
                        PreparedRequest::with_body(
                            Method::Post,
                            url,
                            Some("application/x-www-form-urlencoded"),
                            grammar::encode_pairs(&fields),
                        )
                    } else {
                        let mut multipart = Multipart::new();
                        for (key, value) in fields.iter() {
                            multipart = multipart.field(key, value);
                        }
                        let (content_type, body) = with_files(multipart, names, files).finish();
                        PreparedRequest::with_body(Method::Post, url, Some(&content_type), body)
                    }
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
