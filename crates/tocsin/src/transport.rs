//! Sending prepared requests.

use std::future::Future;

use crate::{PreparedRequest, SecretBytes, TransportError};

/// What a successful delivery looks like.
///
/// The body is there for the few services whose next request depends on the
/// answer to the one before. It is a [`SecretBytes`], because services
/// sometimes echo the credentials they were sent, so formatting a response
/// never shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Response {
    /// The HTTP status, below 400.
    pub status: u16,
    /// The start of the response body. Transports keep at most 1 MiB of it.
    pub body: SecretBytes,
}

impl Response {
    /// A response with the given status and no body.
    #[must_use]
    pub fn new(status: u16) -> Self {
        Self {
            status,
            body: SecretBytes::default(),
        }
    }

    /// A response with a body.
    #[must_use]
    pub fn with_body(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: SecretBytes::new(body),
        }
    }
}

/// Something that can send a [`PreparedRequest`].
///
/// Implement it to use your own blocking HTTP client. An async application
/// implements [`AsyncTransport`] instead, or drives a [`Plan`](crate::Plan) with
/// its own client.
pub trait Transport {
    /// Send one request.
    ///
    /// # Errors
    ///
    /// A [`TransportError`] that must not contain the URL, headers or body. An
    /// HTTP status of 400 or more is an error, too.
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError>;
}

/// Something that can send a [`PreparedRequest`] without blocking.
///
/// The future has to be [`Send`], so that [`Notifier::send_async`] can be used
/// in a task that moves between threads.
///
/// [`Notifier::send_async`]: crate::Notifier::send_async
pub trait AsyncTransport {
    /// Send one request.
    ///
    /// # Errors
    ///
    /// The same as [`Transport::send`].
    fn send(
        &mut self,
        request: &PreparedRequest,
    ) -> impl Future<Output = Result<Response, TransportError>> + Send;
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

impl AsyncTransport for MockTransport {
    fn send(
        &mut self,
        request: &PreparedRequest,
    ) -> impl Future<Output = Result<Response, TransportError>> + Send {
        std::future::ready(Transport::send(self, request))
    }
}
