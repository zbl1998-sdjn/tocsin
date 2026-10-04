//! Options shared by every service URL (`?format=`, `?overflow=`, `?verify=`, ...).

use std::fmt;

use crate::{Format, SecretString};

/// What to do with a message that is longer than a service accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Overflow {
    /// Send it as is and let the service decide (the default).
    Upstream,
    /// Cut it to the maximum length.
    Truncate,
    /// Send it as several messages.
    Split,
}

impl fmt::Display for Overflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Upstream => "upstream",
            Self::Truncate => "truncate",
            Self::Split => "split",
        })
    }
}

/// Which body formats a service understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatMode {
    /// Several formats; the notification's own format is used when supported.
    #[cfg_attr(
        not(any(feature = "telegram", feature = "discord")),
        allow(dead_code, reason = "only telegram and discord offer a choice")
    )]
    Supported(&'static [Format]),
    /// Exactly one format.
    #[cfg_attr(
        not(any(feature = "ntfy", feature = "gotify")),
        allow(dead_code, reason = "only ntfy and gotify have a fixed format")
    )]
    Fixed(Format),
}

/// The options every notification URL accepts, as parsed from the URL.
///
/// Some options are parsed and validated for Apprise compatibility but not yet
/// applied by tocsin: [`retry`](Self::retry), [`wait`](Self::wait),
/// [`optional`](Self::optional), [`emojis`](Self::emojis),
/// [`store`](Self::store) and [`timezone`](Self::timezone).
///
/// Hosts, users and passwords can hold credentials, so they have no accessors
/// and the [`Debug`] output omits them.
// One bool per URL flag, mirroring Apprise's option set.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub struct Options {
    pub(crate) host: String,
    pub(crate) port: Option<i64>,
    pub(crate) user: Option<String>,
    pub(crate) password: Option<SecretString>,
    pub(crate) secure: bool,
    pub(crate) format: FormatMode,
    pub(crate) format_override: Option<Format>,
    pub(crate) overflow: Overflow,
    pub(crate) verify_tls: bool,
    pub(crate) redirects: bool,
    pub(crate) connect_timeout: f64,
    pub(crate) read_timeout: f64,
    pub(crate) retry: u8,
    pub(crate) wait: f64,
    pub(crate) optional: bool,
    pub(crate) emojis: Option<bool>,
    pub(crate) store: bool,
    pub(crate) tz: Option<String>,
}

impl Options {
    /// Whether the scheme asked for TLS (`ntfys://` rather than `ntfy://`).
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// What to do with an over-long message.
    #[must_use]
    pub fn overflow(&self) -> Overflow {
        self.overflow
    }

    /// Whether the server certificate is verified.
    #[must_use]
    pub fn verify_tls(&self) -> bool {
        self.verify_tls
    }

    /// Whether redirects are followed.
    #[must_use]
    pub fn follow_redirects(&self) -> bool {
        self.redirects
    }

    /// Seconds to wait for a connection.
    #[must_use]
    pub fn connect_timeout(&self) -> f64 {
        self.connect_timeout
    }

    /// Seconds to wait for a response.
    #[must_use]
    pub fn read_timeout(&self) -> f64 {
        self.read_timeout
    }

    /// Requested retries, 0 to 10. Parsed, not yet applied.
    #[must_use]
    pub fn retry(&self) -> u8 {
        self.retry
    }

    /// Requested pause between retries in seconds. Parsed, not yet applied.
    #[must_use]
    pub fn wait(&self) -> f64 {
        self.wait
    }

    /// Whether failures of this service are tolerated. Parsed, not yet applied.
    #[must_use]
    pub fn optional(&self) -> bool {
        self.optional
    }

    /// Whether `:emoji:` shortcodes are interpreted. Parsed, not yet applied.
    #[must_use]
    pub fn emojis(&self) -> Option<bool> {
        self.emojis
    }

    /// Whether persistent storage is allowed. Parsed, not yet applied.
    #[must_use]
    pub fn store(&self) -> bool {
        self.store
    }

    /// The requested time zone name. Parsed, not yet applied.
    #[must_use]
    pub fn timezone(&self) -> Option<&str> {
        self.tz.as_deref()
    }

    #[cfg(feature = "_services")]
    pub(crate) fn policy(&self) -> crate::request::RequestPolicy {
        crate::request::RequestPolicy {
            connect_timeout: self.connect_timeout,
            read_timeout: self.read_timeout,
            verify_tls: self.verify_tls,
            redirects: self.redirects,
        }
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::{Value, json};

        let format = match self.format {
            FormatMode::Supported(list) => {
                Value::Array(list.iter().map(|f| json!(f.to_string())).collect())
            }
            FormatMode::Fixed(format) => json!(format.to_string()),
        };
        for (key, value) in [
            ("host", json!(self.host)),
            ("port", json!(self.port)),
            ("user", json!(self.user)),
            (
                "password",
                json!(self.password.as_ref().map(SecretString::expose)),
            ),
            ("secure", json!(self.secure)),
            ("format", format),
            (
                "format_override",
                json!(self.format_override.map(|f| f.to_string())),
            ),
            ("overflow", json!(self.overflow.to_string())),
            ("verify", json!(self.verify_tls)),
            ("redirect", json!(self.redirects)),
            ("cto", json!(self.connect_timeout)),
            ("rto", json!(self.read_timeout)),
            ("retry", json!(self.retry)),
            ("wait", json!(self.wait)),
            ("optional", json!(self.optional)),
            ("emojis", json!(self.emojis)),
            ("store", json!(self.store)),
            ("tz", json!(self.tz)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("secure", &self.secure)
            .field("overflow", &self.overflow)
            .field("verify_tls", &self.verify_tls)
            .field("redirects", &self.redirects)
            .finish_non_exhaustive()
    }
}
