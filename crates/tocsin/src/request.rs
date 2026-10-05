//! HTTP requests prepared for a transport.

use std::{collections::BTreeMap, fmt};

use crate::{SecretBytes, SecretString};

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
    pub body: SecretBytes,
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

    /// A request with a body and, optionally, its content type.
    pub(crate) fn with_body(
        method: Method,
        url: impl Into<String>,
        content_type: Option<&str>,
        body: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            method,
            url: SecretString::new(url),
            headers: content_type
                .map(|value| ("Content-Type".to_owned(), SecretString::new(value)))
                .into_iter()
                .collect(),
            body: SecretBytes::new(body),
            policy: RequestPolicy::default(),
        }
    }

    /// Replace the policy.
    #[must_use]
    pub fn with_policy(mut self, policy: RequestPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Send the user and password of the URL as basic authentication. A user
    /// without a password sends an empty one, where Apprise sends the text
    /// `None`.
    #[cfg(feature = "_basic")]
    #[must_use]
    pub(crate) fn with_basic_auth(mut self, options: &crate::options::Options) -> Self {
        use base64::{Engine, engine::general_purpose::STANDARD};

        if let Some(user) = options.user.as_deref().filter(|user| !user.is_empty()) {
            let password = options.password.as_ref().map_or("", SecretString::expose);
            let credentials = STANDARD.encode(format!("{user}:{password}"));
            self.headers.insert(
                "Authorization".to_owned(),
                SecretString::new(format!("Basic {credentials}")),
            );
        }
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

/// Hand a request to any client that speaks the `http` crate, such as `hyper`
/// or, through `reqwest::Request::try_from`, `reqwest`.
///
/// Headers, method, URL and body carry over. The [`RequestPolicy`] does not:
/// timeouts, redirects and certificate checks belong to the client you send it
/// with, so set them there.
#[cfg(feature = "http")]
impl TryFrom<&PreparedRequest> for http::Request<Vec<u8>> {
    type Error = crate::TransportError;

    fn try_from(request: &PreparedRequest) -> Result<Self, Self::Error> {
        let mut builder = http::Request::builder()
            .method(request.method.as_str())
            .uri(request.url.expose());
        for (name, value) in &request.headers {
            builder = builder.header(name, value.expose());
        }
        builder
            .body(request.body.expose().to_vec())
            .map_err(|_| crate::TransportError::InvalidRequest)
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

    #[cfg(feature = "http")]
    #[test]
    fn converts_to_an_http_request() {
        let mut request = PreparedRequest::json(
            "https://example.com/hook?token=FAKE_token",
            &serde_json::json!({"a": 1}),
        );
        request.method = Method::Put;
        request
            .headers
            .insert("X-Key".into(), SecretString::new("FAKE_header"));
        let converted = http::Request::try_from(&request).expect("valid request");
        assert_eq!(converted.method(), http::Method::PUT);
        assert_eq!(converted.uri(), "https://example.com/hook?token=FAKE_token");
        assert_eq!(converted.headers()["x-key"], "FAKE_header");
        assert_eq!(converted.body().as_slice(), br#"{"a":1}"#);

        request.url = SecretString::new("not a url");
        assert_eq!(
            http::Request::try_from(&request).expect_err("bad URL"),
            crate::TransportError::InvalidRequest
        );
    }
}
