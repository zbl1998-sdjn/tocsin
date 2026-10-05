//! Golden tests for the requests each service builds. They check the exact
//! wire format against the providers' own documentation, which Apprise's
//! behaviour is not always faithful to (see `DESIGN.md`).

use tocsin::{MockTransport, PreparedRequest, SecretString, Transport};

#[cfg(feature = "_services")]
#[allow(
    unused_imports,
    reason = "the form and Pushover tests read bodies as text"
)]
use serde_json::{Value, json};
#[cfg(any(feature = "json", feature = "slack"))]
use tocsin::Kind;
#[cfg(any(feature = "json", feature = "form"))]
use tocsin::Method;
#[cfg(feature = "_services")]
use tocsin::{Notification, Service};

#[cfg(feature = "_services")]
fn notification(body: &str) -> Notification {
    Notification::new(body).title("Title")
}

#[cfg(feature = "_services")]
#[allow(dead_code, reason = "the form and Pushover tests read bodies as text")]
fn body(request: &PreparedRequest) -> Value {
    serde_json::from_str(&request.body.text()).expect("JSON body")
}

#[test]
fn mock_transport_and_secret_formatting() {
    let secret = "FAKE_SENSITIVE_Query_AND_password";
    let mut request = PreparedRequest::json(
        format!("http://127.0.0.1:1/?token={secret}"),
        &serde_json::json!({"password": secret}),
    );
    request
        .headers
        .insert("Authorization".into(), SecretString::new(secret));
    let mut mock = MockTransport::default();
    mock.send(&request).expect("mock");
    let output = format!(
        "{request:?} {request} {:?} {} {mock:?}",
        request.url, request.body
    );
    assert!(!output.contains(secret));
    assert_eq!(mock.requests.len(), 1);
    for error in [
        tocsin::ParseError::InvalidUrl,
        tocsin::ParseError::InvalidToken,
        tocsin::ParseError::InvalidOption,
    ] {
        assert!(!format!("{error:?} {error}").contains(secret));
    }
}

#[cfg(feature = "telegram")]
#[test]
fn telegram_golden_and_unicode_overflow() {
    let service: Service =
        "tgram://123456789:FAKE_token/100:9/-200?format=text&silent=yes&preview=yes"
            .parse()
            .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 2);
    assert_eq!(
        body(&requests[0]),
        json!({
            "chat_id": -200,
            "text": "Title\r\nBody",
            "disable_notification": true,
            "link_preview_options": {"is_disabled": false},
        })
    );
    assert_eq!(body(&requests[1])["message_thread_id"], json!(9));
    assert!(
        requests[0]
            .url
            .expose()
            .ends_with("/bot123456789:FAKE_token/sendMessage")
    );

    let mut long = notification(&"🦀".repeat(4100));
    long.title.clear();
    for (overflow, count) in [("truncate", 1), ("split", 2), ("upstream", 1)] {
        let service: Service = format!("tgram://123:FAKE/1?format=text&overflow={overflow}")
            .parse()
            .expect("parse");
        let requests = service.prepare(&long);
        assert_eq!(requests.len(), count, "{overflow}");
        let len = body(&requests[0])["text"]
            .as_str()
            .expect("text")
            .chars()
            .count();
        assert_eq!(len, if overflow == "upstream" { 4100 } else { 4096 });
    }
    let shown = format!("{service:?} {service}");
    assert!(!shown.contains("FAKE_token"));
    // A valid URL with no chat id has nothing to send to.
    let empty: Service = "tgram://123:FAKE_token".parse().expect("parse");
    assert!(empty.prepare(&long).is_empty());
}

#[cfg(feature = "telegram")]
#[test]
fn telegram_markdown_modes() {
    let notification = Notification::new("*bold*").format(tocsin::Format::Markdown);
    for (url, mode) in [
        ("tgram://123:FAKE/1", "Markdown"),
        ("tgram://123:FAKE/1?mdv=v2", "MarkdownV2"),
    ] {
        let service: Service = url.parse().expect("parse");
        let requests = service.prepare(&notification);
        assert_eq!(body(&requests[0])["parse_mode"], json!(mode), "{url}");
    }
}

#[cfg(feature = "discord")]
#[test]
fn discord_wait_is_a_query_parameter() {
    let service: Service = "discord://FakeBot@FAKE_id/FAKE_token?format=text&tts=yes"
        .parse()
        .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert!(requests[0].url.expose().ends_with("?wait=false"));
    assert_eq!(
        body(&requests[0]),
        json!({"content": "Title\r\nBody", "tts": true, "username": "FakeBot"})
    );

    let service: Service = "discord://FAKE_id/FAKE_token?thread=123&href=http://127.0.0.1/item"
        .parse()
        .expect("parse");
    let requests = service.prepare(&notification("Body"));
    let request = &requests[0];
    assert!(request.url.expose().ends_with("?wait=true&thread_id=123"));
    assert_eq!(body(request)["embeds"][0]["description"], json!("Body"));
    // Neither `wait` nor Apprise's internal `allow_mentions` belong in the body.
    assert!(body(request).get("wait").is_none());
    assert!(body(request).get("allow_mentions").is_none());
}

#[cfg(feature = "discord")]
#[test]
fn discord_split_respects_the_content_limit() {
    let mut long = notification(&"🦀".repeat(2002));
    long.title.clear();
    let service: Service = "discord://FAKE_id/FAKE_token?overflow=split"
        .parse()
        .expect("parse");
    let requests = service.prepare(&long);
    assert_eq!(requests.len(), 2);
    let content = body(&requests[0])["content"]
        .as_str()
        .expect("text")
        .to_owned();
    assert_eq!(content.chars().count(), 2000);
}

#[cfg(feature = "ntfy")]
#[test]
fn ntfy_private_server_and_auth_golden() {
    let service: Service =
        "ntfys://fake_user:FAKE_password@127.0.0.1:8080/topic1/topic2?priority=high&image=no"
            .parse()
            .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.expose(), "https://127.0.0.1:8080");
    assert_eq!(
        body(&requests[0]),
        json!({"topic": "topic2", "title": "Title", "message": "Body"})
    );
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Basic ZmFrZV91c2VyOkZBS0VfcGFzc3dvcmQ="
    );
    assert_eq!(requests[0].headers["X-Priority"].expose(), "high");

    let service: Service = "ntfy://127.0.0.1/topic?token=FAKE_query_token&format=markdown"
        .parse()
        .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Bearer FAKE_query_token"
    );
    assert_eq!(requests[0].headers["X-Markdown"].expose(), "yes");
    assert!(!format!("{service:?} {requests:?}").contains("FAKE_query_token"));

    let tokenless: Service = "ntfy://127.0.0.1/topic?auth=token".parse().expect("parse");
    assert!(tokenless.prepare(&notification("Body")).is_empty());
}

#[cfg(feature = "ntfy")]
#[test]
fn ntfy_cloud_goes_to_ntfy_sh_without_credentials() {
    let service: Service = "ntfy://my-topic".parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.expose(), "https://ntfy.sh");
    assert!(!requests[0].headers.contains_key("Authorization"));
}

#[cfg(feature = "ntfy")]
#[test]
fn ntfy_unicode_title_and_body_truncate_by_characters() {
    let long = Notification::new("🦀".repeat(7801)).title("中".repeat(201));
    let service: Service = "ntfy://127.0.0.1/topic?overflow=truncate"
        .parse()
        .expect("parse");
    let requests = service.prepare(&long);
    let sent = body(&requests[0]);
    assert_eq!(sent["title"].as_str().expect("title").chars().count(), 200);
    assert_eq!(
        sent["message"].as_str().expect("body").chars().count(),
        7600
    );
}

#[cfg(feature = "gotify")]
#[test]
fn gotify_base_path_priority_and_markdown() {
    let service: Service =
        "gotifys://example.com:8443/gotify/FAKE_token?priority=high&format=markdown"
            .parse()
            .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.expose(),
        "https://example.com:8443/gotify/message"
    );
    assert_eq!(requests[0].headers["X-Gotify-Key"].expose(), "FAKE_token");
    assert_eq!(
        body(&requests[0]),
        json!({
            "priority": 8,
            "title": "Title",
            "message": "Body",
            "extras": {"client::display": {"contentType": "text/markdown"}},
        })
    );

    // Without a title, a plain server and the default priority.
    let service: Service = "gotify://localhost/FAKE_token".parse().expect("parse");
    let requests = service.prepare(&Notification::new("Body"));
    assert_eq!(requests[0].url.expose(), "http://localhost/message");
    assert_eq!(
        body(&requests[0]),
        json!({"priority": 5, "message": "Body"})
    );

    // A URL without a token is not a Gotify URL.
    assert!("gotify://localhost".parse::<Service>().is_err());
}

#[cfg(feature = "json")]
#[test]
fn json_webhook_default_layout_and_basic_auth() {
    let service: Service = "jsons://FAKE_user:FAKE_password@example.com:8443/hooks/in"
        .parse()
        .expect("parse");
    let requests = service.prepare(&Notification::new("Body").title("Title").kind(Kind::Warning));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::Post);
    assert_eq!(
        requests[0].url.expose(),
        "https://example.com:8443/hooks/in"
    );
    assert_eq!(
        body(&requests[0]),
        json!({
            "version": "1.0",
            "title": "Title",
            "message": "Body",
            "attachments": [],
            "type": "warning",
        })
    );
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Basic RkFLRV91c2VyOkZBS0VfcGFzc3dvcmQ="
    );
    assert_eq!(
        requests[0].headers["Content-Type"].expose(),
        "application/json; charset=utf-8"
    );

    // A user without a password sends an empty password, not the text "None".
    let service: Service = "json://FAKE_user@localhost".parse().expect("parse");
    let requests = service.prepare(&Notification::new("Body"));
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Basic RkFLRV91c2VyOg=="
    );
}

#[cfg(feature = "json")]
#[test]
fn json_webhook_headers_parameters_and_payload_changes() {
    // Headers replace defaults whatever their case; parameters join the URL;
    // `:title=` drops a field and `:message=text` renames one.
    let url = [
        "json://localhost:8080/api/in?method=put",
        "+content-type=application/vnd.fake%2Bjson",
        "+X-Token=FAKE_header",
        "-b=two words",
        "-a=%C3%A9",
        ":title=",
        ":message=text",
        ":extra=1",
        ":type=kind",
    ]
    .join("&");
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::Put);
    assert_eq!(
        requests[0].url.expose(),
        "http://localhost:8080/api/in?b=two+words&a=%C3%A9"
    );
    let headers = &requests[0].headers;
    assert_eq!(headers.len(), 2, "{headers:?}");
    assert_eq!(
        headers["content-type"].expose(),
        "application/vnd.fake+json"
    );
    assert_eq!(headers["X-Token"].expose(), "FAKE_header");
    assert_eq!(
        body(&requests[0]),
        json!({
            "version": "1.0",
            "text": "Body",
            "attachments": [],
            "kind": "info",
            "extra": "1",
        })
    );
    assert!(!format!("{service:?} {requests:?}").contains("FAKE_header"));
}

#[cfg(feature = "workflows")]
#[test]
fn teams_workflow_adaptive_card_and_url() {
    let service: Service = "workflows://prod-1.example.logic.azure.com:443/FAKE_flow/FAKE_sig"
        .parse()
        .expect("parse");
    let requests =
        service.prepare(&Notification::new("Hi <at>Ada</at> and <at>Ada</at>").title("Title"));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.expose(),
        concat!(
            "https://prod-1.example.logic.azure.com:443/workflows/FAKE_flow",
            "/triggers/manual/paths/invoke?api-version=2016-06-01",
            "&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=FAKE_sig"
        )
    );
    // A mention appears once in the entities even when it is written twice.
    assert_eq!(
        body(&requests[0]),
        json!({
            "type": "message",
            "attachments": [{
                "contentType": "application/vnd.microsoft.card.adaptive",
                "contentUrl": null,
                "content": {
                    "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                    "type": "AdaptiveCard",
                    "version": "1.4",
                    "body": [
                        {"type": "TextBlock", "text": "Title", "style": "heading",
                         "weight": "Bolder", "size": "Large", "id": "title"},
                        {"type": "TextBlock", "text": "Hi <at>Ada</at> and <at>Ada</at>",
                         "style": "default", "wrap": true, "id": "body"},
                    ],
                    "msteams": {
                        "width": "full",
                        "entities": [{
                            "type": "mention",
                            "text": "<at>Ada</at>",
                            "mentioned": {"id": "Ada", "name": "Ada"},
                        }],
                    },
                },
            }],
        })
    );

    // Power Automate URLs add a path and a routing id.
    let service: Service = "workflows://host/FAKE_flow/FAKE_sig?pa=yes&route=42&ver=x y"
        .parse()
        .expect("parse");
    let requests = service.prepare(&Notification::new("Body"));
    assert_eq!(
        requests[0].url.expose(),
        concat!(
            "https://host/powerautomate/automations/direct/cu/42/workflows/FAKE_flow",
            "/triggers/manual/paths/invoke?api-version=x+y",
            "&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=FAKE_sig"
        )
    );
    assert_eq!(
        body(&requests[0])["attachments"][0]["content"]["msteams"],
        json!({"width": "full"})
    );

    // A template file is not read while parsing, so the URL is refused.
    assert!(
        "workflows://host/FAKE_flow/FAKE_sig?template=/tmp/card.json"
            .parse::<Service>()
            .is_err()
    );
}

#[cfg(feature = "form")]
#[test]
fn form_webhook_body_renames_and_get_query() {
    let url = [
        "form://localhost:8080/api?:title=headline&:version=&:extra=a b",
        "-token=1",
        "+X-Key=FAKE_header",
    ]
    .join("&");
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::Post);
    assert_eq!(
        requests[0].url.expose(),
        "http://localhost:8080/api?token=1"
    );
    assert_eq!(
        requests[0].headers["Content-Type"].expose(),
        "application/x-www-form-urlencoded"
    );
    assert_eq!(requests[0].headers["X-Key"].expose(), "FAKE_header");
    assert_eq!(
        requests[0].body.text(),
        "headline=Title&message=Body&type=info&extra=a+b"
    );

    // With GET everything goes in the query string, and there is no body.
    let service: Service = "form://localhost/api?method=get&-b=2"
        .parse()
        .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests[0].method, Method::Get);
    assert_eq!(
        requests[0].url.expose(),
        "http://localhost/api?version=1.0&title=Title&message=Body&type=info&b=2"
    );
    assert_eq!(requests[0].body.text(), "");
    assert!(!requests[0].headers.contains_key("Content-Type"));
    assert!(!format!("{service:?} {requests:?}").contains("FAKE_header"));
}

#[cfg(feature = "mattermost")]
#[test]
fn mattermost_webhook_and_bot_requests() {
    let url = "mmosts://botty@chat.example.com:8443/base/FAKE_hook?to=ops,%23dev&icon_url=https://x/i.png";
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            request.url.expose(),
            "https://chat.example.com:8443/base/hooks/FAKE_hook"
        );
    }
    // The title goes in front of the body, and channels come out sorted.
    assert_eq!(
        body(&requests[0]),
        json!({
            "text": "Title\r\nBody",
            "username": "botty",
            "icon_url": "https://x/i.png",
            "channel": "dev",
        })
    );
    assert_eq!(body(&requests[1])["channel"], json!("ops"));

    // Without a channel the webhook's own default is used.
    let service: Service = "mmost://localhost/FAKE_hook".parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.expose(), "http://localhost/hooks/FAKE_hook");
    assert_eq!(
        body(&requests[0]),
        json!({"text": "Title\r\nBody", "username": "tocsin"})
    );

    // Bot mode posts to channel ids. A channel name needs a lookup request
    // first, which `prepare` cannot make, so it is skipped.
    let url = "mmost://localhost/FAKE_bot?mode=bot&team=myteam&to=%23general,%2Bid1";
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.expose(), "http://localhost/api/v4/posts");
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Bearer FAKE_bot"
    );
    assert_eq!(
        body(&requests[0]),
        json!({"channel_id": "id1", "message": "Title\r\nBody"})
    );
    let service: Service = "mmost://localhost/FAKE_bot?mode=bot"
        .parse()
        .expect("parse");
    assert!(service.prepare(&notification("Body")).is_empty());
}

#[cfg(feature = "rocketchat")]
#[test]
fn rocketchat_webhook_token_and_basic_requests() {
    // A webhook: users, then channels, then rooms, one request each.
    let url = "rockets://web/token@chat.example.com:3000/@ann/%23ops/room1";
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(
            request.url.expose(),
            "https://chat.example.com:3000/hooks/web/token"
        );
        assert_eq!(body(request)["text"], json!("# Title\nBody"));
    }
    let channels: Vec<_> = requests
        .iter()
        .map(|r| body(r)["channel"].clone())
        .collect();
    assert_eq!(channels, [json!("@ann"), json!("#ops"), json!("room1")]);

    // A personal access token goes in headers, and a room id has its own key.
    let url = format!(
        "rocket://user:{}@localhost/@ann/%23ops/room1",
        "t".repeat(40)
    );
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(
            request.url.expose(),
            "http://localhost/api/v1/chat.postMessage"
        );
        assert_eq!(request.headers["X-User-Id"].expose(), "user");
        assert_eq!(request.headers["X-Auth-Token"].expose(), "t".repeat(40));
    }
    assert_eq!(
        body(&requests[2]),
        json!({"text": "# Title\nBody", "roomId": "room1"})
    );

    // A user name and password need a login request first, so `prepare` has
    // nothing to send; `plan` does the login (see `tests/plans.rs`).
    let service: Service = "rocket://user:pass@localhost/room1".parse().expect("parse");
    assert!(service.prepare(&notification("Body")).is_empty());
}

#[cfg(feature = "pushover")]
#[test]
fn pushover_devices_groups_priority_and_encryption() {
    let (user, token) = ("u".repeat(30), "a".repeat(30));
    // Devices share one request, and every group gets its own.
    let url = format!(
        "pover://{user}@{token}/phone/tablet/%23ops?priority=emergency&sound=siren\
         &url=https://x/y&url_title=Open it"
    );
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].url.expose(),
        "https://api.pushover.net/1/messages.json"
    );
    assert_eq!(
        requests[0].headers["Content-Type"].expose(),
        "application/x-www-form-urlencoded"
    );
    assert_eq!(
        requests[0].body.text(),
        format!(
            "token={token}&priority=2&title=Title&message=Body&sound=siren\
             &url=https%3A%2F%2Fx%2Fy&url_title=Open+it&retry=900&expire=3600\
             &user={user}&device=phone%2Ctablet"
        )
    );
    assert!(requests[1].body.text().ends_with("&user=ops"));

    // With no target the device is left out, which is how Pushover says "all".
    let service: Service = format!("pover://{user}@{token}?format=html")
        .parse()
        .expect("parse");
    let requests = service.prepare(&Notification::new("Body"));
    assert_eq!(
        requests[0].body.text(),
        format!("token={token}&priority=0&message=Body&sound=pushover&html=1&user={user}")
    );

    // End-to-end encryption is not implemented, and the text must not go out
    // unencrypted when the URL asked for it.
    let key = "b".repeat(64);
    let service: Service = format!("pover://{user}@{token}?key={key}")
        .parse()
        .expect("parse");
    assert!(service.prepare(&notification("Body")).is_empty());
    let service: Service = format!("pover://{user}@{token}?key={key}&e2ee=no")
        .parse()
        .expect("parse");
    assert_eq!(service.prepare(&notification("Body")).len(), 1);

    // Nothing valid to send to.
    let service: Service = format!("pover://{user}@{token}/%23bad-group")
        .parse()
        .expect("parse");
    assert!(service.prepare(&notification("Body")).is_empty());
}

#[cfg(feature = "slack")]
#[test]
fn slack_webhook_bot_blocks_and_workflow_requests() {
    // A webhook: channels come out sorted, a thread follows a colon, and an
    // e-mail address would need a lookup request, so `prepare` skips it.
    let url = "slack://TFAKE1/BFAKE2/CFAKE3/#ops/@ann/+C123:1700.5/dev@example.com";
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&Notification::new("a <b> & c").title("T & T"));
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(
            request.url.expose(),
            "https://hooks.slack.com/services/TFAKE1/BFAKE2/CFAKE3"
        );
    }
    assert_eq!(
        body(&requests[0]),
        json!({
            "mrkdwn": false,
            "attachments": [{
                "title": "T &amp; T",
                "text": "a &lt;b&gt; &amp; c",
                "color": "#3AA3E3",
                "footer": "tocsin",
            }],
            "channel": "#ops",
        })
    );
    assert_eq!(body(&requests[1])["channel"], json!("C123"));
    assert_eq!(body(&requests[1])["thread_ts"], json!("1700.5"));
    assert_eq!(body(&requests[2])["channel"], json!("@ann"));

    // Without a channel the webhook's own default is used.
    let service: Service = "slack://botty@TFAKE1/BFAKE2/CFAKE3?footer=no"
        .parse()
        .expect("parse");
    let requests = service.prepare(&Notification::new("Body").kind(Kind::Failure));
    assert_eq!(
        body(&requests[0]),
        json!({
            "mrkdwn": false,
            "attachments": [{"title": "", "text": "Body", "color": "#A32037"}],
            "username": "botty",
        })
    );

    // A bot token goes in a header, and the default channel is #general.
    let service: Service = "slack://xoxb-1234-1234-abc124".parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(
        requests[0].url.expose(),
        "https://slack.com/api/chat.postMessage"
    );
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Bearer xoxb-1234-1234-abc124"
    );
    assert_eq!(body(&requests[0])["channel"], json!("#general"));

    // Blocks, and a Markdown body that is sent as written.
    let service: Service = "slack://TFAKE1/BFAKE2/CFAKE3?blocks=yes"
        .parse()
        .expect("parse");
    let markdown = Notification::new("*Body*")
        .title("Title")
        .format(tocsin::Format::Markdown);
    let requests = service.prepare(&markdown);
    assert_eq!(
        body(&requests[0]),
        json!({"attachments": [{
            "blocks": [
                {"type": "header", "text": {"type": "plain_text", "text": "Title", "emoji": true}},
                {"type": "section", "text": {"type": "mrkdwn", "text": "*Body*"}},
                {"type": "context", "elements": [{"type": "mrkdwn", "text": "tocsin"}]},
            ],
            "color": "#3AA3E3",
        }]})
    );

    // A workflow takes only text.
    let url = "slack://TFAKE1/Ft07XXXX/XXXXXXXX/YYYYYYYY/?mode=workflow";
    let service: Service = url.parse().expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(
        requests[0].url.expose(),
        "https://hooks.slack.com/workflows/TFAKE1/Ft07XXXX/XXXXXXXX/YYYYYYYY"
    );
    assert_eq!(body(&requests[0]), json!({"text": "Title: Body"}));

    // The address Slack shows for a webhook works as it is.
    assert!(
        "https://hooks.slack.com/services/TFAKE1/BFAKE2/CFAKE3"
            .parse::<Service>()
            .is_ok()
    );
}

#[cfg(feature = "prowl")]
#[test]
fn prowl_posts_a_form_to_the_public_api() {
    let (key, provider) = ("a".repeat(40), "b".repeat(40));
    let service: Service = format!("prowl://{key}/{provider}?priority=high")
        .parse()
        .expect("parse");
    let requests = service.prepare(&notification("Body"));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.expose(),
        "https://api.prowlapp.com/publicapi/add"
    );
    assert_eq!(
        requests[0].headers["Content-Type"].expose(),
        "application/x-www-form-urlencoded"
    );
    assert_eq!(
        requests[0].body.text(),
        format!(
            "apikey={key}&application=tocsin&event=Title&description=Body&priority=1&providerkey={provider}"
        )
    );
    // Without a provider key, and with the default priority.
    let service: Service = format!("prowl://{key}").parse().expect("parse");
    let body = service.prepare(&notification("Body"))[0]
        .body
        .text()
        .into_owned();
    assert!(body.ends_with("&priority=0"), "{body}");
}

#[cfg(feature = "_services")]
#[test]
fn credentials_in_urls_never_show_up_in_formatting() {
    for url in [
        "ntfy://FAKE_user:FAKE_password@127.0.0.1/topic?token=FAKE_query_token",
        "tgram://FakeUser:FAKE_password@123:FAKE_token/1?token=FAKE_query_token",
        "discord://FAKE_user:FAKE_password@FAKE_id/FAKE_token?token=FAKE_query_token",
        "gotify://FAKE_user:FAKE_password@127.0.0.1/FAKE_token?priority=high",
        "json://FAKE_user:FAKE_password@127.0.0.1/FAKE_token?+X-Key=FAKE_query_token",
        "workflows://FAKE_user:FAKE_password@127.0.0.1/FAKE_token/FAKE_sig?:key=FAKE_query_token",
        "form://FAKE_user:FAKE_password@127.0.0.1/FAKE_token?+X-Key=FAKE_query_token",
        "mmost://FAKE_user@127.0.0.1/FAKE_token?to=ops&icon_url=http://127.0.0.1/FAKE_query_token",
        "rocket://FAKE_user:FAKE_password@127.0.0.1/%23ops?mode=token&to=FAKE_query_token",
        "pover://FAKE_user@FAKE_token/FAKE_password?url=http://127.0.0.1/FAKE_query_token",
        "slack://FAKE_user@TFAKE/BFAKE/CFAKE/FAKE_password?:key=FAKE_query_token",
        "prowl://FAKE_user:FAKE_password@FAKEKEYFAKEKEYFAKEKEYFAKEKEYFAKEKEYFAKE0/FAKEKEYFAKEKEYFAKEKEYFAKEKEYFAKEKEYFAKE1",
    ] {
        if let Ok(service) = url.parse::<Service>() {
            let requests = service.prepare(&notification("body"));
            let output = format!("{service:?} {service} {:?} {requests:?}", service.options());
            for secret in [
                "FAKE_user",
                "FAKE_password",
                "FAKE_query_token",
                "FAKE_token",
                "FAKE_id",
                "FAKEKEYFAKEKEY",
            ] {
                assert!(!output.contains(secret), "{url}: leaked {secret}");
            }
        }
    }
}
