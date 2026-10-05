//! Error types. None of them ever carries a URL, a token or third-party error
//! text, so they are safe to log.

use std::{error::Error, fmt};

/// Why a notification URL could not be turned into a [`Service`](crate::Service).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// The URL is malformed.
    InvalidUrl,
    /// A required token is missing or has the wrong shape.
    InvalidToken,
    /// An option has an invalid value.
    InvalidOption,
    /// The scheme is unknown, or the service was not compiled in.
    UnsupportedService,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUrl => "invalid notification URL",
            Self::InvalidToken => "invalid notification token",
            Self::InvalidOption => "invalid notification option",
            Self::UnsupportedService => "unsupported or disabled notification service",
        })
    }
}

impl Error for ParseError {}

/// Why a request could not be delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportError {
    /// The connection failed, timed out, or the response was unreadable.
    Connection,
    /// The server answered with an error status.
    HttpStatus(u16),
    /// The request asks for something this transport refuses to do, such as
    /// disabling TLS verification.
    UnsupportedPolicy,
    /// The request itself is malformed.
    InvalidRequest,
    /// This transport cannot send the request's HTTP method.
    UnsupportedMethod,
    /// The service answered, but not in a way the next request could be built
    /// from, such as a login answer without a token.
    InvalidResponse,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection => f.write_str("notification transport failed"),
            Self::HttpStatus(code) => write!(f, "notification HTTP status {code}"),
            Self::UnsupportedPolicy => f.write_str("unsupported transport policy"),
            Self::InvalidRequest => f.write_str("invalid prepared request"),
            Self::UnsupportedMethod => f.write_str("unsupported HTTP method"),
            Self::InvalidResponse => f.write_str("unexpected notification service response"),
        }
    }
}

impl Error for TransportError {}
