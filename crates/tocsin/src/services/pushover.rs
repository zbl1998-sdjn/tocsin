//! Pushover: `pover://<user key>@<app token>[/<device or #group>...]`.
//!
//! Devices share one request and every group gets its own, as Pushover's API
//! wants. Apprise can also encrypt the fields end to end (`key=` and `e2ee=`);
//! tocsin does not, so a URL with a key sends nothing instead of falling back to
//! plain text.

use std::sync::LazyLock;

use regex::Regex;

use crate::{
    Format, Method, Notification, ParseError, PreparedRequest, SecretString,
    grammar::{self, Pairs, encode_pairs},
    message,
    options::{FormatMode, Options},
};

static GROUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:%23|#)(?P<group>[a-z0-9]+)\s*$").expect("static regex")
});
static DEVICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?P<device>[a-z0-9_-]{1,25})\s*$").expect("static regex")
});

/// What Apprise puts in `devices` when no target is given.
const ALL_DEVICES: &str = "ALL_DEVICES";

/// Apprise takes the first entry whose key starts the lower-cased value.
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

/// A parsed `pover://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Pushover {
    user_key: SecretString,
    token: SecretString,
    devices: Vec<String>,
    groups: Vec<String>,
    invalid_targets: Vec<String>,
    sound: String,
    priority: i8,
    /// Seconds between repeats, for emergency priority only.
    interval: Option<i64>,
    /// Seconds until repeats stop, for emergency priority only.
    expire: Option<i64>,
    url: Option<String>,
    url_title: Option<String>,
    key: Option<SecretString>,
    e2ee: bool,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Pushover), ParseError> {
    // Apprise turns every `#` before the query into `%23`.
    let (path, query) = input.split_at(input.find('?').unwrap_or(input.len()));
    let url = format!("{}{query}", path.replace('#', "%23"));
    let raw = grammar::parse(
        &url,
        grammar::Hosts::Any,
        FormatMode::Supported(&[Format::Text, Format::Html, Format::Markdown]),
    )?;
    let argument = |key: &str| {
        raw.query
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| grammar::decode(s))
    };
    let token = grammar::token(&raw.options.host).ok_or(ParseError::InvalidToken)?;
    let user_key = raw
        .options
        .user
        .as_deref()
        .and_then(grammar::token)
        .ok_or(ParseError::InvalidToken)?;
    let mut candidates = raw.paths.clone();
    if let Some(to) = argument("to") {
        candidates.extend(grammar::list(&to));
    }
    let candidates = grammar::list(&candidates.join(" "));
    let (mut devices, mut groups, mut invalid_targets) = (Vec::new(), Vec::new(), Vec::new());
    if candidates.is_empty() {
        devices.push(ALL_DEVICES.to_owned());
    }
    for target in candidates {
        if let Some(found) = GROUP.captures(&target) {
            groups.push(found["group"].to_owned());
        } else if let Some(found) = DEVICE.captures(&target) {
            devices.push(found["device"].to_owned());
        } else {
            invalid_targets.push(target);
        }
    }
    let sound = argument("sound").map_or_else(|| "pushover".to_owned(), |s| s.to_lowercase());
    let priority = argument("priority").map_or(0, |p| priority_from(&p.to_lowercase()));
    let (mut interval, mut expire) = (None, None);
    if priority == 2 {
        let number = |key: &str, default: i64| {
            raw.query
                .get(key)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(default)
        };
        let (repeat, stop) = (number("interval", 900), number("expire", 3600));
        if repeat < 30 || !(0..=10800).contains(&stop) {
            return Err(ParseError::InvalidOption);
        }
        interval = Some(repeat);
        expire = Some(stop);
    }
    let key = match argument("key") {
        None => None,
        Some(key) => {
            let key = grammar::strip(&key).to_lowercase();
            if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ParseError::InvalidOption);
            }
            Some(SecretString::new(key))
        }
    };
    let e2ee = if raw.query.get("e2ee").is_some_and(|s| !s.is_empty()) {
        raw.flag("e2ee", true)
    } else {
        true
    };
    let pushover = Pushover {
        user_key: SecretString::new(user_key),
        token: SecretString::new(token),
        devices,
        groups,
        invalid_targets,
        sound,
        priority,
        interval,
        expire,
        url: argument("url"),
        url_title: argument("url_title"),
        key,
        e2ee,
    };
    Ok((raw.options, pushover))
}

impl Pushover {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        // Fields cannot be encrypted here, and they must not go out in the clear
        // when the URL asked for encryption.
        if (self.key.is_some() && self.e2ee) || (self.devices.is_empty() && self.groups.is_empty())
        {
            return Vec::new();
        }
        let html = message::format(options, notification) == Format::Html;
        let mut requests = Vec::new();
        for (title, body) in message::parts(notification, 250, 1024, options.overflow) {
            let mut fields = Pairs::default();
            fields.set("token", self.token.expose());
            fields.set("priority", &self.priority.to_string());
            if !title.is_empty() {
                fields.set("title", &title);
            }
            fields.set("message", &body);
            fields.set("sound", &self.sound);
            if let Some(url) = &self.url {
                fields.set("url", url);
            }
            if let Some(url_title) = &self.url_title {
                fields.set("url_title", url_title);
            }
            if html {
                fields.set("html", "1");
            }
            if let (Some(interval), Some(expire)) = (self.interval, self.expire) {
                fields.set("retry", &interval.to_string());
                fields.set("expire", &expire.to_string());
            }
            let mut recipients = Vec::new();
            if !self.devices.is_empty() {
                recipients.push((
                    self.user_key.expose().to_owned(),
                    Some(self.devices.join(",")),
                ));
            }
            for group in &self.groups {
                recipients.push((group.clone(), None));
            }
            for (user, device) in recipients {
                let mut fields = fields.clone();
                fields.set("user", &user);
                // Without a device, Pushover sends to all of them.
                if let Some(device) = device.filter(|d| d != ALL_DEVICES) {
                    fields.set("device", &device);
                }
                requests.push(
                    PreparedRequest::with_body(
                        Method::Post,
                        "https://api.pushover.net/1/messages.json",
                        Some("application/x-www-form-urlencoded"),
                        encode_pairs(&fields),
                    )
                    .with_policy(options.policy()),
                );
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;

        for (key, value) in [
            ("user_key", json!(self.user_key.expose())),
            ("token", json!(self.token.expose())),
            ("devices", json!(self.devices)),
            ("groups", json!(self.groups)),
            ("invalid_targets", json!(self.invalid_targets)),
            ("sound", json!(self.sound)),
            ("priority", json!(self.priority)),
            ("interval", json!(self.interval)),
            ("expire", json!(self.expire)),
            ("url", json!(self.url)),
            ("url_title", json!(self.url_title)),
            ("key", json!(self.key.as_ref().map(SecretString::expose))),
            ("e2ee", json!(self.e2ee)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
