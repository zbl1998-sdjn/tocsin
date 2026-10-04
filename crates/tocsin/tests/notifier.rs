//! `Notifier` and its report.
#![cfg(feature = "ntfy")]

use tocsin::{
    MockTransport, Notification, Notifier, Outcome, PreparedRequest, Response, Transport,
    TransportError,
};

/// Fails every request whose body mentions "boom", succeeds for the rest.
struct Flaky;

impl Transport for Flaky {
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError> {
        if request.body.expose().contains("boom") {
            Err(TransportError::HttpStatus(500))
        } else {
            Ok(Response::new(204))
        }
    }
}

#[test]
fn delivers_to_every_service() {
    let mut notifier = Notifier::new();
    notifier.add("ntfy://first").expect("first");
    notifier.add("ntfy://second").expect("second");
    let mut transport = MockTransport::default();
    let report = notifier.send(&Notification::new("hi"), &mut transport);
    assert!(report.is_success());
    assert_eq!(report.receipts().len(), 2);
    assert_eq!(transport.requests.len(), 2);
    assert_eq!(report.failures().count(), 0);
}

#[test]
fn one_failure_does_not_stop_the_others() {
    let mut notifier = Notifier::new();
    notifier.add("ntfy://one").expect("one");
    notifier.add("ntfy://two").expect("two");
    let mut transport = Flaky;
    let report = notifier.send(&Notification::new("boom"), &mut transport);
    assert!(!report.is_success());
    assert_eq!(report.receipts().len(), 2);
    assert!(report.receipts().iter().all(|r| r.service == "ntfy"));
    assert_eq!(
        report.failures().next().map(|r| r.outcome),
        Some(Outcome::Failed(TransportError::HttpStatus(500)))
    );
}

#[test]
fn a_service_with_nothing_to_send_is_reported() {
    let mut notifier = Notifier::new();
    notifier
        .add("ntfy://127.0.0.1/topic?auth=token")
        .expect("tokenless");
    let mut transport = MockTransport::default();
    let report = notifier.send(&Notification::new("hi"), &mut transport);
    assert!(!report.is_success());
    assert_eq!(report.receipts()[0].outcome, Outcome::NothingToSend);
    assert!(transport.requests.is_empty());
}

#[test]
fn an_empty_notifier_is_not_a_success() {
    let report = Notifier::new().send(&Notification::new("hi"), &mut MockTransport::default());
    assert!(!report.is_success());
    assert_eq!(report.receipts().len(), 0);
}

#[test]
fn bad_urls_are_rejected_when_added() {
    let mut notifier = Notifier::new();
    assert!(notifier.add("not a url").is_err());
    assert!(notifier.add("nosuch://thing").is_err());
    assert!(notifier.services().is_empty());
}
