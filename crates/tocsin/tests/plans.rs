//! Services whose delivery needs several requests, each built from the answer
//! to the one before.
#![cfg(feature = "rocketchat")]

use serde_json::{Value, json};
use tocsin::{
    Notification, Notifier, Outcome, PreparedRequest, Response, Service, Transport, TransportError,
};

/// Answers every request with a function of it and keeps what it was asked.
struct Scripted<F> {
    answer: F,
    seen: Vec<PreparedRequest>,
}

impl<F> Scripted<F> {
    fn new(answer: F) -> Self {
        Self {
            answer,
            seen: Vec::new(),
        }
    }

    fn urls(&self) -> Vec<String> {
        self.seen
            .iter()
            .map(|r| {
                let url = r.url.expose();
                url.rsplit_once("/api/v1/")
                    .map_or(url, |(_, call)| call)
                    .to_owned()
            })
            .collect()
    }
}

impl<F: FnMut(&PreparedRequest) -> Result<Response, TransportError>> Transport for Scripted<F> {
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError> {
        self.seen.push(request.clone());
        (self.answer)(request)
    }
}

fn login_answer() -> Response {
    Response::with_body(
        200,
        r#"{"status":"success","data":{"authToken":"FAKE_token","userId":"FAKE_id"}}"#,
    )
}

/// Logs in fine and accepts everything else.
#[allow(clippy::unnecessary_wraps, reason = "the shape a transport answers in")]
fn friendly(request: &PreparedRequest) -> Result<Response, TransportError> {
    if request.url.expose().ends_with("/login") {
        Ok(login_answer())
    } else {
        Ok(Response::new(200))
    }
}

fn notifier(url: &str) -> Notifier {
    let mut notifier = Notifier::new();
    notifier.add(url).expect("URL");
    notifier
}

fn json_body(request: &PreparedRequest) -> Value {
    serde_json::from_str(&request.body.text()).expect("JSON body")
}

const BASIC: &str = "rocket://user:pass@localhost/@ann/%23ops/room1";

#[test]
fn basic_mode_logs_in_posts_to_every_target_and_logs_out() {
    let mut transport = Scripted::new(friendly);
    let report = notifier(BASIC).send(&Notification::new("Body").title("Title"), &mut transport);

    assert!(report.is_success());
    // One receipt for each target; the login and logout are not receipts.
    assert_eq!(report.receipts().len(), 3);
    assert_eq!(
        transport.urls(),
        [
            "login",
            "chat.postMessage",
            "chat.postMessage",
            "chat.postMessage",
            "logout"
        ]
    );

    let login = &transport.seen[0];
    assert_eq!(login.url.expose(), "http://localhost/api/v1/login");
    assert_eq!(login.body.text(), "username=user&password=pass");
    assert_eq!(
        login.headers["Content-Type"].expose(),
        "application/x-www-form-urlencoded"
    );
    assert!(!login.headers.contains_key("X-Auth-Token"));

    // Users, then channels, then rooms, each with what the login handed back.
    let posts = &transport.seen[1..4];
    let places: Vec<_> = posts.iter().map(json_body).collect();
    assert_eq!(
        places,
        [
            json!({"text": "# Title\nBody", "channel": "@ann"}),
            json!({"text": "# Title\nBody", "channel": "#ops"}),
            json!({"text": "# Title\nBody", "roomId": "room1"}),
        ]
    );
    for request in posts.iter().chain(&transport.seen[4..]) {
        assert_eq!(request.headers["X-User-Id"].expose(), "FAKE_id");
        assert_eq!(request.headers["X-Auth-Token"].expose(), "FAKE_token");
    }
    assert!(transport.seen[4].body.is_empty());
}

#[test]
fn the_login_is_form_encoded() {
    let mut transport = Scripted::new(friendly);
    let notifier = notifier("rocket://my%20user:pa%26ss%3Dword@localhost/room1");
    notifier.send(&Notification::new("Body"), &mut transport);
    assert_eq!(
        transport.seen[0].body.text(),
        "username=my+user&password=pa%26ss%3Dword"
    );
}

#[test]
fn a_rejected_login_ends_the_session() {
    let mut transport = Scripted::new(|_: &PreparedRequest| Err(TransportError::HttpStatus(401)));
    let report = notifier(BASIC).send(&Notification::new("Body"), &mut transport);
    assert_eq!(transport.urls(), ["login"]);
    assert_eq!(
        report
            .failures()
            .map(|r| r.outcome.clone())
            .collect::<Vec<_>>(),
        [Outcome::Failed(TransportError::HttpStatus(401))]
    );
    assert!(!report.is_success());
}

#[test]
fn a_login_answer_without_a_token_is_a_failure() {
    for answer in [
        r#"{"status":"error","message":"nope"}"#,
        r#"{"status":"success","data":{"userId":"FAKE_id"}}"#,
        "not json",
        "",
    ] {
        let mut transport =
            Scripted::new(|_: &PreparedRequest| Ok(Response::with_body(200, answer)));
        let report = notifier(BASIC).send(&Notification::new("Body"), &mut transport);
        assert_eq!(transport.urls(), ["login"], "answer: {answer}");
        assert_eq!(
            report
                .failures()
                .map(|r| r.outcome.clone())
                .collect::<Vec<_>>(),
            [Outcome::Failed(TransportError::InvalidResponse)],
            "answer: {answer}"
        );
    }
}

#[test]
fn a_failed_post_does_not_keep_the_others_or_the_logout_from_going_out() {
    let mut transport = Scripted::new(|request: &PreparedRequest| {
        if request.body.text().contains("#ops") {
            Err(TransportError::HttpStatus(500))
        } else {
            friendly(request)
        }
    });
    let report = notifier(BASIC).send(&Notification::new("Body"), &mut transport);
    assert_eq!(
        transport.urls(),
        [
            "login",
            "chat.postMessage",
            "chat.postMessage",
            "chat.postMessage",
            "logout"
        ]
    );
    assert_eq!(report.receipts().len(), 3);
    assert_eq!(report.failures().count(), 1);
}

#[test]
fn a_failed_logout_is_not_reported() {
    let mut transport = Scripted::new(|request: &PreparedRequest| {
        if request.url.expose().ends_with("/logout") {
            Err(TransportError::Connection)
        } else {
            friendly(request)
        }
    });
    let report = notifier(BASIC).send(&Notification::new("Body"), &mut transport);
    assert!(report.is_success());
}

#[test]
fn every_piece_of_a_long_message_gets_its_own_session() {
    let mut transport = Scripted::new(friendly);
    let body = "word ".repeat(300);
    let report = notifier("rocket://user:pass@localhost/room1?overflow=split")
        .send(&Notification::new(body), &mut transport);
    assert!(report.is_success());
    assert_eq!(
        transport.urls(),
        [
            "login",
            "chat.postMessage",
            "logout",
            "login",
            "chat.postMessage",
            "logout"
        ]
    );
}

#[test]
fn the_modes_that_need_no_login_are_unchanged() {
    // Webhook and token mode build every request up front.
    for url in [
        "rocket://web/token@localhost/room1",
        &format!("rocket://user:{}@localhost/room1", "t".repeat(40)),
    ] {
        let service: Service = url.parse().expect("service");
        assert_eq!(service.prepare(&Notification::new("Body")).len(), 1);
        let mut plan = service.plan(&Notification::new("Body"));
        assert!(plan.next_request().is_some());
        plan.report(Ok(Response::new(200)));
        assert!(plan.next_request().is_none());
    }
    // Basic mode has to be planned: `prepare` cannot log in.
    let service: Service = BASIC.parse().expect("service");
    assert!(service.prepare(&Notification::new("Body")).is_empty());
}
