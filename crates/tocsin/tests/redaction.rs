//! Secrets must not leak through `Debug`, `Display`, errors or `tracing`.

use std::{
    fmt::Write,
    sync::{Arc, Mutex},
};

use tocsin::{Notification, PreparedRequest, Service};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

struct Capture(Arc<Mutex<String>>);
struct Visitor<'a>(&'a mut String);

impl Visit for Visitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        write!(self.0, "{}={value:?};", field.name()).expect("string write");
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        attrs.record(&mut Visitor(&mut self.0.lock().expect("lock")));
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, record: &Record<'_>) {
        record.record(&mut Visitor(&mut self.0.lock().expect("lock")));
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        event.record(&mut Visitor(&mut self.0.lock().expect("lock")));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[test]
fn tracing_debug_display_and_errors_never_reveal_secrets() {
    let captured = Arc::new(Mutex::new(String::new()));
    let request = PreparedRequest::json(
        "http://127.0.0.1:1/?token=FAKE_query_secret",
        &serde_json::json!({"password": "FAKE_password_secret"}),
    );
    let notification = Notification::new("FAKE_body_secret").title("FAKE_title_secret");
    tracing::subscriber::with_default(Capture(captured.clone()), || {
        tracing::event!(
            tracing::Level::TRACE,
            request = ?request,
            url = %request.url,
            body = %request.body,
            notification = ?notification
        );
        for url in [
            "tgram://FakeUser:FAKE_password_secret@123:FAKE_path_secret/1",
            "discord://FakeUser:FAKE_password_secret@FAKE_id/FAKE_path_secret",
            "ntfy://127.0.0.1/topic?token=FAKE_query_secret",
        ] {
            match Service::parse(url) {
                Ok(service) => {
                    tracing::event!(
                        tracing::Level::TRACE,
                        service = ?service,
                        display = %service,
                        options = ?service.options()
                    );
                }
                Err(error) => {
                    assert!(std::error::Error::source(&error).is_none());
                    tracing::event!(tracing::Level::TRACE, error = ?error, display = %error);
                }
            }
        }
    });
    let output = captured.lock().expect("lock");
    assert!(output.contains("REDACTED"));
    for secret in [
        "FAKE_query_secret",
        "FAKE_password_secret",
        "FAKE_path_secret",
        "FAKE_title_secret",
        "FAKE_body_secret",
        "FAKE_id",
    ] {
        assert!(!output.contains(secret), "leaked {secret}");
    }
}
