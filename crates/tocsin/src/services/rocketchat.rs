//! Rocket.Chat: `rocket://<webhook>@<host>[:<port>]/[<targets>]` for an incoming
//! webhook, `rocket://<user>:<token>@<host>/<targets>` for a personal access
//! token, and `rockets://...` for TLS.
//!
//! A webhook is written as two parts, `<id>/<token>`, in the place of the user
//! name. The third way Apprise supports, a user name and password, needs a
//! login request whose answer is needed for the next one, which a pure
//! `prepare` cannot do; such a URL parses but sends nothing.

use std::{fmt::Write as _, sync::LazyLock};

use regex::Regex;
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// `<webhook>@` in front of the host, with an optional user before it.
static WEBHOOK_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?P<schema>[^:]+://)(?:(?P<user>[^:]+):)?(?P<webhook>[a-z0-9]+(?:/|%2F)[a-z0-9]+)@(?P<url>.+)$",
    )
    .expect("static regex")
});
static IS_CHANNEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#(?P<name>[A-Za-z0-9_-]+)$").expect("static regex"));
static IS_USER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@(?P<name>[A-Za-z0-9._-]+)$").expect("static regex"));
static IS_ROOM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<name>[A-Za-z0-9]+)$").expect("static regex"));

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Webhook,
    Token,
    Basic,
}

impl Mode {
    #[cfg(feature = "compat")]
    fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Token => "token",
            Self::Basic => "basic",
        }
    }
}

/// A parsed `rocket://` or `rockets://` URL.
///
/// `avatar` is parsed and reported, but no avatar is added: Apprise points it
/// at a picture in its own repository.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct RocketChat {
    mode: Mode,
    webhook: Option<SecretString>,
    channels: Vec<String>,
    rooms: Vec<String>,
    users: Vec<String>,
    avatar: bool,
    /// The user id and access token that token mode sends as headers.
    credentials: Option<(String, SecretString)>,
}

/// Split targets into channels (`#name`), rooms (an id) and users (`@name`),
/// dropping what is none of them.
fn sort_targets(targets: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut channels, mut rooms, mut users) = (Vec::new(), Vec::new(), Vec::new());
    for target in targets {
        if let Some(found) = IS_CHANNEL.captures(target) {
            channels.push(found["name"].to_owned());
        } else if let Some(found) = IS_ROOM.captures(target) {
            rooms.push(found["name"].to_owned());
        } else if let Some(found) = IS_USER.captures(target) {
            users.push(found["name"].to_owned());
        }
    }
    (channels, rooms, users)
}

pub(crate) fn parse(input: &str) -> Result<(Options, RocketChat), ParseError> {
    // Apprise takes `<webhook>@` out of the URL before it reads the rest.
    let found = WEBHOOK_URL.captures(input);
    let rewritten = found.as_ref().map(|found| {
        format!(
            "{}{}{}",
            &found["schema"],
            found
                .name("user")
                .map_or_else(String::new, |user| format!("{}@", user.as_str())),
            &found["url"]
        )
    });
    let raw = grammar::parse(
        rewritten.as_deref().unwrap_or(input),
        grammar::Hosts::Verified,
        FormatMode::Fixed(Format::Markdown),
    )?;
    let mut options = raw.options.clone();
    let argument = |key: &str| {
        raw.query
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| grammar::decode(s))
    };
    let mut webhook = None;
    if let Some(found) = &found {
        let text = grammar::decode(&found["webhook"]);
        options.password = Some(SecretString::new(text.clone()));
        webhook = Some(text);
    }
    if let Some(text) = argument("webhook") {
        webhook = Some(text);
    }
    let requested = argument("mode").map(|mode| mode.to_lowercase());
    let mode = match requested.as_deref() {
        Some("webhook") => Some(Mode::Webhook),
        Some("token") => Some(Mode::Token),
        Some("basic") => Some(Mode::Basic),
        Some(_) => return Err(ParseError::InvalidOption),
        None => None,
    };
    let user = options.user.clone().filter(|u| !u.is_empty());
    let password = options
        .password
        .as_ref()
        .map(SecretString::expose)
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    let mode = mode.unwrap_or_else(|| {
        if webhook.is_some() {
            Mode::Webhook
        } else if password.as_deref().is_some_and(|p| p.chars().count() > 32) {
            Mode::Token
        } else {
            Mode::Basic
        }
    });
    match mode {
        Mode::Basic | Mode::Token if user.is_none() || password.is_none() => {
            return Err(ParseError::InvalidOption);
        }
        Mode::Webhook if webhook.as_deref().is_none_or(str::is_empty) => {
            return Err(ParseError::InvalidOption);
        }
        _ => {}
    }
    let mut candidates = raw.paths.clone();
    if let Some(to) = argument("to") {
        candidates.extend(grammar::list(&to));
    }
    let (channels, rooms, users) = sort_targets(&grammar::list(&candidates.join(" ")));
    if mode == Mode::Basic && rooms.is_empty() && channels.is_empty() {
        return Err(ParseError::InvalidOption);
    }
    let avatar = if raw.query.get("avatar").is_some_and(|s| !s.is_empty()) {
        raw.flag("avatar", true)
    } else {
        mode != Mode::Basic
    };
    let credentials = match (mode, user, password) {
        (Mode::Token, Some(user), Some(password)) => Some((user, SecretString::new(password))),
        _ => None,
    };
    let rocketchat = RocketChat {
        mode,
        webhook: webhook.map(SecretString::new),
        channels,
        rooms,
        users,
        avatar,
        credentials,
    };
    Ok((options, rocketchat))
}

impl RocketChat {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        // Logging in first and using the answer is not possible here.
        if self.mode == Mode::Basic {
            return Vec::new();
        }
        let mut api = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port {
            write!(api, ":{port}").expect("writing to a String works");
        }
        // Apprise has no separate title here: it goes in front of the body.
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        // Users first, then channels, then rooms, as Apprise does.
        let mut destinations: Vec<(&str, String)> = self
            .users
            .iter()
            .map(|user| ("channel", format!("@{user}")))
            .chain(self.channels.iter().map(|c| ("channel", format!("#{c}"))))
            .collect();
        let mut rooms: Vec<(&str, String)> = self
            .rooms
            .iter()
            .map(|room| {
                (
                    if self.mode == Mode::Webhook {
                        "channel"
                    } else {
                        "roomId"
                    },
                    room.clone(),
                )
            })
            .collect();
        destinations.append(&mut rooms);
        let url = match (&self.webhook, self.mode) {
            (Some(webhook), Mode::Webhook) => format!("{api}/hooks/{}", webhook.expose()),
            _ => format!("{api}/api/v1/chat.postMessage"),
        };
        let mut requests = Vec::new();
        for piece in message::chunks(&text, 1000, options.overflow) {
            let targets: Vec<Option<&(&str, String)>> = if destinations.is_empty() {
                vec![None]
            } else {
                destinations.iter().map(Some).collect()
            };
            for target in targets {
                let mut payload = json!({"text": piece});
                if let Some((key, value)) = target {
                    payload[*key] = json!(value);
                }
                let mut request =
                    PreparedRequest::json(&url, &payload).with_policy(options.policy());
                if let Some((user, token)) = &self.credentials {
                    request
                        .headers
                        .insert("X-User-Id".to_owned(), SecretString::new(user.as_str()));
                    request
                        .headers
                        .insert("X-Auth-Token".to_owned(), token.clone());
                }
                requests.push(request);
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        let headers = self.credentials.as_ref().map_or_else(
            || json!({}),
            |(user, token)| json!({"X-User-Id": user, "X-Auth-Token": token.expose()}),
        );
        for (key, value) in [
            ("mode", json!(self.mode.as_str())),
            (
                "webhook",
                json!(self.webhook.as_ref().map(SecretString::expose)),
            ),
            ("channels", json!(self.channels)),
            ("rooms", json!(self.rooms)),
            ("users", json!(self.users)),
            ("avatar", json!(self.avatar)),
            ("headers", headers),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
