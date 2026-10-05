//! Services whose delivery needs several requests, each built from the answer
//! to the one before.
#![cfg(any(
    feature = "rocketchat",
    feature = "mattermost",
    feature = "slack",
    feature = "telegram"
))]

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

    /// The server of the lookups, and of the uploads, which get the id `F-` and
    /// the name of the file.
    #[allow(clippy::unnecessary_wraps, reason = "the shape a transport answers in")]
    fn server(request: &PreparedRequest) -> Result<Response, TransportError> {
        if request.url.expose().ends_with("/api/v4/files") {
            let body = request.body.text();
            let name = body
                .split("filename=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or("");
            let answer = json!({"file_infos": [{"id": format!("F-{name}")}]});
            return Ok(Response::with_body(201, answer.to_string()));
        }
        lookup_answer(request)
    }

    fn files() -> Notification {
        Notification::new("Body")
            .attach(tocsin::Attachment::new("a.txt", b"one".to_vec()))
            .attach(tocsin::Attachment::new("b.png", b"two".to_vec()))
    }

    #[test]
    fn a_bot_uploads_files_to_the_channel_and_names_them_in_the_post() {
        let mut transport = Scripted::new(server);
        let url = "mmost://localhost/FAKE_bot?mode=bot&to=%2Bid1";
        let report = notifier(url).send(&files(), &mut transport);
        assert!(report.is_success());
        assert_eq!(report.receipts().len(), 1, "the uploads are not receipts");
        assert_eq!(
            transport.calls(),
            [
                "POST /api/v4/files",
                "POST /api/v4/files",
                "POST /api/v4/posts"
            ]
        );
        let first = &transport.seen[0];
        assert!(
            first.headers["Content-Type"]
                .expose()
                .starts_with("multipart/form-data; boundary=")
        );
        assert_eq!(first.headers["Authorization"].expose(), "Bearer FAKE_bot");
        let body = first.body.text();
        assert!(
            body.contains("name=\"channel_id\"\r\n\r\nid1\r\n"),
            "{body}"
        );
        assert!(
            body.contains(
                "name=\"files\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\none\r\n"
            ),
            "{body}"
        );
        assert_eq!(
            json_body(&transport.seen[2]),
            json!({"channel_id": "id1", "message": "Body", "file_ids": ["F-a.txt", "F-b.png"]})
        );
    }

    #[test]
    fn every_channel_gets_its_own_uploads() {
        let mut transport = Scripted::new(server);
        let url = "mmost://team@localhost/FAKE_bot?mode=bot&to=%23ops,%2Bid1";
        let report = notifier(url).send(&files(), &mut transport);
        assert!(report.is_success());
        // The name is looked up, then each channel gets uploads and a post.
        assert_eq!(transport.calls().len(), 1 + 2 * 3);
        let channels: Vec<_> = transport
            .seen
            .iter()
            .filter(|r| r.url.expose().ends_with("/api/v4/files"))
            .map(|r| {
                let body = r.body.text();
                body.contains("name=\"channel_id\"\r\n\r\nid1\r\n")
            })
            .collect();
        // Channels are sorted: the name `ops` first, then `id1`.
        assert_eq!(channels, [false, false, true, true]);
    }

    #[test]
    fn a_failed_upload_leaves_that_channel_without_a_post() {
        let mut transport = Scripted::new(|request: &PreparedRequest| {
            let for_id1 = request
                .body
                .text()
                .contains("name=\"channel_id\"\r\n\r\nid1");
            if request.url.expose().ends_with("/api/v4/files") && for_id1 {
                Err(TransportError::HttpStatus(413))
            } else {
                server(request)
            }
        });
        let url = "mmost://localhost/FAKE_bot?mode=bot&to=%2Bid1,%2Bid2";
        let report = notifier(url).send(&files(), &mut transport);
        // id1: the first upload fails, nothing else is sent for it.
        // id2: both uploads and the post.
        assert_eq!(transport.calls().len(), 1 + 3);
        assert_eq!(report.receipts().len(), 2);
        assert_eq!(
            report
                .failures()
                .map(|r| r.outcome.clone())
                .collect::<Vec<_>>(),
            [Outcome::Failed(TransportError::HttpStatus(413))]
        );
    }

    #[test]
    fn an_upload_without_a_file_id_is_a_failure() {
        let mut transport = Scripted::new(|request: &PreparedRequest| {
            if request.url.expose().ends_with("/api/v4/files") {
                Ok(Response::with_body(201, r#"{"file_infos":[]}"#))
            } else {
                server(request)
            }
        });
        let url = "mmost://localhost/FAKE_bot?mode=bot&to=%2Bid1";
        let report = notifier(url).send(&files(), &mut transport);
        assert_eq!(transport.calls(), ["POST /api/v4/files"]);
        assert_eq!(
            report
                .failures()
                .map(|r| r.outcome.clone())
                .collect::<Vec<_>>(),
            [Outcome::Failed(TransportError::InvalidResponse)]
        );
    }

    #[test]
    fn the_files_go_with_the_first_part_of_a_long_message() {
        let mut transport = Scripted::new(server);
        let url = "mmost://localhost/FAKE_bot?mode=bot&to=%2Bid1&overflow=split";
        let notification = Notification::new("word ".repeat(1000))
            .attach(tocsin::Attachment::new("a.txt", b"one".to_vec()));
        let report = notifier(url).send(&notification, &mut transport);
        assert!(report.is_success());
        assert_eq!(
            transport.calls(),
            [
                "POST /api/v4/files",
                "POST /api/v4/posts",
                "POST /api/v4/posts"
            ]
        );
        assert!(json_body(&transport.seen[1]).get("file_ids").is_some());
        assert!(json_body(&transport.seen[2]).get("file_ids").is_none());
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

#[cfg(feature = "telegram")]
mod telegram {
    use serde_json::json;
    use tocsin::{Notification, Outcome, Response, Service, TransportError};

    use super::*;

    const DETECTING: &str = "tgram://123456789:abcdefghijklmn";

    fn updates(
        answer: &'static str,
    ) -> impl FnMut(&PreparedRequest) -> Result<Response, TransportError> {
        move |request: &PreparedRequest| {
            if request.url.expose().ends_with("/getUpdates") {
                Ok(Response::with_body(200, answer))
            } else {
                Ok(Response::new(200))
            }
        }
    }

    const WROTE: &str = r#"{"ok":true,"result":[
        {"update_id":1,"edited_message":{}},
        {"update_id":2,"message":{"from":{"id":9007199254740993,"first_name":"Ann"}}},
        {"update_id":3,"message":{"from":{"id":7}}}]}"#;

    #[test]
    fn without_a_chat_id_the_message_goes_to_whoever_wrote_to_the_bot() {
        let mut transport = Scripted::new(updates(WROTE));
        let report = notifier(DETECTING).send(&Notification::new("Body"), &mut transport);
        assert!(report.is_success());
        assert_eq!(
            transport.calls(),
            [
                "POST /bot123456789:abcdefghijklmn/getUpdates",
                "POST /bot123456789:abcdefghijklmn/sendMessage",
            ]
        );
        assert!(transport.seen[0].body.is_empty());
        // The id keeps every digit.
        assert!(
            transport.seen[1]
                .body
                .text()
                .contains(r#""chat_id":9007199254740993"#)
        );
    }

    #[test]
    fn the_topic_goes_with_the_detected_chat() {
        let mut transport = Scripted::new(updates(WROTE));
        notifier("tgram://123456789:abcdefghijklmn?topic=42")
            .send(&Notification::new("Body"), &mut transport);
        assert_eq!(
            json_body(&transport.seen[1])["message_thread_id"],
            json!(42)
        );
    }

    #[test]
    fn a_bot_nobody_wrote_to_is_a_failure() {
        for answer in [
            r#"{"ok":true,"result":[]}"#,
            r#"{"ok":false,"description":"Unauthorized"}"#,
            r#"{"ok":true,"result":[{"message":{"from":{"first_name":"No id"}}}]}"#,
            r#"{"ok":true,"result":[{"message":{"from":{"id":0}}}]}"#,
            "not json",
        ] {
            let mut transport = Scripted::new(updates(answer));
            let report = notifier(DETECTING).send(&Notification::new("Body"), &mut transport);
            assert_eq!(transport.seen.len(), 1, "answer: {answer}");
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
    fn nothing_is_detected_when_a_chat_is_given_or_detection_is_off() {
        let mut transport = Scripted::new(updates(WROTE));
        notifier("tgram://123456789:abcdefghijklmn/55")
            .send(&Notification::new("Body"), &mut transport);
        assert_eq!(transport.calls().len(), 1);
        assert!(transport.calls()[0].ends_with("/sendMessage"));

        let off: Service = "tgram://123456789:abcdefghijklmn?detect=no"
            .parse()
            .expect("service");
        assert_eq!(
            off.plan(&Notification::new("Body")).finish(),
            [Outcome::NothingToSend]
        );
    }
}

#[cfg(feature = "slack")]
mod slack_files {
    use serde_json::json;
    use tocsin::{Attachment, Notification, Outcome, Response, TransportError};

    use super::*;

    const BOT: &str = "slack://xoxb-1234-1234-abc124/ops/dev";

    /// Slack's side of messages, lookups and uploads.
    #[allow(clippy::unnecessary_wraps, reason = "the shape a transport answers in")]
    fn slack(request: &PreparedRequest) -> Result<Response, TransportError> {
        let url = request.url.expose();
        let answer = if url.contains("users.lookupByEmail") {
            let user = url.rsplit("email=").next().unwrap_or("");
            let user = user.split("%40").next().unwrap_or("");
            json!({"ok": true, "user": {"id": format!("U-{user}")}})
        } else if url.contains("chat.postMessage") {
            let channel = json_body(request)["channel"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            // A user is answered with the conversation that was opened for them.
            let conversation = match channel.strip_prefix("U-") {
                Some(user) => format!("D-{user}"),
                None => format!("C{}", channel.trim_start_matches('#')),
            };
            json!({"ok": true, "channel": conversation})
        } else if url.contains("getUploadURLExternal") {
            json!({"ok": true, "file_id": "F1", "upload_url": "https://files.slack.com/upload/v1/ABC"})
        } else if url.starts_with("https://files.slack.com/") {
            return Ok(Response::with_body(200, "OK - 3"));
        } else if url.contains("completeUploadExternal") {
            json!({"ok": true, "files": [{"id": "F1"}]})
        } else {
            json!({"ok": true})
        };
        Ok(Response::with_body(200, answer.to_string()))
    }

    fn with_files() -> Notification {
        Notification::new("Body")
            .attach(Attachment::new("a.txt", b"one".to_vec()))
            .attach(Attachment::new("b.bin", b"two".to_vec()))
    }

    fn failures(report: &tocsin::Report) -> Vec<Outcome> {
        report.failures().map(|r| r.outcome.clone()).collect()
    }

    #[test]
    fn a_bot_shares_every_file_into_every_channel_the_message_went_to() {
        let mut transport = Scripted::new(slack);
        let report = notifier(BOT).send(&with_files(), &mut transport);
        assert!(report.is_success(), "{:?}", failures(&report));
        // The messages, and then each file's three steps.
        assert_eq!(
            transport.calls(),
            [
                "POST /api/chat.postMessage",
                "POST /api/chat.postMessage",
                "GET /api/files.getUploadURLExternal?filename=a.txt&length=3",
                "POST /upload/v1/ABC",
                "POST /api/files.completeUploadExternal",
                "POST /api/files.completeUploadExternal",
                "GET /api/files.getUploadURLExternal?filename=b.bin&length=3",
                "POST /upload/v1/ABC",
                "POST /api/files.completeUploadExternal",
                "POST /api/files.completeUploadExternal",
            ]
        );
        // Two messages and four shares are deliveries; the rest only sets up.
        assert_eq!(report.receipts().len(), 2 + 4);

        let address = &transport.seen[2];
        assert_eq!(
            address.headers["Authorization"].expose(),
            "Bearer xoxb-1234-1234-abc124"
        );
        let upload = &transport.seen[3];
        assert!(
            !upload.headers.contains_key("Authorization"),
            "the token stays home"
        );
        assert_eq!(
            upload.body.text(),
            "--tocsin-0\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\none\r\n--tocsin-0--\r\n"
        );
        assert_eq!(
            json_body(&transport.seen[4]),
            json!({"files": [{"id": "F1", "title": "a.txt"}], "channel_id": "Cdev"})
        );
        assert_eq!(json_body(&transport.seen[5])["channel_id"], json!("Cops"));
    }

    #[test]
    fn a_user_known_by_e_mail_gets_the_files_in_their_conversation() {
        let mut transport = Scripted::new(slack);
        let url = "slack://xoxb-1234-1234-abc124/bob@example.com";
        let notification =
            Notification::new("Body").attach(Attachment::new("a.txt", b"one".to_vec()));
        let report = notifier(url).send(&notification, &mut transport);
        assert!(report.is_success(), "{:?}", failures(&report));
        assert_eq!(
            transport.calls(),
            [
                "GET /api/users.lookupByEmail?email=bob%40example.com",
                "POST /api/chat.postMessage",
                "GET /api/files.getUploadURLExternal?filename=a.txt&length=3",
                "POST /upload/v1/ABC",
                "POST /api/files.completeUploadExternal",
            ]
        );
        assert_eq!(json_body(&transport.seen[4])["channel_id"], json!("D-bob"));
    }

    #[test]
    fn a_message_slack_does_not_place_leaves_the_files_nowhere_to_go() {
        let mut transport = Scripted::new(|request: &PreparedRequest| {
            if request.url.expose().contains("chat.postMessage") {
                Ok(Response::with_body(200, r#"{"ok":true}"#))
            } else {
                slack(request)
            }
        });
        let report =
            notifier("slack://xoxb-1234-1234-abc124/ops").send(&with_files(), &mut transport);
        assert_eq!(transport.calls(), ["POST /api/chat.postMessage"]);
        assert_eq!(
            failures(&report),
            [Outcome::Failed(TransportError::InvalidResponse)]
        );
    }

    #[test]
    fn a_file_that_cannot_be_uploaded_does_not_keep_the_others() {
        let mut transport = Scripted::new(|request: &PreparedRequest| {
            if request.url.expose().contains("filename=a.txt") {
                Ok(Response::with_body(
                    200,
                    r#"{"ok":false,"error":"missing_scope"}"#,
                ))
            } else {
                slack(request)
            }
        });
        let report =
            notifier("slack://xoxb-1234-1234-abc124/ops").send(&with_files(), &mut transport);
        // The message, the failed address, then the second file in full.
        assert_eq!(transport.calls().len(), 1 + 1 + 3);
        assert_eq!(
            failures(&report),
            [Outcome::Failed(TransportError::Rejected)]
        );
    }

    #[test]
    fn an_upload_slack_did_not_accept_is_not_shared() {
        let mut transport = Scripted::new(|request: &PreparedRequest| {
            if request.url.expose().starts_with("https://files.slack.com/") {
                Ok(Response::with_body(200, "denied"))
            } else {
                slack(request)
            }
        });
        let notification =
            Notification::new("Body").attach(Attachment::new("a.txt", b"one".to_vec()));
        let report =
            notifier("slack://xoxb-1234-1234-abc124/ops").send(&notification, &mut transport);
        assert_eq!(transport.calls().len(), 3, "no share after the upload");
        assert_eq!(
            failures(&report),
            [Outcome::Failed(TransportError::Rejected)]
        );
    }

    #[test]
    fn a_webhook_ignores_the_files() {
        let mut transport = Scripted::new(|_: &PreparedRequest| Ok(Response::with_body(200, "ok")));
        let report =
            notifier("slack://TFAKE1/BFAKE2/CFAKE3/ops").send(&with_files(), &mut transport);
        assert!(report.is_success());
        assert_eq!(transport.seen.len(), 1);
    }

    #[test]
    fn the_files_go_with_the_first_part_of_a_long_message_only() {
        let mut transport = Scripted::new(slack);
        let url = "slack://xoxb-1234-1234-abc124/ops?overflow=split";
        let notification = Notification::new("word ".repeat(10_000))
            .attach(Attachment::new("a.txt", b"one".to_vec()));
        let report = notifier(url).send(&notification, &mut transport);
        assert!(report.is_success(), "{:?}", failures(&report));
        let calls = transport.calls();
        let uploads = calls
            .iter()
            .filter(|call| call.contains("getUploadURLExternal"))
            .count();
        assert_eq!(uploads, 1);
        let messages = calls
            .iter()
            .filter(|call| call.contains("chat.postMessage"))
            .count();
        assert!(messages > 1);
    }
}
