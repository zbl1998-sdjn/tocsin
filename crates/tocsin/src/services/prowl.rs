//! Prowl: `prowl://<apikey>[/<providerkey>]`.
//!
//! The message is a form post to Prowl's public API, with the title as the
//! event and the body as the description.

use std::sync::LazyLock;

use regex::Regex;

use crate::{
    Format, Method, Notification, ParseError, PreparedRequest, SecretString, grammar,
    grammar::{Pairs, encode_pairs},
    message,
    options::{FormatMode, Options},
};

/// What Apprise accepts as an API key or a provider key.
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Za-z0-9]{40}$").expect("static regex"));

/// Apprise takes the first entry whose key starts the lower-cased value: the
/// names by their first letter, and Prowl's own numbers. Anything else is
/// normal.
fn priority_from(wanted: &str) -> i8 {
    [
        ("l", -2),
        ("m", -1),
        ("n", 0),
        ("h", 1),
        ("e", 2),
        ("-2", -2),
        ("-1", -1),
        ("0", 0),
        ("1", 1),
        ("2", 2),
    ]
    .into_iter()
    .find(|(prefix, _)| wanted.starts_with(prefix))
    .map_or(0, |(_, priority)| priority)
}

/// A parsed `prowl://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Prowl {
    apikey: SecretString,
    providerkey: Option<SecretString>,
    priority: i8,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Prowl), ParseError> {
    let raw = grammar::parse(input, grammar::Hosts::Any, FormatMode::Fixed(Format::Text))?;
    let apikey = grammar::validate(&KEY, &raw.options.host).ok_or(ParseError::InvalidToken)?;
    let providerkey = raw
        .paths
        .first()
        .map(|key| grammar::validate(&KEY, key).ok_or(ParseError::InvalidToken))
        .transpose()?;
    let priority = raw
        .query
        .get("priority")
        .filter(|s| !s.is_empty())
        .map_or(0, |s| priority_from(&grammar::decode(s).to_lowercase()));
    let prowl = Prowl {
        apikey: SecretString::new(apikey),
        providerkey: providerkey.map(SecretString::new),
        priority,
    };
    Ok((raw.options, prowl))
}

impl Prowl {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        message::parts(notification, 1024, 10000, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let mut fields = Pairs::default();
                fields.set("apikey", self.apikey.expose());
                fields.set("application", "tocsin");
                fields.set("event", &title);
                fields.set("description", &body);
                fields.set("priority", &self.priority.to_string());
                if let Some(providerkey) = &self.providerkey {
                    fields.set("providerkey", providerkey.expose());
                }
                PreparedRequest::with_body(
                    Method::Post,
                    "https://api.prowlapp.com/publicapi/add",
                    Some("application/x-www-form-urlencoded"),
                    encode_pairs(&fields),
                )
                .with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;

        for (key, value) in [
            ("apikey", json!(self.apikey.expose())),
            (
                "providerkey",
                json!(self.providerkey.as_ref().map(SecretString::expose)),
            ),
            ("priority", json!(self.priority)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
