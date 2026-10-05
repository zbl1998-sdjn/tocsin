//! Zulip: `zulip://<bot>@<organization>/<token>[/<stream or e-mail>...]`.
//!
//! The bot name may end in `-bot`, which is dropped. The organization is the
//! sub-domain of `zulipchat.com`, or `organization.host` for another server. A
//! target is a stream, or an e-mail address for a private message, and without
//! one the message goes to the stream `general`.

use std::sync::LazyLock;

use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;

use crate::{
    Format, Method, Notification, ParseError, PreparedRequest, SecretString, grammar,
    grammar::{Pairs, encode_pairs},
    message,
    options::{FormatMode, Options},
};

/// The start of the bot name, which is all that Apprise reads of it.
static BOT_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9_-]{1,32}").expect("static regex"));
/// The organization, and the host name after a dot.
static ORGANIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<org>[A-Z0-9_-]{1,32})(?:\.(?P<hostname>[^\s]+))?").expect("static regex")
});
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9]{32}$").expect("static regex"));

const DEFAULT_HOSTNAME: &str = "zulipchat.com";
const DEFAULT_STREAM: &str = "general";

/// A parsed `zulip://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Zulip {
    bot: String,
    organization: String,
    hostname: String,
    token: SecretString,
    /// Unique and sorted; `general` when the URL gave none.
    targets: Vec<String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Zulip), ParseError> {
    let raw = grammar::parse(input, grammar::Hosts::Any, FormatMode::Fixed(Format::Text))?;
    let options = &raw.options;
    let bot = options
        .user
        .as_deref()
        .map(grammar::strip)
        .and_then(|user| BOT_NAME.find(user))
        .map(|found| found.as_str())
        .ok_or(ParseError::InvalidOption)?;
    let bot = bot.strip_suffix("-bot").unwrap_or(bot).to_owned();
    let found = ORGANIZATION
        .captures(grammar::strip(&options.host))
        .ok_or(ParseError::InvalidOption)?;
    let organization = found["org"].to_owned();
    let hostname = found.name("hostname").map_or_else(
        || DEFAULT_HOSTNAME.to_owned(),
        |name| name.as_str().to_owned(),
    );

    let mut targets = raw.paths.clone();
    // The token is the `token` option, or else the first path segment.
    let token = match raw.query.get("token").filter(|token| !token.is_empty()) {
        Some(token) => Some(grammar::decode(token)),
        None if targets.is_empty() => None,
        None => Some(targets.remove(0)),
    };
    let token = token
        .and_then(|token| grammar::validate(&TOKEN, &token))
        .ok_or(ParseError::InvalidToken)?;
    if let Some(to) = raw.query.get("to").filter(|to| !to.is_empty()) {
        targets.extend(grammar::channel_list(&grammar::decode(to)));
    }
    let mut targets = grammar::list(&targets.join(" "));
    if targets.is_empty() {
        targets.push(DEFAULT_STREAM.to_owned());
    }
    let zulip = Zulip {
        bot,
        organization,
        hostname,
        token: SecretString::new(token),
        targets,
    };
    Ok((raw.options, zulip))
}

impl Zulip {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let url = format!(
            "https://{}.{}/api/v1/messages",
            self.organization, self.hostname
        );
        // The bot's name is the user of a basic authentication, and its token
        // the password.
        let authorization = SecretString::new(format!(
            "Basic {}",
            STANDARD.encode(format!(
                "{}-bot@{}.{}:{}",
                self.bot,
                self.organization,
                self.hostname,
                self.token.expose()
            ))
        ));
        let mut requests = Vec::new();
        for (title, body) in message::parts(notification, 60, 10000, options.overflow) {
            for target in &self.targets {
                let mut fields = Pairs::default();
                fields.set("subject", &title);
                fields.set("content", &body);
                if let Some(address) = grammar::email_of(target) {
                    fields.set("type", "private");
                    fields.set("to", &address);
                } else {
                    fields.set("type", "stream");
                    fields.set("to", target);
                }
                let mut request = PreparedRequest::with_body(
                    Method::Post,
                    &url,
                    Some("application/x-www-form-urlencoded; charset=utf-8"),
                    encode_pairs(&fields),
                )
                .with_policy(options.policy());
                request
                    .headers
                    .insert("Authorization".to_owned(), authorization.clone());
                requests.push(request);
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;

        for (key, value) in [
            ("botname", json!(self.bot)),
            ("organization", json!(self.organization)),
            ("hostname", json!(self.hostname)),
            ("token", json!(self.token.expose())),
            ("targets", json!(self.targets)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
