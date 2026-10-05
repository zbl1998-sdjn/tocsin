//! Mattermost: `mmost://[team@]<host>[:<port>][/<path>]/<token>[?to=<channel>...]`
//! and `mmosts://...`.
//!
//! Two modes, chosen with `?mode=`. In `webhook` mode (the default) the token is
//! an incoming webhook and the targets are channel names. In `bot` mode the
//! token is a bot access token and the targets are channel ids, or channel
//! names, which are looked up in the team first (once for each name, before
//! anything is posted). The lookup's answer is needed to build the post, so
//! names only work through [`Service::plan`](crate::Service::plan);
//! [`Service::prepare`](crate::Service::prepare) skips them.

use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    sync::LazyLock,
};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Format, Method, Notification, ParseError, Plan, PreparedRequest, RequestPolicy, Response,
    SecretString, TransportError, grammar, message,
    options::{FormatMode, Options},
    plan::{Next, Sequence, Step},
};

static IS_CHANNEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:#|%23)(?P<name>[A-Za-z0-9_-]+)$").expect("static regex"));
static IS_CHANNEL_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\+|%2B)?(?P<name>[A-Za-z0-9_-]+)$").expect("static regex"));

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Webhook,
    Bot,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Bot => "bot",
        }
    }
}

/// A channel written as a name (`#`) or as an id (`+`).
#[derive(Clone, PartialEq, Eq)]
enum Target {
    Name(String),
    Id(String),
}

/// A parsed `mmost://` or `mmosts://` URL.
///
/// `image` is parsed and reported, but no icon is added: Apprise points it at a
/// picture in its own repository. Only an explicit `icon_url` is sent.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Mattermost {
    mode: Mode,
    token: SecretString,
    /// The base path, `""` or `/a/b`.
    path: String,
    targets: Vec<Target>,
    invalid_targets: Vec<String>,
    image: bool,
    icon_url: Option<String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Mattermost), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Fixed(Format::Text),
    )?;
    let mut options = raw.options.clone();
    let mut segments = raw.paths.clone();
    let token = segments
        .pop()
        .and_then(|segment| grammar::token(&segment))
        .ok_or(ParseError::InvalidToken)?;
    let path = if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    };
    let argument = |key: &str| {
        raw.query
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| grammar::decode(s))
    };
    let mut requested_mode = argument("mode");
    if let Some(team) = argument("team") {
        options.user = Some(team);
        requested_mode.get_or_insert_with(|| "bot".to_owned());
    } else if let Some(botname) = argument("botname") {
        options.user = Some(botname);
    }
    let mode = match requested_mode
        .as_deref()
        .map(|mode| mode.trim().to_lowercase())
        .filter(|mode| !mode.is_empty())
    {
        None => Mode::Webhook,
        Some(wanted) => [Mode::Webhook, Mode::Bot]
            .into_iter()
            .find(|mode| mode.as_str().starts_with(&wanted))
            .ok_or(ParseError::InvalidOption)?,
    };
    let candidates: Vec<String> = ["to", "channel", "channels"]
        .iter()
        .filter_map(|key| argument(key))
        .flat_map(|value| grammar::list(&value))
        .collect();
    let has_team = options.user.as_deref().is_some_and(|user| !user.is_empty());
    let mut targets = Vec::new();
    let mut invalid_targets = Vec::new();
    for target in grammar::list(&candidates.join(" ")) {
        if let Some(found) = IS_CHANNEL.captures(&target) {
            if mode == Mode::Bot && !has_team {
                invalid_targets.push(target);
            } else {
                targets.push(Target::Name(found["name"].to_owned()));
            }
        } else if let Some(found) = IS_CHANNEL_ID.captures(&target) {
            targets.push(if mode == Mode::Webhook {
                Target::Name(found["name"].to_owned())
            } else {
                Target::Id(found["name"].to_owned())
            });
        } else {
            invalid_targets.push(target);
        }
    }
    let mattermost = Mattermost {
        mode,
        token: SecretString::new(token),
        path,
        targets,
        invalid_targets,
        image: raw.flag("image", true),
        icon_url: raw.query.get("icon_url").map(|s| grammar::decode(s)),
    };
    Ok((options, mattermost))
}

impl Mattermost {
    /// The server's address with the base path, without a trailing slash.
    fn base(&self, options: &Options) -> String {
        let mut base = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port {
            write!(base, ":{port}").expect("writing to a String works");
        }
        base.push_str(self.path.trim_end_matches('/'));
        base
    }

    /// The message in the pieces that fit one post. Apprise has no separate
    /// title here: it goes in front of the body.
    fn pieces(options: &Options, notification: &Notification) -> Vec<String> {
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        message::chunks(&text, 4000, options.overflow)
    }

    /// A bot's post of `piece` to the channel with this id.
    fn bot_post(
        &self,
        base: &str,
        id: &str,
        piece: &str,
        policy: RequestPolicy,
    ) -> PreparedRequest {
        let payload = json!({"channel_id": id, "message": piece});
        let mut request =
            PreparedRequest::json(format!("{base}/api/v4/posts"), &payload).with_policy(policy);
        request.headers.insert(
            "Authorization".to_owned(),
            SecretString::new(format!("Bearer {}", self.token.expose())),
        );
        request
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let base = self.base(options);
        let mut requests = Vec::new();
        for piece in Self::pieces(options, notification) {
            match self.mode {
                Mode::Webhook => {
                    let url = format!("{base}/hooks/{}", self.token.expose());
                    let channels: Vec<Option<&str>> = if self.targets.is_empty() {
                        vec![None]
                    } else {
                        self.targets
                            .iter()
                            .map(|target| match target {
                                Target::Name(name) | Target::Id(name) => Some(name.as_str()),
                            })
                            .collect()
                    };
                    for channel in channels {
                        let mut payload = json!({
                            "text": piece,
                            "username": options.user.as_deref().filter(|u| !u.is_empty()).unwrap_or("tocsin"),
                        });
                        if let Some(icon) = self.icon_url.as_deref().filter(|i| !i.is_empty()) {
                            payload["icon_url"] = json!(icon);
                        }
                        if let Some(channel) = channel {
                            payload["channel"] = json!(channel);
                        }
                        requests.push(
                            PreparedRequest::json(&url, &payload).with_policy(options.policy()),
                        );
                    }
                }
                Mode::Bot => {
                    for target in &self.targets {
                        if let Target::Id(id) = target {
                            requests.push(self.bot_post(&base, id, &piece, options.policy()));
                        }
                    }
                }
            }
        }
        requests
    }

    /// [`prepare`](Self::prepare), plus the lookup of every channel that a bot
    /// was given by name.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        let mut names: Vec<String> = Vec::new();
        for target in &self.targets {
            if let Target::Name(name) = target {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        let team = options.user.clone().filter(|team| !team.is_empty());
        let (Mode::Bot, Some(team), false) = (self.mode, team, names.is_empty()) else {
            return Plan::requests(self.prepare(options, notification));
        };
        Plan::requests(Vec::new()).then(Lookups {
            mattermost: self.clone(),
            base: self.base(options),
            team,
            policy: options.policy(),
            pieces: Self::pieces(options, notification),
            pending: names.into(),
            asked: None,
            found: HashMap::new(),
            posts: None,
        })
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
        let targets: Vec<_> = self
            .targets
            .iter()
            .map(|target| match target {
                Target::Name(name) => json!(["#", name]),
                Target::Id(id) => json!(["+", id]),
            })
            .collect();
        for (key, value) in [
            ("mode", json!(self.mode.as_str())),
            ("token", json!(self.token.expose())),
            ("path", json!(self.path)),
            ("targets", json!(targets)),
            ("image", json!(self.image)),
            ("icon_url", json!(self.icon_url)),
            ("invalid_targets", json!(self.invalid_targets)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}

/// A bot that was given channels by name: look every name up, then post to
/// every channel. A name that cannot be resolved is a failure of its own and
/// does not keep the others from being posted to.
struct Lookups {
    mattermost: Mattermost,
    base: String,
    team: String,
    policy: RequestPolicy,
    pieces: Vec<String>,
    /// The names still to look up.
    pending: VecDeque<String>,
    /// The name whose answer is awaited.
    asked: Option<String>,
    /// The channel id of each name that was resolved.
    found: HashMap<String, String>,
    /// The posts still to send, once every name was looked up.
    posts: Option<VecDeque<PreparedRequest>>,
}

impl Lookups {
    fn lookup(&mut self, name: String) -> Step {
        let url = format!(
            "{}/api/v4/teams/name/{}/channels/name/{}",
            self.base,
            grammar::quote(&self.team),
            grammar::quote(&name)
        );
        let mut request = PreparedRequest::with_body(Method::Get, url, None, Vec::<u8>::new())
            .with_policy(self.policy);
        request
            .headers
            .insert("Accept".to_owned(), SecretString::new("application/json"));
        request.headers.insert(
            "Authorization".to_owned(),
            SecretString::new(format!("Bearer {}", self.mattermost.token.expose())),
        );
        self.asked = Some(name);
        Step::setup(request)
    }

    /// The next lookup, or the next post once every name was looked up.
    fn advance(&mut self) -> Option<Step> {
        if let Some(name) = self.pending.pop_front() {
            return Some(self.lookup(name));
        }
        if self.posts.is_none() {
            let mut posts = VecDeque::new();
            for piece in &self.pieces {
                for target in &self.mattermost.targets {
                    let id = match target {
                        Target::Id(id) => Some(id),
                        Target::Name(name) => self.found.get(name),
                    };
                    if let Some(id) = id {
                        posts.push_back(self.mattermost.bot_post(
                            &self.base,
                            id,
                            piece,
                            self.policy,
                        ));
                    }
                }
            }
            self.posts = Some(posts);
        }
        self.posts
            .as_mut()
            .and_then(VecDeque::pop_front)
            .map(Step::delivery)
    }
}

/// The channel id in the answer to a lookup.
fn read_channel_id(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    let id = answer.get("id")?.as_str()?;
    (!id.trim().is_empty()).then(|| id.to_owned())
}

impl Sequence for Lookups {
    fn start(&mut self) -> Step {
        self.advance()
            .expect("a lookup is planned only when there is a name to look up")
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        let Some(name) = self.asked.take() else {
            // The answer to a post.
            return self.advance().map_or_else(Next::done, Next::go);
        };
        match result.map(read_channel_id) {
            Ok(Some(id)) => {
                self.found.insert(name, id);
                self.advance().map_or_else(Next::done, Next::go)
            }
            // The answer is no channel: a failure the plan cannot see.
            Ok(None) => Next::skip(TransportError::InvalidResponse, self.advance()),
            // A failed request is already a failure of the plan.
            Err(_) => self.advance().map_or_else(Next::done, Next::go),
        }
    }
}
