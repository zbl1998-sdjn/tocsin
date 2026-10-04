//! HTTP requests prepared for a transport.

use std::{collections::BTreeMap, fmt};

use crate::SecretString;

/// An HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// `GET`
    Get,
    /// `POST`
    Post,
    /// `PUT`
    Put,
    /// `PATCH`
    Patch,
    /// `DELETE`
    Delete,
    /// `HEAD`
    Head,
    /// `OPTIONS`
    Options,
    /// `UPDATE`, an unofficial method that Apprise's JSON webhook accepts.
    Update,
}

impl Method {
    /// The method name as sent on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Update => "UPDATE",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a transport should perform a request.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RequestPolicy {
    /// Seconds to wait for the connection.
    pub connect_timeout: f64,
    /// Seconds to wait for the server to respond.
    pub read_timeout: f64,
    /// Verify the server certificate.
    pub verify_tls: bool,
    /// Follow redirects.
    pub redirects: bool,
}

impl Default for RequestPolicy {
    fn default() -> Self {
        Self {
            connect_timeout: 4.0,
            read_timeout: 4.0,
            verify_tls: true,
            redirects: true,
        }
    }
}

/// A request ready to be sent by a [`Transport`](crate::Transport).
///
/// The URL, header values and body are [`SecretString`]s, so formatting a
/// request never reveals tokens.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PreparedRequest {
    /// The HTTP method.
    pub method: Method,
    /// The full URL. Often contains a token.
    pub url: SecretString,
    /// Request headers.
    pub headers: BTreeMap<String, SecretString>,
    /// The request body.
    pub body: SecretString,
    /// How to perform the request.
    pub policy: RequestPolicy,
}

impl PreparedRequest {
    /// A `POST` request with a JSON body.
    pub fn json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        Self::with_body(
            Method::Post,
            url,
            Some("application/json; charset=utf-8"),
            body.to_string(),
        )
    }

    /// A request with a text body and, optionally, its content type.
    pub(crate) fn with_body(
        method: Method,
        url: impl Into<String>,
        content_type: Option<&str>,
        body: String,
    ) -> Self {
        Self {
            method,
            url: SecretString::new(url),
            headers: content_type
                .map(|value| ("Content-Type".to_owned(), SecretString::new(value)))
                .into_iter()
                .collect(),
            body: SecretString::new(body),
            policy: RequestPolicy::default(),
        }
    }

    /// Replace the policy.
    #[must_use]
    pub fn with_policy(mut self, policy: RequestPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// `METHOD scheme://host[:port]`, without path, query, credentials,
    /// headers or body. Meant for dry-run output on the user's own terminal:
    /// the host is **not** redacted, so do not send this to shared logs.
    #[must_use]
    pub fn summary(&self) -> String {
        let url = self.url.expose();
        let origin = url
            .split_once("://")
            .map_or("<invalid>".to_owned(), |(scheme, rest)| {
                let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
                let host = authority
                    .rsplit_once('@')
                    .map_or(authority, |(_, host)| host);
                format!("{scheme}://{host}")
            });
        format!("{} {origin}", self.method)
    }
}

impl fmt::Display for PreparedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedRequest([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_drops_path_query_and_credentials() {
        let request = PreparedRequest::json(
            "https://user:FAKE_pass@example.com:8443/secret/path?token=FAKE_token",
            &serde_json::json!({}),
        );
        assert_eq!(request.summary(), "POST https://example.com:8443");
        let output = format!("{request:?} {request}");
        assert!(!output.contains("FAKE_pass") && !output.contains("FAKE_token"));
    }
}
