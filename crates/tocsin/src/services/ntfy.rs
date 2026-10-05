//! ntfy topics: `ntfy://<topic>` (ntfy.sh) or `ntfys://[user:pass@]host[:port]/<topic>`.
//!
//! A message without attachments is one JSON publish. Attachments are published
//! the other way ntfy knows: each file is the body of a request to the topic,
//! named in the `filename` query parameter, and the title and the text go in the
//! query parameters of the first one.

use std::{fmt::Write as _, sync::LazyLock};

use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;
use serde_json::json;

use crate::{
    Format, Method, Notification, ParseError, PreparedRequest, SecretString,
    grammar::{self, Pairs, Raw, is_hostname},
    message,
    options::{FormatMode, Options},
};

static TOPIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z0-9_-]{1,64}$").expect("static regex"));
static TOKEN_USER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^tk_[^ \t]+").expect("static regex"));

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Cloud,
    Private,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Auth {
    Basic,
    Token,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Priority {
    Min,
    Low,
    Default,
    High,
    Max,
}

impl Priority {
    fn as_str(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Low => "low",
            Self::Default => "default",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

/// A parsed `ntfy://` or `ntfys://` URL.
///
/// `image` is parsed and reported, but the picture Apprise links in its own
/// repository is not added; only an explicit `avatar_url` is sent.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Ntfy {
    targets: Vec<String>,
    mode: Mode,
    auth: Auth,
    token: Option<SecretString>,
    image: bool,
    avatar_url: Option<String>,
    priority: Priority,
    click: Option<String>,
    delay: Option<String>,
    email: Option<String>,
    tags: Vec<String>,
    actions: Option<String>,
    attach: Option<String>,
    filename: Option<String>,
}

/// Apprise matches the first letters of the priority name, or a digit 1 to 5.
fn priority_from(wanted: &str) -> Priority {
    [
        ("l", Priority::Low),
        ("mo", Priority::Low),
        ("n", Priority::Default),
        ("h", Priority::High),
        ("e", Priority::Max),
        ("mi", Priority::Min),
        ("ma", Priority::Max),
        ("d", Priority::Default),
        ("1", Priority::Min),
        ("2", Priority::Low),
        ("3", Priority::Default),
        ("4", Priority::High),
        ("5", Priority::Max),
    ]
    .into_iter()
    .find(|(prefix, _)| wanted.starts_with(prefix))
    .map_or(Priority::Default, |(_, priority)| priority)
}

/// Decide between basic and token authentication, and find the token.
///
/// A `?token=` wins; otherwise an `tk_...` user, or the password, or the user,
/// is taken as the token when token authentication is chosen.
fn resolve_auth(raw: &Raw) -> Result<(Auth, Option<String>), ParseError> {
    let mut token = raw
        .query
        .get("token")
        .filter(|s| !s.is_empty())
        .map(|s| grammar::decode(s));
    let user = raw.options.user.as_deref().unwrap_or("");
    let password = raw
        .options
        .password
        .as_ref()
        .map_or("", SecretString::expose);
    let explicit = raw
        .query
        .get("auth")
        .filter(|s| !s.is_empty())
        .map(|s| grammar::decode(&s.to_lowercase()));
    let auth = match explicit.as_deref() {
        Some("basic") => Auth::Basic,
        Some("token") => Auth::Token,
        Some(_) => return Err(ParseError::InvalidOption),
        None => {
            if token.is_some() || (password.is_empty() && TOKEN_USER.is_match(user)) {
                Auth::Token
            } else {
                Auth::Basic
            }
        }
    };
    if auth == Auth::Token && token.is_none() {
        token = if !password.is_empty() {
            Some(grammar::decode(password))
        } else if !user.is_empty() {
            Some(grammar::decode(user))
        } else {
            None
        };
    }
    Ok((auth, token))
}

pub(crate) fn parse(input: &str) -> Result<(Options, Ntfy), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Text),
    )?;
    let mut candidates = raw.paths.clone();
    if let Some(to) = raw.query.get("to") {
        candidates.extend(grammar::list(&grammar::decode(to)));
    }
    let explicit_mode = raw
        .query
        .get("mode")
        .filter(|s| !s.is_empty())
        .map(|s| grammar::decode(&s.to_lowercase()));
    let mode = match explicit_mode.as_deref() {
        Some("cloud") => Mode::Cloud,
        Some("private") => Mode::Private,
        Some(_) => return Err(ParseError::InvalidOption),
        None => {
            if is_hostname(&raw.original_host).is_some() && !candidates.is_empty() {
                Mode::Private
            } else {
                Mode::Cloud
            }
        }
    };
    if mode == Mode::Private && is_hostname(&raw.original_host).is_none() {
        return Err(ParseError::InvalidUrl);
    }
    if mode == Mode::Cloud && !raw.original_host.to_lowercase().contains("ntfy.sh") {
        candidates.push(raw.original_host.clone());
    }
    let targets: Vec<String> = grammar::list(&candidates.join(" "))
        .into_iter()
        .filter(|s| TOPIC.is_match(s))
        .collect();
    let (auth, token) = resolve_auth(&raw)?;
    let priority = priority_from(
        &raw.query
            .get("priority")
            .map(|s| s.to_lowercase())
            .unwrap_or_default(),
    );
    let tags = raw
        .query
        .get("xtags")
        .filter(|s| !s.is_empty())
        .or_else(|| raw.query.get("tags"))
        .map(|s| grammar::list(&grammar::decode(s)))
        .unwrap_or_default();
    let non_empty = |key: &str| {
        raw.query
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| grammar::decode(s))
    };
    let attach = non_empty("attach");
    let filename = non_empty("filename").or_else(|| {
        attach.as_ref().and_then(|s| {
            s.rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    });
    let ntfy = Ntfy {
        targets,
        mode,
        auth,
        token: token.map(SecretString::new),
        image: raw.flag("image", true),
        avatar_url: raw.optional("avatar_url"),
        priority,
        click: non_empty("click"),
        delay: non_empty("delay"),
        email: non_empty("email"),
        tags,
        actions: non_empty("actions"),
        attach,
        filename,
    };
    Ok((raw.options, ntfy))
}

impl Ntfy {
    /// The headers every request carries, and the policy.
    fn finish(&self, mut request: PreparedRequest, options: &Options) -> PreparedRequest {
        let mut header = |name: &str, value: String| {
            request
                .headers
                .insert(name.to_owned(), SecretString::new(value));
        };
        if self.mode == Mode::Private {
            match (self.auth, &self.token) {
                (Auth::Token, Some(token)) => {
                    header("Authorization", format!("Bearer {}", token.expose()));
                }
                (Auth::Basic, _) => {
                    if let Some(user) = options.user.as_deref().filter(|u| !u.is_empty()) {
                        let password = options.password.as_ref().map_or("", SecretString::expose);
                        header(
                            "Authorization",
                            format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
                        );
                    }
                }
                (Auth::Token, None) => {}
            }
        }
        if matches!(options.format, FormatMode::Fixed(Format::Markdown)) {
            header("X-Markdown", "yes".to_owned());
        }
        if self.priority != Priority::Default {
            header("X-Priority", self.priority.as_str().to_owned());
        }
        for (value, name) in [
            (&self.delay, "X-Delay"),
            (&self.click, "X-Click"),
            (&self.email, "X-Email"),
            (&self.actions, "X-Actions"),
        ] {
            if let Some(value) = value {
                header(name, value.clone());
            }
        }
        if !self.tags.is_empty() {
            header("X-Tags", self.tags.join(","));
        }
        if self.image {
            if let Some(icon) = self.avatar_url.as_deref().filter(|u| !u.is_empty()) {
                header("X-Icon", icon.to_owned());
            }
        }
        request.with_policy(options.policy())
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let private = self.mode == Mode::Private;
        if private && self.auth == Auth::Token && self.token.is_none() {
            return Vec::new();
        }
        let mut url = if private {
            format!(
                "{}://{}",
                if options.secure { "https" } else { "http" },
                options.host
            )
        } else {
            "https://ntfy.sh".to_owned()
        };
        if private {
            if let Some(port) = options.port {
                write!(url, ":{port}").expect("writing to a String works");
            }
        }
        let pieces = message::parts(notification, 200, 7800, options.overflow);
        let carried = notification.carried(options.overflow);
        let mut requests = Vec::new();
        for topic in self.targets.iter().rev() {
            for (part, (title, chunk)) in pieces.iter().enumerate() {
                // The files go with the first part of a long message only.
                let files = if part == 0 { carried } else { &[] };
                let mut built = Vec::new();
                if files.is_empty() {
                    let mut payload = json!({"topic": topic});
                    if let Some(attach) = self.attach.as_deref().filter(|a| !a.is_empty()) {
                        payload["attach"] = json!(attach);
                        if let Some(filename) = self.filename.as_deref().filter(|f| !f.is_empty()) {
                            payload["filename"] = json!(filename);
                        }
                    }
                    if !title.is_empty() {
                        payload["title"] = json!(title);
                    }
                    if !chunk.is_empty() {
                        payload["message"] = json!(chunk);
                    }
                    built.push(PreparedRequest::json(&url, &payload));
                } else {
                    for (index, attachment) in files.iter().enumerate() {
                        let mut query = Pairs::default();
                        query.set("filename", &attachment.name_or_default(index + 1));
                        // The text goes with the first file only.
                        if index == 0 {
                            if !title.is_empty() {
                                query.set("title", title);
                            }
                            if !chunk.is_empty() {
                                query.set("message", chunk);
                            }
                        }
                        built.push(PreparedRequest::with_body(
                            Method::Post,
                            format!("{url}/{topic}?{}", grammar::encode_pairs(&query)),
                            None,
                            attachment.data(),
                        ));
                    }
                }
                for request in built {
                    requests.push(self.finish(request, options));
                }
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("targets", json!(self.targets)),
            (
                "mode",
                json!(match self.mode {
                    Mode::Cloud => "cloud",
                    Mode::Private => "private",
                }),
            ),
            (
                "auth",
                json!(match self.auth {
                    Auth::Basic => "basic",
                    Auth::Token => "token",
                }),
            ),
            (
                "token",
                json!(self.token.as_ref().map(SecretString::expose)),
            ),
            ("image", json!(self.image)),
            ("avatar_url", json!(self.avatar_url)),
            ("priority", json!(self.priority.as_str())),
            ("click", json!(self.click)),
            ("delay", json!(self.delay)),
            ("email", json!(self.email)),
            ("tags", json!(self.tags)),
            ("actions", json!(self.actions)),
            ("attach", json!(self.attach)),
            ("filename", json!(self.filename)),
            ("payload", json!({})),
            ("headers", json!({})),
            ("query", json!({})),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
