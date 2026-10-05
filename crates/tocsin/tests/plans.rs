//! Services whose delivery needs several requests, each built from the answer
//! to the one before.
#![cfg(any(feature = "rocketchat", feature = "mattermost"))]

use serde_json::Value;
use tocsin::{Notifier, PreparedRequest, Response, Transport, TransportError};

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

    /// `METHOD /path` of every request, in the order they were sent.
    fn calls(&self) -> Vec<String> {
        self.seen
            .iter()
            .map(|r| {
                let url = r.url.expose();
                let path = url
                    .split_once("://")
                    .and_then(|(_, rest)| rest.find('/').map(|at| &rest[at..]))
                    .unwrap_or("");
                format!("{} {path}", r.method)
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

fn notifier(url: &str) -> Notifier {
    let mut notifier = Notifier::new();
    notifier.add(url).expect("URL");
    notifier
}

fn json_body(request: &PreparedRequest) -> Value {
    serde_json::from_str(&request.body.text()).expect("JSON body")
}

#[cfg(feature = "rocketchat")]
mod rocketchat {
    use serde_json::json;
    use tocsin::{Notification, Outcome, Response, Service, TransportError};

    use super::*;

    /// The Rocket.Chat call each request made, such as `login`.
    fn steps<F>(transport: &Scripted<F>) -> Vec<String> {
        transport
            .calls()
            .iter()
            .map(|call| call.trim_start_matches("POST /api/v1/").to_owned())
            .collect()
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

    const BASIC: &str = "rocket://user:pass@localhost/@ann/%23ops/room1";

    #[test]
    fn basic_mode_logs_in_posts_to_every_target_and_logs_out() {
        let mut transport = Scripted::new(friendly);
        let report =
            notifier(BASIC).send(&Notification::new("Body").title("Title"), &mut transport);

        assert!(report.is_success());
        // One receipt for each target; the login and logout are not receipts.
        assert_eq!(report.receipts().len(), 3);
        assert_eq!(
            steps(&transport),
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
        let mut transport =
            Scripted::new(|_: &PreparedRequest| Err(TransportError::HttpStatus(401)));
        let report = notifier(BASIC).send(&Notification::new("Body"), &mut transport);
        assert_eq!(steps(&transport), ["login"]);
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
            assert_eq!(steps(&transport), ["login"], "answer: {answer}");
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
            steps(&transport),
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
            steps(&transport),
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
}

#[cfg(feature = "mattermost")]
mod mattermost {
    use serde_json::json;
    use tocsin::{Notification, Outcome, Response, TransportError};

    use super::*;

    const BOT: &str = "mmost://team%20one@localhost/FAKE_bot?mode=bot&to=%23town,%2Bid9,%23ops";

    #[allow(clippy::unnecessary_wraps, reason = "the shape a transport answers in")]
    fn lookup_answer(request: &PreparedRequest) -> Result<Response, TransportError> {
        let url = request.url.expose();
        if url.contains("/channels/name/") {
            let name = url.rsplit('/').next().unwrap_or("");
            Ok(Response::with_body(
                200,
                json!({"id": format!("id-of-{name}")}).to_string(),
            ))
        } else {
            Ok(Response::new(201))
        }
    }

    #[test]
    fn a_bot_looks_channel_names_up_before_posting_to_every_channel() {
        let mut transport = Scripted::new(lookup_answer);
        let report = notifier(BOT).send(&Notification::new("Body"), &mut transport);

        assert!(report.is_success());
        assert_eq!(report.receipts().len(), 3, "the lookups are not receipts");
        assert_eq!(
            transport.calls(),
            [
                "GET /api/v4/teams/name/team%20one/channels/name/ops",
                "GET /api/v4/teams/name/team%20one/channels/name/town",
                "POST /api/v4/posts",
                "POST /api/v4/posts",
                "POST /api/v4/posts",
            ]
        );
        let lookup = &transport.seen[0];
        assert_eq!(lookup.headers["Authorization"].expose(), "Bearer FAKE_bot");
        assert_eq!(lookup.headers["Accept"].expose(), "application/json");
        assert!(lookup.body.is_empty());

        // Channels keep their order; each name got the id its lookup returned.
        let ids: Vec<_> = transport.seen[2..]
            .iter()
            .map(|post| json_body(post)["channel_id"].clone())
            .collect();
        assert_eq!(ids, [json!("id-of-ops"), json!("id-of-town"), json!("id9")]);
        assert_eq!(
            transport.seen[2].headers["Authorization"].expose(),
            "Bearer FAKE_bot"
        );
    }

    #[test]
    fn a_name_is_looked_up_once_however_often_it_is_posted_to() {
        let mut transport = Scripted::new(lookup_answer);
        let notification = Notification::new("word ".repeat(1_500));
        let report = notifier("mmost://localhost/FAKE_bot?mode=bot&team=t&to=%23a&overflow=split")
            .send(&notification, &mut transport);
        assert!(report.is_success());
        let lookups = transport
            .calls()
            .iter()
            .filter(|call| call.starts_with("GET"))
            .count();
        assert_eq!(lookups, 1);
        assert_eq!(transport.seen.len(), 1 + report.receipts().len());
        assert!(report.receipts().len() > 1);
    }

    #[test]
    fn a_name_that_cannot_be_resolved_does_not_keep_the_others_from_going_out() {
        for answer in [
            Err(TransportError::HttpStatus(404)),
            Ok(Response::with_body(200, "not json")),
            Ok(Response::with_body(200, r#"{"id":"  "}"#)),
        ] {
            let mut transport = Scripted::new(|request: &PreparedRequest| {
                if request.url.expose().ends_with("/channels/name/town") {
                    answer.clone()
                } else {
                    lookup_answer(request)
                }
            });
            let report = notifier(BOT).send(&Notification::new("Body"), &mut transport);
            // Two lookups and the two posts that could be made.
            assert_eq!(transport.seen.len(), 4, "answer: {answer:?}");
            assert_eq!(report.receipts().len(), 3, "answer: {answer:?}");
            let failures: Vec<_> = report.failures().map(|r| r.outcome.clone()).collect();
            let expected = match &answer {
                Err(error) => Outcome::Failed(*error),
                Ok(_) => Outcome::Failed(TransportError::InvalidResponse),
            };
            assert_eq!(failures, [expected], "answer: {answer:?}");
        }
    }

    #[test]
    fn only_ids_need_no_lookup() {
        let mut transport = Scripted::new(lookup_answer);
        let report = notifier("mmost://localhost/FAKE_bot?mode=bot&to=%2Bid1,id2")
            .send(&Notification::new("Body"), &mut transport);
        assert!(report.is_success());
        assert_eq!(
            transport.calls(),
            ["POST /api/v4/posts", "POST /api/v4/posts"]
        );
    }

    #[test]
    fn webhook_mode_is_unchanged() {
        let mut transport = Scripted::new(lookup_answer);
        let report = notifier("mmost://localhost/FAKE_hook?to=%23ops")
            .send(&Notification::new("Body"), &mut transport);
        assert!(report.is_success());
        assert_eq!(transport.calls(), ["POST /hooks/FAKE_hook"]);
    }
}

#[cfg(feature = "slack")]
mod slack {
    use serde_json::json;
    use tocsin::{Notification, Outcome, Response, TransportError};

    use super::*;

    const BOT: &str = "slack://xoxb-1234-1234-abc124/bob@example.com/%23ops/ann@example.org";
    const HOOK: &str = "slack://TFAKE1/BFAKE2/CFAKE3/ops";

    /// Slack's Web API: a user for every address, and `ok` for the rest.
    #[allow(clippy::unnecessary_wraps, reason = "the shape a transport answers in")]
    fn api(request: &PreparedRequest) -> Result<Response, TransportError> {
        let url = request.url.expose();
        let answer = match url.split_once("email=") {
            Some((_, address)) => {
                let user = address.split("%40").next().unwrap_or("");
                json!({"ok": true, "user": {"id": format!("U-{user}")}})
            }
            None => json!({"ok": true, "channel": "C1"}),
        };
        Ok(Response::with_body(200, answer.to_string()))
    }

    fn failures(report: &tocsin::Report) -> Vec<Outcome> {
        report.failures().map(|r| r.outcome.clone()).collect()
    }

    #[test]
    fn a_bot_looks_users_up_by_e_mail_before_messaging_them() {
        let mut transport = Scripted::new(api);
        let report = notifier(BOT).send(&Notification::new("Body"), &mut transport);

        assert!(report.is_success(), "{:?}", failures(&report));
        assert_eq!(report.receipts().len(), 3, "the lookups are not receipts");
        assert_eq!(
            transport.calls(),
            [
                "POST /api/chat.postMessage",
                "GET /api/users.lookupByEmail?email=ann%40example.org",
                "GET /api/users.lookupByEmail?email=bob%40example.com",
                "POST /api/chat.postMessage",
                "POST /api/chat.postMessage",
            ]
        );
        let channels: Vec<_> = [0, 3, 4]
            .map(|at| json_body(&transport.seen[at])["channel"].clone())
            .into();
        assert_eq!(channels, [json!("#ops"), json!("U-ann"), json!("U-bob")]);
        for request in &transport.seen {
            assert_eq!(
                request.headers["Authorization"].expose(),
                "Bearer xoxb-1234-1234-abc124"
            );
        }
    }

    #[test]
    fn an_e_mail_address_is_encoded_like_a_query_value() {
        let mut transport = Scripted::new(api);
        let service = "slack://xoxb-1234-1234-abc124/a+b@example.com";
        notifier(service).send(&Notification::new("Body"), &mut transport);
        assert_eq!(
            transport.calls()[0],
            "GET /api/users.lookupByEmail?email=a%2Bb%40example.com"
        );
    }

    #[test]
    fn an_address_that_cannot_be_resolved_does_not_keep_the_others_from_going_out() {
        // Missing scope: Slack answers 200, but not `ok`.
        let mut transport = Scripted::new(|request: &PreparedRequest| {
            if request.url.expose().ends_with("ann%40example.org") {
                Ok(Response::with_body(
                    200,
                    r#"{"ok":false,"error":"missing_scope"}"#,
                ))
            } else {
                api(request)
            }
        });
        let report = notifier(BOT).send(&Notification::new("Body"), &mut transport);
        // The channel, both lookups, and the one message that could be sent.
        assert_eq!(transport.seen.len(), 4);
        assert_eq!(report.receipts().len(), 3);
        assert_eq!(
            failures(&report),
            [Outcome::Failed(TransportError::Rejected)]
        );
    }

    #[test]
    fn a_bot_message_slack_did_not_accept_is_a_failure() {
        for answer in [
            r#"{"ok":false,"error":"channel_not_found"}"#,
            "not json",
            "",
        ] {
            let mut transport =
                Scripted::new(|_: &PreparedRequest| Ok(Response::with_body(200, answer)));
            let report = notifier("slack://xoxb-1234-1234-abc124/ops")
                .send(&Notification::new("Body"), &mut transport);
            assert_eq!(
                failures(&report),
                [Outcome::Failed(TransportError::Rejected)],
                "answer: {answer}"
            );
        }
    }

    #[test]
    fn a_webhook_has_to_answer_ok() {
        for (answer, delivered) in [("ok", true), ("invalid_token", false), ("", false)] {
            let mut transport =
                Scripted::new(|_: &PreparedRequest| Ok(Response::with_body(200, answer)));
            let report = notifier(HOOK).send(&Notification::new("Body"), &mut transport);
            assert_eq!(report.is_success(), delivered, "answer: {answer}");
        }
    }

    #[test]
    fn targets_that_cannot_be_used_are_failures_not_silence() {
        let mut transport = Scripted::new(|_: &PreparedRequest| Ok(Response::with_body(200, "ok")));
        // An address needs a bot to look it up, and `bad!name` is no channel.
        let url = "slack://TFAKE1/BFAKE2/CFAKE3/ops/bob@example.com/bad!name";
        let report = notifier(url).send(&Notification::new("Body"), &mut transport);
        assert_eq!(transport.seen.len(), 1, "only the channel is sent to");
        assert_eq!(report.receipts().len(), 3);
        assert_eq!(
            failures(&report),
            [
                Outcome::Failed(TransportError::InvalidRequest),
                Outcome::Failed(TransportError::InvalidRequest),
            ]
        );
    }

    #[test]
    fn workflows_only_need_a_good_status() {
        let mut transport = Scripted::new(|_: &PreparedRequest| Ok(Response::new(200)));
        let url = "slack://TFAKE1/Ft07XXXX/XXXXXXXX/YYYYYYYY/?mode=workflow";
        let report = notifier(url).send(&Notification::new("Body"), &mut transport);
        assert!(report.is_success());
    }
}
