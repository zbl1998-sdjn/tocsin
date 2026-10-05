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
        if request.body.text().contains("boom") {
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
        report.failures().next().map(|r| r.outcome.clone()),
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

/// Runs a future to completion on the current thread. The futures in these
/// tests never wait on anything, so the first poll finishes them.
fn block_on<F: Future>(future: F) -> F::Output {
    use std::{
        pin::pin,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };

    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }

    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

#[test]
fn async_sending_gives_the_same_report_as_blocking_sending() {
    use tocsin::AsyncTransport;

    let mut notifier = Notifier::new();
    notifier.add("ntfy://one").expect("one");
    notifier.add("ntfy://two").expect("two");
    let notification = Notification::new("hi");

    let mut blocking = MockTransport::default();
    let expected = notifier.send(&notification, &mut blocking);

    let mut asynchronous = MockTransport::default();
    let actual = block_on(notifier.send_async(&notification, &mut asynchronous));

    assert_eq!(actual, expected);
    let summaries = |transport: &MockTransport| {
        transport
            .requests
            .iter()
            .map(PreparedRequest::summary)
            .collect::<Vec<_>>()
    };
    assert_eq!(summaries(&asynchronous), summaries(&blocking));

    // The adapter itself is usable on its own.
    let request = asynchronous.requests[0].clone();
    let response = block_on(AsyncTransport::send(&mut asynchronous, &request));
    assert_eq!(response.map(|r| r.status), Ok(200));
}

#[test]
fn async_sending_keeps_going_after_a_failure() {
    use tocsin::AsyncTransport;

    struct AsyncFlaky;
    impl AsyncTransport for AsyncFlaky {
        async fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError> {
            Flaky.send(request)
        }
    }

    let mut notifier = Notifier::new();
    notifier.add("ntfy://one").expect("one");
    notifier.add("ntfy://two").expect("two");
    let report = block_on(notifier.send_async(&Notification::new("boom"), &mut AsyncFlaky));
    assert_eq!(report.receipts().len(), 2);
    assert_eq!(report.failures().count(), 2);
}

#[test]
fn a_plan_can_be_driven_by_hand() {
    let service: tocsin::Service = "ntfy://my-topic".parse().expect("service");
    let mut plan = service.plan(&Notification::new("hi"));
    let first = plan.next_request().expect("one request");
    assert_eq!(first.url.expose(), "https://ntfy.sh");
    assert!(first.body.text().contains("my-topic"));
    plan.report(Ok(Response::new(200)));
    assert!(plan.next_request().is_none());
    assert_eq!(plan.finish(), [Outcome::Delivered(Response::new(200))]);

    let empty: tocsin::Service = "ntfy://127.0.0.1/topic?auth=token"
        .parse()
        .expect("tokenless");
    assert_eq!(
        empty.plan(&Notification::new("hi")).finish(),
        [Outcome::NothingToSend]
    );
}
