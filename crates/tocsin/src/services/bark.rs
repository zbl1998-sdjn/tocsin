//! Bark: `bark://[<user>:<password>@]<host>[:<port>]/<device key>[/<device key>...]`
//! and `barks://...` for HTTPS.
//!
//! Every device key gets a request of its own, in reverse order of the sorted
//! keys as Apprise sends them. A message that has no title goes out without
//! one, where Apprise fills in its own name, and tocsin does not send Apprise's
//! hosted image as the icon, only an `icon=` that the URL names.
//!
//! Apprise can also encrypt the parameters with AES-GCM (`key=`); tocsin does
//! not, so a URL with a key sends nothing instead of falling back to plain text.
//! The numbers `badge=` and `volume=` are read the way Python's `int()` reads
//! them, except that one too large for 64 bits is dropped.

use std::fmt::Write as _;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// The sounds Bark ships with.
const SOUNDS: [&str; 32] = [
    "alarm.caf",
    "anticipate.caf",
    "bell.caf",
    "birdsong.caf",
    "bloom.caf",
    "calypso.caf",
    "chime.caf",
    "choo.caf",
    "descent.caf",
    "electronic.caf",
    "fanfare.caf",
    "glass.caf",
    "gotosleep.caf",
    "healthnotification.caf",
    "horn.caf",
    "ladder.caf",
    "mailsent.caf",
    "minuet.caf",
    "multiwayinvitation.caf",
    "newmail.caf",
    "newsflash.caf",
    "noir.caf",
    "paymentsuccess.caf",
    "shake.caf",
    "sherwoodforest.caf",
    "silence.caf",
    "spell.caf",
    "suspense.caf",
    "telegraph.caf",
    "tiptoes.caf",
    "typewriters.caf",
    "update.caf",
];

const LEVELS: [&str; 4] = ["active", "timeSensitive", "passive", "critical"];

/// A parsed `bark://` or `barks://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Bark {
    /// Unique and sorted.
    targets: Vec<SecretString>,
    /// Whether Apprise would use its own image as the icon; tocsin sends none.
    include_image: bool,
    sound: Option<&'static str>,
    category: Option<String>,
    group: Option<String>,
    level: Option<&'static str>,
    click: Option<String>,
    badge: Option<i64>,
    volume: Option<i64>,
    icon: Option<String>,
    call: bool,
    /// 16, 24 or 32 ASCII characters. Messages are not sent when this is set.
    key: Option<SecretString>,
}

/// What Python's `int()` takes: white space around the number, a sign, and
/// digits that may have single underscores between them.
fn python_int(text: &str) -> Option<i64> {
    let text = grammar::strip(text);
    let (negative, digits) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let value: i64 = digits.replace('_', "").parse().ok()?;
    Some(if negative { -value } else { value })
}

pub(crate) fn parse(input: &str) -> Result<(Options, Bark), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Fixed(Format::Text),
    )?;
    // An argument that is not empty, unquoted once more as Apprise does.
    let text = |name: &str| {
        raw.query
            .get(name)
            .filter(|value| !value.is_empty())
            .map(|value| grammar::decode(value))
    };

    let mut targets = raw.paths.clone();
    if let Some(to) = raw.query.get("to") {
        targets.push(to.clone());
    }
    let targets = grammar::list(&targets.join(" "))
        .into_iter()
        .map(SecretString::new)
        .collect();

    let key = match text("key") {
        None => None,
        Some(key) if key.is_ascii() && matches!(key.len(), 16 | 24 | 32) => {
            Some(SecretString::new(key))
        }
        Some(_) => return Err(ParseError::InvalidOption),
    };
    // The first one that starts with what was written, in lower case.
    let sound = text("sound").and_then(|wanted| {
        let wanted = wanted.to_lowercase();
        SOUNDS
            .iter()
            .find(|sound| sound.starts_with(&wanted))
            .copied()
    });
    // Only the first character counts, and the case matters.
    let level = text("level").and_then(|wanted| {
        let first = wanted.chars().next()?;
        LEVELS
            .iter()
            .find(|level| level.starts_with(first))
            .copied()
    });
    let bark = Bark {
        targets,
        include_image: raw.flag("image", true),
        sound,
        category: text("category"),
        group: text("group"),
        level,
        click: text("click"),
        badge: text("badge")
            .and_then(|badge| python_int(&badge))
            .filter(|badge| *badge >= 0),
        volume: text("volume")
            .and_then(|volume| python_int(&volume))
            .filter(|volume| (0..=10).contains(volume)),
        icon: text("icon"),
        call: raw.flag("call", false),
        key,
    };
    Ok((raw.options, bark))
}

impl Bark {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        // The parameters cannot be encrypted here, and they must not go out in
        // the clear when the URL asked for encryption.
        if self.key.is_some() {
            return Vec::new();
        }
        let mut url = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port.filter(|port| *port != 0) {
            write!(url, ":{port}").expect("writing to a String works");
        }
        url.push_str("/push");
        let authorization = options
            .user
            .as_deref()
            .filter(|user| !user.is_empty())
            .map(|user| {
                // A URL with a user and no password sends an empty password.
                let password = options.password.as_ref().map_or("", SecretString::expose);
                SecretString::new(format!(
                    "Basic {}",
                    STANDARD.encode(format!("{user}:{password}"))
                ))
            });
        let markdown = matches!(options.format, FormatMode::Fixed(Format::Markdown));
        let mut requests = Vec::new();
        for (title, body) in message::parts(notification, 250, 32768, options.overflow) {
            // Apprise takes the keys from the end of its sorted list.
            for target in self.targets.iter().rev() {
                let mut payload = json!({"device_key": target.expose()});
                if !title.is_empty() {
                    payload["title"] = json!(title);
                }
                payload[if markdown { "markdown" } else { "body" }] = json!(body);
                for (field, value) in [
                    ("icon", self.icon.as_deref()),
                    ("sound", self.sound),
                    ("url", self.click.as_deref()),
                    ("level", self.level),
                    ("category", self.category.as_deref()),
                    ("group", self.group.as_deref()),
                ] {
                    if let Some(value) = value.filter(|value| !value.is_empty()) {
                        payload[field] = json!(value);
                    }
                }
                for (field, value) in [("badge", self.badge), ("volume", self.volume)] {
                    if let Some(value) = value.filter(|value| *value != 0) {
                        payload[field] = json!(value);
                    }
                }
                if self.call {
                    payload["call"] = json!(1);
                }
                let mut request =
                    PreparedRequest::json(&url, &payload).with_policy(options.policy());
                if let Some(authorization) = &authorization {
                    request
                        .headers
                        .insert("Authorization".to_owned(), authorization.clone());
                }
                requests.push(request);
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        let targets: Vec<_> = self.targets.iter().map(SecretString::expose).collect();
        for (key, value) in [
            ("targets", json!(targets)),
            ("include_image", json!(self.include_image)),
            ("sound", json!(self.sound)),
            ("category", json!(self.category)),
            ("group", json!(self.group)),
            ("level", json!(self.level)),
            ("click", json!(self.click)),
            ("badge", json!(self.badge)),
            ("volume", json!(self.volume)),
            ("icon", json!(self.icon)),
            ("call", json!(self.call)),
            (
                "encryption_key",
                json!(self.key.as_ref().map(SecretString::expose)),
            ),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
