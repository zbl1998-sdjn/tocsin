//! Google Chat: `gchat://<workspace>/<webhook key>/<webhook token>[/<thread key>]`,
//! and the web address of the webhook.
//!
//! Google Chat has no title, so the title goes in front of the text. Markdown is
//! sent as it is written: Apprise converts `CommonMark` to Google Chat's own
//! dialect, tocsin does not.

use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// A parsed `gchat://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct GoogleChat {
    workspace: SecretString,
    webhook_key: SecretString,
    webhook_token: SecretString,
    thread_key: Option<SecretString>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, GoogleChat), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Markdown),
    )?;
    // An option replaces what the URL says even when it is empty.
    let option = |key: &str| raw.query.get(key).map(|value| grammar::decode(value));
    // The path is the key, the token and the thread, whatever the options say.
    let mut path = raw.paths.clone().into_iter();
    let (path_key, path_token, path_thread) = (path.next(), path.next(), path.next());
    let workspace = option("workspace").unwrap_or_else(|| raw.options.host.clone());
    let key = option("key").or(path_key);
    let token = option("token").or(path_token);
    // The options name the thread two ways, and beat the path.
    let thread = option("thread")
        .or_else(|| option("threadkey"))
        .or(path_thread);

    let required = |value: Option<String>| {
        value
            .and_then(|text| grammar::token(&text))
            .map(SecretString::new)
            .ok_or(ParseError::InvalidToken)
    };
    let thread_key = thread
        .filter(|text| !text.is_empty())
        .map(|text| required(Some(text)))
        .transpose()?;
    let google_chat = GoogleChat {
        workspace: required(Some(workspace))?,
        webhook_key: required(key)?,
        webhook_token: required(token)?,
        thread_key,
    };
    Ok((raw.options, google_chat))
}

impl GoogleChat {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let mut url = format!(
            "https://chat.googleapis.com/v1/spaces/{}/messages?token={}&key={}",
            grammar::quote(self.workspace.expose()),
            grammar::form_encode(self.webhook_token.expose()),
            grammar::form_encode(self.webhook_key.expose()),
        );
        if self.thread_key.is_some() {
            url.push_str("&messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD");
        }
        message::chunks(&text, 4000, options.overflow)
            .into_iter()
            .map(|piece| {
                let mut payload = json!({"text": piece});
                if let Some(thread) = &self.thread_key {
                    payload["thread"] = json!({"thread_key": thread.expose()});
                }
                PreparedRequest::json(&url, &payload).with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("workspace", json!(self.workspace.expose())),
            ("webhook_key", json!(self.webhook_key.expose())),
            ("webhook_token", json!(self.webhook_token.expose())),
            (
                "thread_key",
                json!(self.thread_key.as_ref().map(SecretString::expose)),
            ),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
