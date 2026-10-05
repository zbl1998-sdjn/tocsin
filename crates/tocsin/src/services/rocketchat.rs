//! Rocket.Chat: `rocket://<webhook>@<host>[:<port>]/[<targets>]` for an incoming
//! webhook, `rocket://<user>:<token>@<host>/<targets>` for a personal access
//! token, and `rockets://...` for TLS.
//!
//! A webhook is written as two parts, `<id>/<token>`, in the place of the user
//! name. The third way, a user name and password, logs in, posts to every
//! target and logs out again, once for every piece of a long message, as
//! Apprise does. The token the login hands back is needed for the next
//! request, so this only works through [`Service::plan`](crate::Service::plan);
//! [`Service::prepare`](crate::Service::prepare) returns nothing for it.

use std::{collections::VecDeque, fmt::Write as _, sync::LazyLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Format, Method, Notification, ParseError, Plan, PreparedRequest, RequestPolicy, Response,
    SecretString, TransportError, grammar, message,
    options::{FormatMode, Options},
    plan::{Next, Sequence, Step},
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
    /// The user name and password that basic mode logs in with.
    login: Option<(String, SecretString)>,
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
    let (credentials, login) = match (mode, user, password) {
        (Mode::Token, Some(user), Some(password)) => {
            (Some((user, SecretString::new(password))), None)
        }
        (Mode::Basic, Some(user), Some(password)) => {
            (None, Some((user, SecretString::new(password))))
        }
        _ => (None, None),
    };
    let rocketchat = RocketChat {
        mode,
        webhook: webhook.map(SecretString::new),
        channels,
        rooms,
        users,
        avatar,
        credentials,
        login,
    };
    Ok((options, rocketchat))
}

/// The address of the server's API, without a trailing slash.
fn api_url(options: &Options) -> String {
    let mut api = format!(
        "{}://{}",
        if options.secure { "https" } else { "http" },
        options.host
    );
    if let Some(port) = options.port {
        write!(api, ":{port}").expect("writing to a String works");
    }
    api
}

impl RocketChat {
    /// The pieces of the message and the places to post each of them to: users
    /// first, then channels, then rooms, as Apprise does. A key is the name of
    /// the payload field the place goes in.
    fn pieces_and_places(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> (Vec<String>, Vec<(&'static str, String)>) {
        // Apprise has no separate title here: it goes in front of the body.
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let mut destinations: Vec<(&'static str, String)> = self
            .users
            .iter()
            .map(|user| ("channel", format!("@{user}")))
            .chain(self.channels.iter().map(|c| ("channel", format!("#{c}"))))
            .collect();
        let mut rooms: Vec<(&'static str, String)> = self
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
        (message::chunks(&text, 1000, options.overflow), destinations)
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        // Logging in first and using the answer needs a plan.
        if self.mode == Mode::Basic {
            return Vec::new();
        }
        let api = api_url(options);
        let (pieces, destinations) = self.pieces_and_places(options, notification);
        let url = match (&self.webhook, self.mode) {
            (Some(webhook), Mode::Webhook) => format!("{api}/hooks/{}", webhook.expose()),
            _ => format!("{api}/api/v1/chat.postMessage"),
        };
        let mut requests = Vec::new();
        for piece in pieces {
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

    /// [`prepare`](Self::prepare), plus the login and logout that basic mode
    /// needs around every piece of the message.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        let Some((user, password)) = self.login.as_ref().filter(|_| self.mode == Mode::Basic)
        else {
            return Plan::requests(self.prepare(options, notification));
        };
        let api = api_url(options);
        let (pieces, destinations) = self.pieces_and_places(options, notification);
        let mut plan = Plan::requests(Vec::new());
        for piece in pieces {
            plan = plan.then(Session {
                api: api.clone(),
                user: user.clone(),
                password: password.clone(),
                policy: options.policy(),
                payloads: destinations
                    .iter()
                    .map(|(key, place)| json!({"text": piece, *key: place}))
                    .collect(),
                stage: Stage::Login,
                auth: None,
            });
        }
        plan
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
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

#[derive(Clone, Copy)]
enum Stage {
    Login,
    Posting,
    Logout,
}

/// One piece of the message in basic mode: log in, post it to every target,
/// log out. A failed post does not keep the others from going out, and the
/// logout is sent whatever happened to the posts.
struct Session {
    api: String,
    user: String,
    password: SecretString,
    policy: RequestPolicy,
    /// The JSON payload of each post still to send.
    payloads: VecDeque<Value>,
    stage: Stage,
    /// The user id and token the login answered with.
    auth: Option<(SecretString, SecretString)>,
}

impl Session {
    fn headers(&self, request: &mut PreparedRequest) {
        if let Some((user_id, token)) = &self.auth {
            request
                .headers
                .insert("X-User-Id".to_owned(), user_id.clone());
            request
                .headers
                .insert("X-Auth-Token".to_owned(), token.clone());
        }
    }

    /// The next post, or the logout when there are none left.
    fn next_post(&mut self) -> Next {
        if let Some(payload) = self.payloads.pop_front() {
            self.stage = Stage::Posting;
            let mut request =
                PreparedRequest::json(format!("{}/api/v1/chat.postMessage", self.api), &payload)
                    .with_policy(self.policy);
            self.headers(&mut request);
            return Next::Go(Step::delivery(request));
        }
        self.stage = Stage::Logout;
        let mut request = PreparedRequest::with_body(
            Method::Post,
            format!("{}/api/v1/logout", self.api),
            None,
            Vec::<u8>::new(),
        )
        .with_policy(self.policy);
        self.headers(&mut request);
        Next::Go(Step::cleanup(request))
    }
}

/// The user id and token of a successful login answer.
fn read_login(response: &Response) -> Option<(SecretString, SecretString)> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    if answer.get("status")?.as_str()? != "success" {
        return None;
    }
    let data = answer.get("data")?;
    Some((
        SecretString::new(data.get("userId")?.as_str()?),
        SecretString::new(data.get("authToken")?.as_str()?),
    ))
}

impl Sequence for Session {
    fn start(&mut self) -> Step {
        let body = format!(
            "username={}&password={}",
            grammar::form_encode(&self.user),
            grammar::form_encode(self.password.expose())
        );
        Step::setup(
            PreparedRequest::with_body(
                Method::Post,
                format!("{}/api/v1/login", self.api),
                Some("application/x-www-form-urlencoded"),
                body,
            )
            .with_policy(self.policy),
        )
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        match self.stage {
            Stage::Login => {
                // A login that failed has nothing to log out of.
                let Ok(response) = result else {
                    return Next::Done;
                };
                let Some(auth) = read_login(response) else {
                    return Next::Fail(TransportError::InvalidResponse);
                };
                self.auth = Some(auth);
                self.next_post()
            }
            Stage::Posting => self.next_post(),
            Stage::Logout => Next::Done,
        }
    }
}
