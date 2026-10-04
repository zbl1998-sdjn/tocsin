//! A JSON webhook: `json://<host>[:<port>][/<path>]` and `jsons://...`.
//!
//! The body is Apprise's own layout, so existing receivers keep working:
//! `{"version", "title", "message", "attachments", "type"}`. See
//! [`Webhook`] for the `+`, `-` and `:` arguments.

use serde_json::{Map, Value, json};

use super::webhook::Webhook;
use crate::{
    Format, Notification, ParseError, PreparedRequest, grammar, message,
    options::{FormatMode, Options},
};

/// A parsed `json://` or `jsons://` URL.
#[derive(Clone)]
pub(crate) struct Json {
    webhook: Webhook,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Json), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Supported(&[Format::Text, Format::Html, Format::Markdown]),
    )?;
    let json = Json {
        webhook: Webhook::from_raw(&raw)?,
    };
    Ok((raw.options, json))
}

impl Json {
    /// One request per message part.
    ///
    /// A `:key=value` argument adds `key` to the body. When `key` is one of
    /// the body's own fields it renames that field to `value`, or drops it
    /// when `value` is empty.
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
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let mut payload = Map::new();
                payload.insert("version".to_owned(), json!("1.0"));
                payload.insert("title".to_owned(), json!(title));
                payload.insert("message".to_owned(), json!(body));
                payload.insert("attachments".to_owned(), json!([]));
                payload.insert("type".to_owned(), json!(notification.kind.to_string()));
                for (key, value) in self.webhook.payload.iter() {
                    if let Some(field) = payload.get(key).cloned() {
                        if !value.is_empty() {
                            payload.insert(value.to_owned(), field);
                        }
                        payload.remove(key);
                    } else {
                        payload.insert(key.to_owned(), json!(value));
                    }
                }
                let request = PreparedRequest::json(&url, &Value::Object(payload));
                self.webhook.finish(request, options)
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut Map<String, Value>) {
        self.webhook.compat(map);
    }
}
