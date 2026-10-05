//! A non-blocking [`AsyncTransport`] for [tocsin](https://docs.rs/tocsin), built
//! on [`reqwest`].
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tocsin::{Notification, Notifier};
//! use tocsin_reqwest::ReqwestTransport;
//!
//! let mut notifier = Notifier::new();
//! notifier.add("ntfy://my-topic")?;
//!
//! let mut transport = ReqwestTransport::new();
//! let report = notifier
//!     .send_async(&Notification::new("Backup finished"), &mut transport)
//!     .await;
//! assert!(report.is_success());
//! # Ok(())
//! # }
//! ```
//!
//! A [`Notifier`](tocsin::Notifier) takes its services one after the other. To
//! send to several at once, give each its own notifier and join the futures.
//!
//! Like tocsin's blocking `UreqTransport`, it never turns TLS verification off
//! (a URL with `verify=no` is refused with
//! [`TransportError::UnsupportedPolicy`]), it keeps at most 1 MiB of a response
//! body, and no error carries a URL, a header or a body. The TLS roots are
//! reqwest's, which with the default `rustls` feature are the platform's.
//!
//! Like most HTTP clients, reqwest can write URLs to its logs at the debug and
//! trace levels, for example when it follows a redirect, and tocsin URLs contain
//! tokens. Keep those levels off for it.

use std::{collections::HashMap, future::Future, time::Duration};

use reqwest::{Client, Method as ReqwestMethod, redirect};
use tocsin::{AsyncTransport, PreparedRequest, RequestPolicy, Response, TransportError};

/// The most of a response body that is kept.
const MAX_RESPONSE_BYTES: usize = 1 << 20;

/// How many differently configured clients are kept.
const MAX_CLIENTS: usize = 8;

/// A client's settings: what a [`RequestPolicy`] changes that reqwest only
/// takes when the client is built.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Settings {
    /// The timeouts, in the bits of the `f64` seconds the policy has.
    connect: u64,
    read: u64,
    redirects: bool,
}

/// Sends tocsin's requests with [`reqwest`].
///
/// Clients are kept for reuse, one for each combination of timeouts and
/// redirect setting that the requests ask for, so connections are pooled.
#[derive(Debug, Default)]
pub struct ReqwestTransport {
    clients: HashMap<Settings, Client>,
}

impl ReqwestTransport {
    /// A transport with no client yet; they are made as requests need them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The client for a policy, or why it cannot be had.
    fn client_for(&mut self, policy: RequestPolicy) -> Result<Client, TransportError> {
        // A parsed URL may carry options this transport cannot honour. Refuse
        // them explicitly instead of silently weakening TLS.
        if !policy.verify_tls {
            return Err(TransportError::UnsupportedPolicy);
        }
        let seconds = |value: f64| {
            Duration::try_from_secs_f64(value)
                .ok()
                .filter(|duration| !duration.is_zero())
                .ok_or(TransportError::UnsupportedPolicy)
        };
        let connect = seconds(policy.connect_timeout)?;
        let read = seconds(policy.read_timeout)?;
        let settings = Settings {
            connect: policy.connect_timeout.to_bits(),
            read: policy.read_timeout.to_bits(),
            redirects: policy.redirects,
        };
        if let Some(client) = self.clients.get(&settings) {
            return Ok(client.clone());
        }
        let client = Client::builder()
            .connect_timeout(connect)
            .read_timeout(read)
            .redirect(if policy.redirects {
                redirect::Policy::limited(5)
            } else {
                redirect::Policy::none()
            })
            .build()
            .map_err(|_| TransportError::UnsupportedPolicy)?;
        if self.clients.len() >= MAX_CLIENTS {
            self.clients.clear();
        }
        self.clients.insert(settings, client.clone());
        Ok(client)
    }

    /// The request, ready to send.
    fn build(
        &mut self,
        request: &PreparedRequest,
    ) -> Result<reqwest::RequestBuilder, TransportError> {
        let url = request.url.expose();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(TransportError::InvalidRequest);
        }
        let method = ReqwestMethod::from_bytes(request.method.as_str().as_bytes())
            .map_err(|_| TransportError::UnsupportedMethod)?;
        let client = self.client_for(request.policy)?;
        let mut builder = client.request(method, url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value.expose());
        }
        Ok(builder.body(request.body.expose().to_vec()))
    }
}

impl AsyncTransport for ReqwestTransport {
    fn send(
        &mut self,
        request: &PreparedRequest,
    ) -> impl Future<Output = Result<Response, TransportError>> + Send {
        // Everything that needs the request happens here, so the future owns
        // what it needs and does not borrow the request.
        let built = self.build(request);
        async move {
            let mut response = built?
                .send()
                .await
                .map_err(|_| TransportError::Connection)?;
            let status = response.status().as_u16();
            if status >= 400 {
                return Err(TransportError::HttpStatus(status));
            }
            // A body that cannot be read in full is not a failed delivery.
            let mut body = Vec::new();
            while body.len() < MAX_RESPONSE_BYTES {
                match response.chunk().await {
                    Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                    Ok(None) | Err(_) => break,
                }
            }
            body.truncate(MAX_RESPONSE_BYTES);
            Ok(Response::with_body(status, body))
        }
    }
}
