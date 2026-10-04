//! Sending prepared requests.

use crate::{PreparedRequest, TransportError};

/// What a successful delivery looks like. Response bodies are discarded on
/// purpose: services sometimes echo the credentials they were sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Response {
    /// The HTTP status, below 400.
    pub status: u16,
}

impl Response {
    /// A response with the given status.
    #[must_use]
    pub fn new(status: u16) -> Self {
        Self { status }
    }
}

/// Something that can send a [`PreparedRequest`].
///
/// Implement it to use your own HTTP client. [`Service::prepare`] is pure, so
/// an async application can also send the prepared requests itself.
///
/// [`Service::prepare`]: crate::Service::prepare
pub trait Transport {
    /// Send one request.
    ///
    /// # Errors
    ///
    /// A [`TransportError`] that must not contain the URL, headers or body.
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError>;
}

/// A transport that records requests instead of sending them. For tests and
/// dry runs; it never touches the network.
#[derive(Debug, Default)]
pub struct MockTransport {
    /// Every request that was "sent".
    pub requests: Vec<PreparedRequest>,
}

impl Transport for MockTransport {
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError> {
        self.requests.push(request.clone());
        Ok(Response::new(200))
    }
}
