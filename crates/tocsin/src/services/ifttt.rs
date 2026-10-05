//! IFTTT webhooks: `ifttt://<webhook_id>@<event>[/<event>...]`.
//!
//! One request for each event, `value1` the title, `value2` the text and
//! `value3` the kind. `+key=value` adds a field and `-key` takes one away,
//! which is how the three fields are renamed or dropped.

use std::collections::BTreeMap;

use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar,
    grammar::Pairs,
    message,
    options::{FormatMode, Options},
};

/// A parsed `ifttt://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Ifttt {
    webhook_id: SecretString,
    /// Unique and sorted, as Apprise keeps them.
    events: Vec<String>,
    /// The `+key=value` arguments.
    add_tokens: Pairs,
    /// The keys of the `-key` arguments.
    del_tokens: Vec<String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Ifttt), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Text),
    )?;
    let options = &raw.options;
    // The id is the user, or the host when there is no user, and then the
    // events start with the path.
    let user = options.user.as_deref().filter(|user| !user.is_empty());
    let webhook_id =
        grammar::token(user.unwrap_or(&options.host)).ok_or(ParseError::InvalidToken)?;
    let mut events: Vec<String> = Vec::new();
    if user.is_some() {
        events.push(options.host.clone());
    }
    events.extend(raw.paths.iter().cloned());
    if let Some(to) = raw.query.get("to").filter(|to| !to.is_empty()) {
        events.push(grammar::decode(to));
    }
    let events = grammar::list(&events.join(" "));
    if events.is_empty() {
        return Err(ParseError::InvalidOption);
    }
    let ifttt = Ifttt {
        webhook_id: SecretString::new(webhook_id),
        events,
        add_tokens: raw.headers.clone(),
        del_tokens: raw.params.iter().map(|(key, _)| key.to_owned()).collect(),
    };
    Ok((raw.options, ifttt))
}

impl Ifttt {
    /// The fields of the request for one message: the three of Apprise's, then
    /// the added ones, with every key in lower case and the removed keys left
    /// out. A removal is matched against the key as it was written.
    fn payload(&self, kind: &str, title: &str, body: &str) -> BTreeMap<String, String> {
        let mut fields: Vec<(String, String)> = vec![
            ("value1".to_owned(), title.to_owned()),
            ("value2".to_owned(), body.to_owned()),
            ("value3".to_owned(), kind.to_owned()),
        ];
        for (key, value) in self.add_tokens.iter() {
            match fields.iter_mut().find(|(existing, _)| existing == key) {
                Some(field) => value.clone_into(&mut field.1),
                None => fields.push((key.to_owned(), value.to_owned())),
            }
        }
        fields
            .into_iter()
            .filter(|(key, _)| !self.del_tokens.contains(key))
            .map(|(key, value)| (key.to_lowercase(), value))
            .collect()
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let kind = notification.kind.to_string();
        let mut requests = Vec::new();
        for (title, body) in message::parts(notification, 250, 32768, options.overflow) {
            let payload = json!(self.payload(&kind, &title, &body));
            for event in &self.events {
                let url = format!(
                    "https://maker.ifttt.com/trigger/{}/with/key/{}",
                    grammar::quote(event),
                    grammar::quote(self.webhook_id.expose())
                );
                requests.push(PreparedRequest::json(url, &payload).with_policy(options.policy()));
            }
        }
        requests
    }

    /// The id and the events. The `+` and `-` arguments are left out: Apprise
    /// never applies the ones in a URL, so there is nothing to compare them with.
    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("webhook_id", json!(self.webhook_id.expose())),
            ("events", json!(self.events)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
