//! A blocking HTTP transport built on `ureq` and Rustls.

use std::time::Duration;

use crate::{Method, PreparedRequest, Response, Transport, TransportError};

/// Sends requests with [`ureq`], verifying TLS with the bundled web PKI roots.
///
/// It honours `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` like most HTTP clients.
/// It refuses requests that ask for `verify=no` instead of silently
/// downgrading them.
///
/// `ureq` writes request URLs to the `log` crate at `trace` level, and tocsin
/// URLs contain tokens. Do not enable `trace` logging for `ureq`.
#[derive(Debug, Default)]
pub struct UreqTransport;

impl Transport for UreqTransport {
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError> {
        let url = request.url.expose();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(TransportError::InvalidRequest);
        }
        // `UPDATE` is not a registered method, and ureq refuses to send it.
        if request.method == Method::Update {
            return Err(TransportError::UnsupportedMethod);
        }
        let policy = request.policy;
        // A parsed URL may carry options this transport cannot honour. Refuse
        // them explicitly instead of silently weakening TLS.
        if !policy.verify_tls {
            return Err(TransportError::UnsupportedPolicy);
        }
        let connect = Duration::try_from_secs_f64(policy.connect_timeout)
            .map_err(|_| TransportError::UnsupportedPolicy)?;
        let read = Duration::try_from_secs_f64(policy.read_timeout)
            .map_err(|_| TransportError::UnsupportedPolicy)?;
        if connect.is_zero() || read.is_zero() {
            return Err(TransportError::UnsupportedPolicy);
        }
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(connect))
            .timeout_recv_response(Some(read))
            .timeout_send_request(Some(read))
            .timeout_send_body(Some(read))
            .max_redirects(if policy.redirects { 5 } else { 0 })
            .build();
        let agent: ureq::Agent = config.into();
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value.expose());
        }
        let http_request = builder
            .body(request.body.expose().to_owned())
            .map_err(|_| TransportError::InvalidRequest)?;
        let response = agent
            .run(http_request)
            .map_err(|_| TransportError::Connection)?;
        let status = response.status().as_u16();
        if status >= 400 {
            Err(TransportError::HttpStatus(status))
        } else {
            Ok(Response::new(status))
        }
    }
}
