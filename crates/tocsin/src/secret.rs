//! Strings that never reveal themselves through formatting.

use std::{borrow::Cow, fmt};

/// A string whose [`Debug`] and [`Display`](fmt::Display) output is always
/// `[REDACTED]`.
///
/// Tokens, passwords and URLs that embed credentials are wrapped in this type
/// so that logging a request, a service or an error can never leak them. Call
/// [`SecretString::expose`] when the value really is needed, for example to put
/// it on the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The wrapped value. Handle the result with care.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the wrapped value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Bytes whose [`Debug`] and [`Display`](fmt::Display) output is always
/// `[REDACTED]`: a request body, which can hold the notification text and
/// attachments, or a response body.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Wrap a value.
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(value.into())
    }

    /// The wrapped bytes. Handle the result with care.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// The wrapped bytes read as UTF-8 text, with invalid sequences replaced.
    /// Handle the result with care.
    #[must_use]
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// The number of bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretBytes, SecretString};

    #[test]
    fn bytes_never_reveal_themselves() {
        let secret = SecretBytes::new("FAKE_body");
        assert_eq!(format!("{secret:?} {secret}"), "[REDACTED] [REDACTED]");
        assert_eq!(secret.expose(), b"FAKE_body");
        assert_eq!(secret.text(), "FAKE_body");
        assert_eq!(secret.len(), 9);
        assert!(SecretBytes::default().is_empty());
        assert_eq!(SecretBytes::new(vec![0xFF, b'a']).text(), "\u{FFFD}a");
    }

    #[test]
    fn formatting_never_reveals_the_value() {
        let secret = SecretString::new("FAKE_value");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(secret.expose(), "FAKE_value");
    }
}
