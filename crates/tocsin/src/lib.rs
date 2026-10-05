//! Send one notification to many services from a single URL.
//!
//! tocsin reads the notification URLs of [Apprise](https://github.com/caronc/apprise)
//! (`tgram://...`, `slack://...`, `ntfy://...`) and turns them into HTTP
//! requests. Its URL parser is checked against Apprise itself: the test suite
//! compares every field of nearly 400 URLs with what Apprise parses.
//!
//! # Send a notification
//!
//! ```
//! # #[cfg(feature = "ntfy")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use tocsin::{MockTransport, Notification, Notifier};
//!
//! let mut notifier = Notifier::new();
//! notifier.add("ntfy://my-topic")?;
//!
//! // `MockTransport` records requests. Use `UreqTransport` to really send.
//! let mut transport = MockTransport::default();
//! let notification = Notification::new("Backup finished").title("nightly");
//! let report = notifier.send(&notification, &mut transport);
//!
//! assert!(report.is_success());
//! assert_eq!(transport.requests.len(), 1);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "ntfy"))]
//! # fn main() {}
//! ```
//!
//! # Use your own HTTP client
//!
//! [`Service::prepare`] does no I/O, so an async application can build the
//! requests here and send them with `reqwest` or any other client.
//!
//! ```
//! # #[cfg(feature = "ntfy")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use tocsin::{Notification, Service};
//!
//! let service: Service = "ntfys://user:password@ntfy.example.com/alerts".parse()?;
//! for request in service.prepare(&Notification::new("Disk almost full")) {
//!     // Send `request.method`, `request.url.expose()`, `request.headers` and
//!     // `request.body.expose()` with your client.
//!     assert_eq!(request.summary(), "POST https://ntfy.example.com");
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "ntfy"))]
//! # fn main() {}
//! ```
//!
//! With the `http` feature a request also converts to an [`http::Request`], which
//! `hyper` takes as it is and `reqwest` takes through
//! `reqwest::Request::try_from`. Timeouts, redirects and certificate checks
//! are the client's to set.
//!
//! ```
//! # #[cfg(all(feature = "ntfy", feature = "http"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use tocsin::{Notification, Service};
//!
//! let service: Service = "ntfy://my-topic".parse()?;
//! for request in service.prepare(&Notification::new("Disk almost full")) {
//!     let request = http::Request::try_from(&request)?;
//!     assert_eq!(request.method(), http::Method::POST);
//!     assert_eq!(request.uri(), "https://ntfy.sh/");
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "ntfy", feature = "http")))]
//! # fn main() {}
//! ```
//!
//! # Secrets
//!
//! URLs, tokens, request bodies and the notification text never appear in
//! [`Debug`] or [`Display`](std::fmt::Display) output, and errors never carry
//! them. Use [`SecretString::expose`] when you really need a value.
//!
//! # Features
//!
//! | Feature | Default | Enables |
//! |---|---|---|
//! | `telegram`, `discord`, `ntfy`, `gotify`, `json`, `form`, `workflows`, `mattermost`, `rocketchat`, `pushover`, `slack` | yes (via `all-services`) | that service |
//! | `ureq` | yes | [`UreqTransport`], a blocking HTTP transport |
//! | `http` | no | `TryFrom<&PreparedRequest>` for `http::Request<String>` |
//! | `compat` | no | test-only access to parsed fields |
//!
//! For the smallest build, use `default-features = false` and pick what you
//! need.

mod error;
#[cfg(feature = "_services")]
mod grammar;
#[cfg(feature = "_services")]
mod message;
#[cfg(feature = "_services")]
mod native;
mod notification;
mod notifier;
mod options;
mod plan;
mod request;
mod secret;
mod service;
#[cfg(feature = "_services")]
mod services;
mod transport;
#[cfg(feature = "ureq")]
mod transport_ureq;

pub use error::{ParseError, TransportError};
pub use notification::{Format, Kind, Notification, UnknownValue};
pub use notifier::{Notifier, Outcome, Receipt, Report};
pub use options::{Options, Overflow};
pub use plan::Plan;
pub use request::{Method, PreparedRequest, RequestPolicy};
pub use secret::{SecretBytes, SecretString};
pub use service::Service;
pub use transport::{AsyncTransport, MockTransport, Response, Transport};
#[cfg(feature = "ureq")]
pub use transport_ureq::UreqTransport;
