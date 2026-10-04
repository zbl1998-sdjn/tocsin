//! Mattermost: `mmost://[team@]<host>[:<port>][/<path>]/<token>[?to=<channel>...]`
//! and `mmosts://...`.
//!
//! Two modes, chosen with `?mode=`. In `webhook` mode (the default) the token is
//! an incoming webhook and the targets are channel names. In `bot` mode the
//! token is a bot access token and the targets are channel ids; a channel
//! *name* needs a lookup request first, which a pure `prepare` cannot make, so
//! those targets are skipped.

use std::{fmt::Write as _, sync::LazyLock};

use regex::Regex;
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
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
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let mut base = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port {
            write!(base, ":{port}").expect("writing to a String works");
        }
        base.push_str(self.path.trim_end_matches('/'));
        // Apprise has no separate title here: it goes in front of the body.
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let mut requests = Vec::new();
        for piece in message::chunks(&text, 4000, options.overflow) {
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
                    let url = format!("{base}/api/v4/posts");
                    for target in &self.targets {
                        let Target::Id(id) = target else {
                            continue;
                        };
                        let payload = json!({"channel_id": id, "message": piece});
                        let mut request =
                            PreparedRequest::json(&url, &payload).with_policy(options.policy());
                        request.headers.insert(
                            "Authorization".to_owned(),
                            SecretString::new(format!("Bearer {}", self.token.expose())),
                        );
                        requests.push(request);
                    }
                }
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
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
