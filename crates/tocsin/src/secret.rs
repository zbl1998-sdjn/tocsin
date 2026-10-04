//! Strings that never reveal themselves through formatting.

use std::fmt;

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

#[cfg(test)]
mod tests {
    use super::SecretString;

    #[test]
    fn formatting_never_reveals_the_value() {
        let secret = SecretString::new("FAKE_value");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(secret.expose(), "FAKE_value");
    }
}
