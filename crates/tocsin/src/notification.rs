//! The message to send.

use std::{fmt, str::FromStr};

/// What kind of event a notification reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// Plain information (the default).
    #[default]
    Info,
    /// Something worked.
    Success,
    /// Something needs attention.
    Warning,
    /// Something failed.
    Failure,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Failure => "failure",
        })
    }
}

impl FromStr for Kind {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "info" => Ok(Self::Info),
            "success" => Ok(Self::Success),
            "warning" => Ok(Self::Warning),
            "failure" => Ok(Self::Failure),
            _ => Err(UnknownValue),
        }
    }
}

/// How the body of a notification is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// Plain text (the default).
    #[default]
    Text,
    /// Markdown.
    Markdown,
    /// HTML.
    Html,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Html => "html",
        })
    }
}

impl FromStr for Format {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            _ => Err(UnknownValue),
        }
    }
}

/// Returned when parsing a [`Kind`] or a [`Format`] from an unknown name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownValue;

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown value")
    }
}

impl std::error::Error for UnknownValue {}

/// A message to send to every service of a [`Notifier`](crate::Notifier).
///
/// Its [`Debug`] output hides the title and the body, because notification
/// text is often sensitive.
#[derive(Clone)]
#[non_exhaustive]
pub struct Notification {
    /// Optional title; empty means none.
    pub title: String,
    /// The message itself.
    pub body: String,
    /// The kind of event.
    pub kind: Kind,
    /// How `body` is written.
    pub format: Format,
}

impl Notification {
    /// A plain-text, [`Kind::Info`] notification with no title.
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            title: String::new(),
            body: body.into(),
            kind: Kind::Info,
            format: Format::Text,
        }
    }

    /// Set the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the kind.
    #[must_use]
    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    /// Set the body format.
    #[must_use]
    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }
}

impl fmt::Debug for Notification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Notification")
            .field("kind", &self.kind)
            .field("format", &self.format)
            .field("title", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .finish()
    }
}
