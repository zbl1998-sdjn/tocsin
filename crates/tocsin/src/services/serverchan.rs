//! `ServerChan`: `schan://<token>`.
//!
//! Apprise does not read the token from the host but with a pattern over the
//! whole URL as it was written, so the token ends at the first character that is
//! not a letter or a digit, and nothing but the URL as written counts: no
//! upper-case scheme, no leading space, no percent escape. This does the same.

use std::sync::LazyLock;

use regex::Regex;

use crate::{
    Format, Method, Notification, ParseError, PreparedRequest, SecretString, grammar,
    grammar::{Pairs, encode_pairs},
    message,
    options::{FormatMode, Options},
};

/// Apprise's pattern for a URL that does not end in a slash: the slash after the
/// token is optional.
static OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^schan://([a-zA-Z0-9]+)/?").expect("static regex"));

/// The same for a URL that ends in a slash: the token has to be followed by one,
/// even when that slash belongs to the query.
static CLOSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^schan://([a-zA-Z0-9]+)/").expect("static regex"));

/// A parsed `schan://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct ServerChan {
    token: SecretString,
}

pub(crate) fn parse(input: &str) -> Result<(Options, ServerChan), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Text),
    )?;
    let pattern = if input.ends_with('/') { &CLOSED } else { &OPEN };
    let token = pattern
        .captures(input)
        .map(|found| found[1].to_owned())
        .and_then(|text| grammar::validate(&TOKEN_CHECK, &text))
        .ok_or(ParseError::InvalidToken)?;
    Ok((
        raw.options,
        ServerChan {
            token: SecretString::new(token),
        },
    ))
}

/// What `__init__` then checks the token against: `^[a-z0-9-]+$` under `re.I`.
/// Whatever the pattern above found passes it.
static TOKEN_CHECK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9-]+$").expect("static regex"));

impl ServerChan {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let url = format!("https://sctapi.ftqq.com/{}.send", self.token.expose());
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                let mut fields = Pairs::default();
                fields.set("title", &title);
                fields.set("desp", &body);
                PreparedRequest::with_body(
                    Method::Post,
                    &url,
                    Some("application/x-www-form-urlencoded"),
                    encode_pairs(&fields),
                )
                .with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        map.insert("token".to_owned(), serde_json::json!(self.token.expose()));
    }
}
