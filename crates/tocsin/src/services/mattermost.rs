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
//!
//! A bot can also attach files, which it uploads to the channel first and then
//! names in the post (`file_ids`). The files go with the first part of a long
//! message, and a channel whose upload failed gets no post, as in Apprise. That
//! needs the answers to the uploads too, so it is
//! [`Service::plan`](crate::Service::plan) only as well;
//! a webhook cannot carry files.

use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    sync::LazyLock,
};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Attachment, Format, Method, Notification, ParseError, Plan, PreparedRequest, RequestPolicy,
    Response, SecretString, TransportError, grammar, message,
    multipart::Multipart,
    options::{FormatMode, Options},
    plan::{Lookups, Next, Sequence, Single, Step},
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

    /// A bot's post of `piece` to the channel with this id, with the files
    /// that were uploaded to it.
    fn bot_post(
        &self,
        base: &str,
        id: &str,
        file_ids: &[String],
        piece: &str,
        policy: RequestPolicy,
    ) -> PreparedRequest {
        let mut payload = json!({"channel_id": id, "message": piece});
        if !file_ids.is_empty() {
            payload["file_ids"] = json!(file_ids);
        }
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
                            requests.push(self.bot_post(&base, id, &[], &piece, options.policy()));
                        }
                    }
                }
            }
        }
        requests
    }

    /// [`prepare`](Self::prepare), plus the lookup of every channel that a bot
    /// was given by name and the upload of its files.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        let files: Vec<Attachment> = notification.carried(options.overflow).to_vec();
        let mut names: Vec<String> = Vec::new();
        for target in &self.targets {
            if let Target::Name(name) = target {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        let team = options.user.clone().filter(|team| !team.is_empty());
        let needs_plan = !names.is_empty() || !files.is_empty();
        if self.mode != Mode::Bot || !needs_plan || (!names.is_empty() && team.is_none()) {
            return Plan::requests(self.prepare(options, notification));
        }
        let base = self.base(options);
        let policy = options.policy();
        let pieces = Self::pieces(options, notification);
        let mattermost = self.clone();
        let deliveries = {
            let base = base.clone();
            move |found: &HashMap<String, String>| -> Vec<Box<dyn Sequence>> {
                // Every piece goes to every channel, in the order they were given.
                let mut sequences: Vec<Box<dyn Sequence>> = Vec::new();
                for (part, piece) in pieces.iter().enumerate() {
                    for target in &mattermost.targets {
                        let id = match target {
                            Target::Id(id) => Some(id),
                            Target::Name(name) => found.get(name),
                        };
                        let Some(id) = id else {
                            continue;
                        };
                        // The files go with the first part only.
                        if part == 0 && !files.is_empty() {
                            sequences.push(Box::new(ChannelPost::new(
                                &mattermost,
                                (&base, id, piece),
                                &files,
                                policy,
                            )));
                        } else {
                            sequences.extend(Single::each(vec![Step::delivery(
                                mattermost.bot_post(&base, id, &[], piece, policy),
                            )]));
                        }
                    }
                }
                sequences
            }
        };
        let Some(team) = team.filter(|_| !names.is_empty()) else {
            // Only channel ids: nothing to look up.
            let mut plan = Plan::requests(Vec::new());
            for sequence in deliveries(&HashMap::new()) {
                plan = plan.then(sequence);
            }
            return plan;
        };
        let token = self.token.clone();
        Plan::requests(Vec::new()).then(Lookups::new(
            names,
            move |name| {
                let url = format!(
                    "{base}/api/v4/teams/name/{}/channels/name/{}",
                    grammar::quote(&team),
                    grammar::quote(name)
                );
                let mut request =
                    PreparedRequest::with_body(Method::Get, url, None, Vec::<u8>::new())
                        .with_policy(policy);
                request
                    .headers
                    .insert("Accept".to_owned(), SecretString::new("application/json"));
                request.headers.insert(
                    "Authorization".to_owned(),
                    SecretString::new(format!("Bearer {}", token.expose())),
                );
                Step::setup(request)
            },
            read_channel_id,
            deliveries,
        ))
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

/// The channel id in the answer to a lookup.
fn read_channel_id(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    let id = answer.get("id")?.as_str()?;
    (!id.trim().is_empty()).then(|| id.to_owned())
}

/// The id of an uploaded file in the answer to the upload.
fn read_file_id(response: &Response) -> Option<String> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    let id = answer.get("file_infos")?.get(0)?.get("id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// The post to one channel of a message with files: upload every file to the
/// channel, then post the message that names them. A file that does not upload
/// leaves the channel without a post.
struct ChannelPost {
    mattermost: Mattermost,
    base: String,
    id: String,
    piece: String,
    policy: RequestPolicy,
    /// The uploads still to send.
    uploads: VecDeque<Step>,
    /// The ids the server gave the files that are uploaded.
    file_ids: Vec<String>,
    posted: bool,
}

impl ChannelPost {
    /// `post` is the server's address, the channel id and the text.
    fn new(
        mattermost: &Mattermost,
        post: (&str, &str, &str),
        files: &[Attachment],
        policy: RequestPolicy,
    ) -> Self {
        let (base, id, piece) = post;
        let uploads = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let name = file.name_or_default(index + 1);
                let (content_type, body) = Multipart::new()
                    .field("channel_id", id)
                    .file("files", &name, Some(file.mime_type()), file.data())
                    .finish();
                let mut request = PreparedRequest::with_body(
                    Method::Post,
                    format!("{base}/api/v4/files"),
                    Some(&content_type),
                    body,
                )
                .with_policy(policy);
                request.headers.insert(
                    "Authorization".to_owned(),
                    SecretString::new(format!("Bearer {}", mattermost.token.expose())),
                );
                Step::setup(request)
            })
            .collect();
        Self {
            mattermost: mattermost.clone(),
            base: base.to_owned(),
            id: id.to_owned(),
            piece: piece.to_owned(),
            policy,
            uploads,
            file_ids: Vec::new(),
            posted: false,
        }
    }

    /// The next upload, or the post once every file is up.
    fn next(&mut self) -> Next {
        if let Some(upload) = self.uploads.pop_front() {
            return Next::go(upload);
        }
        self.posted = true;
        Next::go(Step::delivery(self.mattermost.bot_post(
            &self.base,
            &self.id,
            &self.file_ids,
            &self.piece,
            self.policy,
        )))
    }
}

impl Sequence for ChannelPost {
    fn start(&mut self) -> Step {
        self.uploads
            .pop_front()
            .expect("uploads are planned only when there is a file to upload")
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        if self.posted {
            return Next::done();
        }
        match result.map(read_file_id) {
            Ok(Some(id)) => {
                self.file_ids.push(id);
                self.next()
            }
            // The server did not say which file it stored: no post.
            Ok(None) => Next::fail(TransportError::InvalidResponse),
            // A failed upload is already a failure of the plan: no post.
            Err(_) => Next::done(),
        }
    }
}
