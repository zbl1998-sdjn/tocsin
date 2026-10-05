//! An XML webhook: `xml://<host>[:<port>][/<path>]` and `xmls://...`.
//!
//! The body is Apprise's own SOAP envelope, so existing receivers keep working:
//! a `Notification` with the elements `Version`, `Subject`, `Message` and
//! `MessageType`, and an `Attachments` element with the files in base64. A
//! `:name=value` argument adds an element, or renames one of the four when
//! `name` is its name (an empty value drops it). Characters that cannot be in an
//! element name are removed from `name` first. See [`Webhook`] for `+`, `-` and
//! the method.
//!
//! Apprise parses `-name=value` for this service but never sends it; tocsin adds
//! it to the query string, as the JSON and form webhooks do.

use std::fmt::Write as _;

use base64::{Engine, engine::general_purpose::STANDARD};

use super::webhook::Webhook;
use crate::{
    Attachment, Format, Method, Notification, ParseError, PreparedRequest, grammar,
    grammar::Pairs,
    message,
    options::{FormatMode, Options},
};

/// The elements Apprise sends, which a `:name=value` argument can rename.
const FIELDS: [&str; 4] = ["Version", "Subject", "Message", "MessageType"];

/// The schema Apprise names when nothing is added or renamed.
const SCHEMA: &str =
    "https://raw.githubusercontent.com/caronc/apprise/master/apprise/assets/NotifyXML-1.1.xsd";

/// A parsed `xml://` or `xmls://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Xml {
    webhook: Webhook,
    /// What each of the four elements is called; empty drops it.
    names: Vec<(&'static str, String)>,
    /// The arguments that add an element.
    extras: Pairs,
    /// The arguments that rename one of the four.
    overrides: Pairs,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Xml), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Supported(&[Format::Text, Format::Html, Format::Markdown]),
    )?;
    let webhook = Webhook::from_raw(&raw)?;
    let mut names: Vec<(&'static str, String)> = FIELDS
        .iter()
        .map(|field| (*field, (*field).to_owned()))
        .collect();
    let mut extras = Pairs::default();
    let mut overrides = Pairs::default();
    for (key, value) in webhook.payload.iter() {
        // The name becomes an element, so only what an element name can hold
        // stays.
        let key: String = key
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect();
        if key.is_empty() {
            continue;
        }
        if let Some(slot) = names.iter_mut().find(|(field, _)| *field == key) {
            value.clone_into(&mut slot.1);
            overrides.set(&key, value);
        } else {
            extras.set(&key, value);
        }
    }
    let xml = Xml {
        webhook,
        names,
        extras,
        overrides,
    };
    Ok((raw.options, xml))
}

/// Apprise's `escape_html` without the whitespace tidying: what can end a tag
/// or an attribute.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('>', "&gt;")
        .replace('<', "&lt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// Set a value in a list that keeps the order of first appearance, as a Python
/// dictionary does.
fn set(elements: &mut Vec<(String, String)>, name: &str, value: String) {
    match elements.iter_mut().find(|(existing, _)| existing == name) {
        Some(element) => element.1 = value,
        None => elements.push((name.to_owned(), value)),
    }
}

/// The `Attachments` element, or nothing when there is no file.
fn attachments(files: &[Attachment]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut xml = String::from("<Attachments format=\"base64\">");
    for (index, file) in files.iter().enumerate() {
        write!(
            xml,
            "<Attachment filename=\"{}\" mimetype=\"{}\">{}</Attachment>",
            escape(&file.name_or_default(index + 1)),
            escape(file.mime_type()),
            STANDARD.encode(file.data())
        )
        .expect("writing to a String works");
    }
    xml.push_str("</Attachments>");
    xml
}

impl Xml {
    /// The envelope for one part of the message.
    fn document(
        &self,
        notification: &Notification,
        title: &str,
        body: &str,
        files: &[Attachment],
    ) -> String {
        let kind = notification.kind.to_string();
        let mut elements: Vec<(String, String)> = Vec::new();
        for (field, value) in [
            ("Version", "1.1".to_owned()),
            ("Subject", escape(title)),
            ("Message", escape(body)),
            ("MessageType", escape(&kind)),
        ] {
            let name = self
                .names
                .iter()
                .find(|(key, _)| *key == field)
                .map_or(field, |(_, name)| name.as_str());
            // An empty name drops the element.
            if !name.is_empty() {
                set(&mut elements, name, value);
            }
        }
        for (name, value) in self.extras.iter() {
            set(&mut elements, name, escape(value));
        }
        let mut core = String::new();
        for (name, value) in &elements {
            write!(core, "<{name}>{value}</{name}>").expect("writing to a String works");
        }
        let schema = if self.extras.is_empty() && self.overrides.is_empty() {
            format!(" xmlns:xsi=\"{SCHEMA}\"")
        } else {
            String::new()
        };
        format!(
            "<?xml version='1.0' encoding='utf-8'?>\n\
<soapenv:Envelope\n    \
xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\"\n    \
xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\"\n    \
xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n    \
<soapenv:Body>\n        \
<Notification{schema}>\n            \
{core}\n            \
{files}\n       \
</Notification>\n    \
</soapenv:Body>\n\
</soapenv:Envelope>",
            files = attachments(files)
        )
    }

    /// One request per message part.
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let mut url = self.webhook.origin(options);
        if !self.webhook.params.is_empty() {
            url.push('?');
            url.push_str(&grammar::encode_pairs(&self.webhook.params));
        }
        let files = notification.carried(options.overflow);
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .enumerate()
            .map(|(index, (title, body))| {
                // The files go with the first part of a long message only.
                let document = self.document(
                    notification,
                    &title,
                    &body,
                    if index == 0 { files } else { &[] },
                );
                let request = PreparedRequest::with_body(
                    Method::Post,
                    &url,
                    Some("application/xml"),
                    document,
                );
                self.webhook.finish(request, options)
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;

        // Apprise reports the arguments that add an element as the payload.
        let mut report = self.webhook.clone();
        report.payload = self.extras.clone();
        report.compat(map);
        for (key, value) in [
            ("overrides", json!(super::webhook::map_of(&self.overrides))),
            (
                "xsd_url",
                json!((self.extras.is_empty() && self.overrides.is_empty()).then_some(SCHEMA)),
            ),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
