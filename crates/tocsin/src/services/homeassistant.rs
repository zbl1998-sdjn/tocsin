//! Home Assistant: `hassio://<host>[:<port>]/<token>[/<service>...]` and
//! `hassios://...` for HTTPS.
//!
//! The token is a long-lived access token, found by its shape (`x.y.z` or 8 and
//! 64 hexadecimal digits) or taken from `token=` or `accesstoken=`; without
//! those, the last path element is the token. A service is `service`,
//! `domain.service` or either with `:identity,identity`; the domain is `notify`
//! unless given. Without a service the message is a persistent notification.
//!
//! The token goes in a bearer header and nothing else. Apprise also hands the
//! user and password of the URL to its HTTP library, which then sends them as
//! basic authentication in place of the token, and Home Assistant does not
//! accept that; tocsin ignores them. Apprise leaves `prefix=` off the address of
//! a persistent notification and sends a random id with it; tocsin puts the
//! prefix on every address, and sends an id only when `nid=` gives one.
//!
//! The identities of a service go in the `target` field, which is the one Home
//! Assistant's notify services read. Apprise sends `targets`, which their schema
//! does not allow (checked against `notify/const.py` in Home Assistant's core
//! repository on 2026-10-05; a real server has not been tried).

use std::{fmt::Write as _, sync::LazyLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// What `[a-z0-9_-]` matches in Python with `re.I`: the letters, and four more
/// that case-fold to ones in `a-z`.
const WORD: &str = r"A-Za-z0-9_\-\x{130}\x{131}\x{17f}\x{212a}";

/// A token the way Apprise recognizes one in a path.
static LONG_LIVED_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:[0-9A-Fa-f]{{8}}\.[0-9A-Fa-f]{{64}}|[{WORD}]+\.[{WORD}]+\.[{WORD}]+)\n?$"
    ))
    .expect("static regex")
});
static NOTIFICATION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^[{WORD}]+$")).expect("static regex"));
/// `[domain.]service[:identity,...]`, of which only the start has to match.
static SERVICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^[\s\x{{1c}}-\x{{1f}}]*(?:(?P<domain>[{WORD}]+)\.)?(?P<service>[{WORD}]+)(?::(?P<identities>[{WORD},]+))?"
    ))
    .expect("static regex")
});

const DEFAULT_DOMAIN: &str = "notify";
const DEFAULT_PORT: i64 = 8123;
/// How many identities share a request with `batch=yes`.
const BATCH_SIZE: usize = 10;

/// A parsed `hassio://` or `hassios://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct HomeAssistant {
    token: SecretString,
    nid: Option<String>,
    batch: bool,
    prefix: String,
    /// `(domain, service, identities)`.
    services: Services,
    /// Without any service in the URL, a persistent notification is created.
    persistent: bool,
    /// What was given as a service and is none.
    invalid_targets: Vec<String>,
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '_' | '-' | '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}'
        )
}

fn word_run(chars: &[char], from: usize) -> usize {
    chars[from.min(chars.len())..]
        .iter()
        .take_while(|c| is_word(**c))
        .count()
}

/// The look-ahead `(?=$|[\s,]+(?:[a-z0-9_-]+\.)?[a-z0-9_-]+)`: the end of the
/// text (or a last line break), or separators and then another entry.
fn entry_follows(chars: &[char], at: usize) -> bool {
    if at == chars.len() || (at + 1 == chars.len() && chars[at] == '\n') {
        return true;
    }
    let mut next = at;
    while chars
        .get(next)
        .is_some_and(|c| *c == ',' || grammar::is_python_space(*c))
    {
        next += 1;
    }
    next > at && chars.get(next).is_some_and(|c| is_word(*c))
}

/// Where the entry that starts at `start` ends, as Apprise's pattern
/// `\s*((?:W+\.)?W+(?::(?:W+(?:,+W+)+?))?)(?=...)` finds it. Its list of
/// identities is lazy, so it ends at the first place the look-ahead accepts,
/// which leaves the rest of a list to be found as entries of their own.
fn entry_at(chars: &[char], start: usize) -> Option<(usize, usize)> {
    let mut first = start;
    while chars
        .get(first)
        .is_some_and(|c| grammar::is_python_space(*c))
    {
        first += 1;
    }
    let run = word_run(chars, first);
    if run == 0 {
        return None;
    }
    // A dot after the first word makes it the domain, and without a word
    // behind the dot nothing matches, since the look-ahead fails at the dot.
    let service_end = if chars.get(first + run) == Some(&'.') {
        let service = word_run(chars, first + run + 1);
        if service == 0 {
            return None;
        }
        first + run + 1 + service
    } else {
        first + run
    };
    if chars.get(service_end) != Some(&':') {
        return entry_follows(chars, service_end).then_some((first, service_end));
    }
    let identity = word_run(chars, service_end + 1);
    if identity == 0 {
        return None;
    }
    let mut at = service_end + 1 + identity;
    loop {
        let commas = chars[at..].iter().take_while(|c| **c == ',').count();
        let word = word_run(chars, at + commas);
        if commas == 0 || word == 0 {
            return None;
        }
        at += commas + word;
        if entry_follows(chars, at) {
            return Some((first, at));
        }
    }
}

/// Apprise's `parse_domain_service_targets` for one text: the entries in it, or
/// when there are none, the text split at white space and `[ ] ; ,`.
fn entries(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if let Some((first, end)) = entry_at(&chars, start) {
            found.push(chars[first..end].iter().collect());
            start = end;
        } else {
            start += 1;
        }
    }
    if found.is_empty() {
        text.split(|c: char| matches!(c, '[' | ']' | ';' | ',') || grammar::is_python_space(c))
            .filter(|piece| !piece.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        found
    }
}

type Services = Vec<(String, String, Vec<String>)>;

/// The services of a URL, and what was given as one and is none.
fn sort_entries(elements: &[String]) -> (Services, Vec<String>) {
    let mut services = Vec::new();
    let mut invalid_targets = Vec::new();
    for entry in elements.iter().flat_map(|element| entries(element)) {
        match SERVICE.captures(&entry) {
            Some(found) => services.push((
                found
                    .name("domain")
                    .map_or(DEFAULT_DOMAIN, |domain| domain.as_str())
                    .to_owned(),
                found["service"].to_owned(),
                found
                    .name("identities")
                    .map_or_else(Vec::new, |identities| grammar::list(identities.as_str())),
            )),
            None => invalid_targets.push(entry),
        }
    }
    (services, invalid_targets)
}

pub(crate) fn parse(input: &str) -> Result<(Options, HomeAssistant), ParseError> {
    let raw = grammar::parse(input, grammar::Hosts::Any, FormatMode::Fixed(Format::Text))?;
    let argument = |name: &str| {
        raw.query
            .get(name)
            .filter(|value| !value.is_empty())
            .map(|value| grammar::decode(value))
    };

    let (token, mut elements) = match argument("accesstoken").or_else(|| argument("token")) {
        Some(token) => (Some(token), raw.paths.clone()),
        None => match raw
            .paths
            .iter()
            .position(|path| LONG_LIVED_TOKEN.is_match(path))
        {
            // What follows the token comes first, both in reverse.
            Some(at) => (
                Some(raw.paths[at].clone()),
                raw.paths[at + 1..]
                    .iter()
                    .rev()
                    .chain(raw.paths[..at].iter().rev())
                    .cloned()
                    .collect(),
            ),
            // Otherwise the last element is the token and the others are lost.
            None => (raw.paths.last().cloned(), Vec::new()),
        },
    };
    let token = token
        .and_then(|token| grammar::token(&token))
        .ok_or(ParseError::InvalidToken)?;
    if let Some(to) = argument("to") {
        elements.extend(grammar::split_path(&to));
    }
    let nid = match argument("nid") {
        Some(nid) if !nid.is_empty() => {
            Some(grammar::validate(&NOTIFICATION_ID, &nid).ok_or(ParseError::InvalidOption)?)
        }
        _ => None,
    };

    let (services, invalid_targets) = sort_entries(&elements);
    let home_assistant = HomeAssistant {
        token: SecretString::new(token),
        nid,
        batch: raw.flag("batch", false),
        prefix: argument("prefix").unwrap_or_default(),
        services,
        persistent: elements.is_empty(),
        invalid_targets,
    };
    // A plain-HTTP server without a port is on Home Assistant's own.
    let mut options = raw.options;
    if !options.secure && options.port.is_none_or(|port| port == 0) {
        options.port = Some(DEFAULT_PORT);
    }
    Ok((options, home_assistant))
}

impl HomeAssistant {
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
        base.push_str(self.prefix.trim_end_matches('/'));
        let authorization = SecretString::new(format!("Bearer {}", self.token.expose()));
        let post = |path: &str, payload: &Value| {
            let mut request = PreparedRequest::json(format!("{base}{path}"), payload)
                .with_policy(options.policy());
            request
                .headers
                .insert("Authorization".to_owned(), authorization.clone());
            request
        };
        let batch_size = if self.batch { BATCH_SIZE } else { 1 };

        let mut requests = Vec::new();
        for (title, body) in message::parts(notification, 250, 32768, options.overflow) {
            let mut payload = json!({"message": body});
            if !title.is_empty() {
                payload["title"] = json!(title);
            }
            if self.persistent {
                if let Some(nid) = &self.nid {
                    payload["notification_id"] = json!(nid);
                }
                requests.push(post(
                    "/api/services/persistent_notification/create",
                    &payload,
                ));
                continue;
            }
            for (domain, service, identities) in &self.services {
                let path = format!("/api/services/{domain}/{service}");
                if identities.is_empty() {
                    requests.push(post(&path, &payload));
                }
                for batch in identities.chunks(batch_size) {
                    let mut payload = payload.clone();
                    payload["target"] = json!(batch);
                    requests.push(post(&path, &payload));
                }
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
        let targets = if self.persistent {
            json!([[null, null, []]])
        } else {
            json!(self.services)
        };
        for (key, value) in [
            ("accesstoken", json!(self.token.expose())),
            ("nid", json!(self.nid)),
            ("batch", json!(self.batch)),
            ("targets", targets),
            ("invalid_targets", json!(self.invalid_targets)),
            ("prefix", json!(self.prefix)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
