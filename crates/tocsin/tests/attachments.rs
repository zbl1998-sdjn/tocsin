//! What each service does with attachments.
#![cfg(any(
    feature = "json",
    feature = "form",
    feature = "ntfy",
    feature = "pushover",
    feature = "telegram",
    feature = "discord",
    feature = "xml"
))]

#[cfg(any(feature = "json", feature = "ntfy"))]
use serde_json::{Value, json};
use tocsin::{Attachment, Notification, Service};

fn service(url: &str) -> Service {
    url.parse().expect("service")
}

#[cfg(any(
    feature = "json",
    feature = "form",
    feature = "ntfy",
    feature = "pushover",
    feature = "xml"
))]
fn text(content: &str) -> Attachment {
    Attachment::new("a.txt", content.as_bytes().to_vec())
}

#[cfg(feature = "json")]
#[test]
fn json_webhook_carries_attachments_as_base64() {
    let notification = Notification::new("Body")
        .attach(text("hello"))
        .attach(Attachment::new("", b"x".to_vec()));
    let requests = service("json://localhost/hook").prepare(&notification);
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_str(&requests[0].body.text()).expect("JSON body");
    assert_eq!(
        body["attachments"],
        json!([
            {"filename": "a.txt", "base64": "aGVsbG8=", "mimetype": "text/plain"},
            {"filename": "file002.dat", "base64": "eA==", "mimetype": "application/octet-stream"},
        ])
    );
}

#[cfg(feature = "json")]
#[test]
fn only_the_first_part_of_a_long_message_carries_the_files() {
    let notification = Notification::new("word ".repeat(8000))
        .attach(text("one"))
        .attach(text("two"));
    let requests = service("json://localhost/hook?overflow=split").prepare(&notification);
    assert_eq!(requests.len(), 2);
    let files = |at: usize| -> usize {
        let body: Value = serde_json::from_str(&requests[at].body.text()).expect("JSON body");
        body["attachments"].as_array().expect("array").len()
    };
    assert_eq!((files(0), files(1)), (2, 0));
}

#[cfg(feature = "json")]
#[test]
fn truncating_keeps_only_the_first_file() {
    let notification = Notification::new("Body")
        .attach(text("one"))
        .attach(text("two"));
    let requests = service("json://localhost/hook?overflow=truncate").prepare(&notification);
    let body: Value = serde_json::from_str(&requests[0].body.text()).expect("JSON body");
    assert_eq!(body["attachments"].as_array().expect("array").len(), 1);
}

/// Text with the line ends a multipart body has.
#[cfg(feature = "form")]
fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

#[cfg(feature = "form")]
#[test]
fn form_webhook_sends_multipart_with_attachments() {
    let notification = Notification::new("Body").attach(text("hello"));
    let requests = service("form://localhost/hook").prepare(&notification);
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.url.expose(), "http://localhost/hook");
    assert_eq!(
        request.headers["Content-Type"].expose(),
        "multipart/form-data; boundary=tocsin-0"
    );
    let expected = crlf(
        "--tocsin-0
Content-Disposition: form-data; name=\"version\"

1.0
--tocsin-0
Content-Disposition: form-data; name=\"title\"


--tocsin-0
Content-Disposition: form-data; name=\"message\"

Body
--tocsin-0
Content-Disposition: form-data; name=\"type\"

info
--tocsin-0
Content-Disposition: form-data; name=\"file01\"; filename=\"a.txt\"
Content-Type: text/plain

hello
--tocsin-0--
",
    );
    assert_eq!(request.body.text(), expected);
}

#[cfg(feature = "form")]
#[test]
fn form_attach_as_names_the_file_fields() {
    let two = Notification::new("Body")
        .attach(text("one"))
        .attach(Attachment::new("", b"two".to_vec()));
    let fields = |url: &str| -> Vec<String> {
        let body = service(url).prepare(&two)[0].body.text().into_owned();
        body.lines()
            .filter(|line| line.contains("filename="))
            .map(str::to_owned)
            .collect()
    };
    // A counter gives every file a field of its own.
    assert_eq!(
        fields("form://localhost/hook?attach-as=doc*"),
        [
            "Content-Disposition: form-data; name=\"doc01\"; filename=\"a.txt\"",
            "Content-Disposition: form-data; name=\"doc02\"; filename=\"file002.dat\"",
        ]
    );
    assert_eq!(
        fields("form://localhost/hook?attach-as=*part"),
        [
            "Content-Disposition: form-data; name=\"01part\"; filename=\"a.txt\"",
            "Content-Disposition: form-data; name=\"02part\"; filename=\"file002.dat\"",
        ]
    );
    // A fixed name is used for every file, as Apprise does (with a warning).
    assert_eq!(
        fields("form://localhost/hook?attach-as=upload"),
        [
            "Content-Disposition: form-data; name=\"upload\"; filename=\"a.txt\"",
            "Content-Disposition: form-data; name=\"upload\"; filename=\"file002.dat\"",
        ]
    );
}

#[cfg(feature = "form")]
#[test]
fn a_form_get_sends_the_fields_in_the_query_and_only_the_files_in_the_body() {
    let notification = Notification::new("Body").attach(text("hello"));
    let request = &service("form://localhost/hook?method=get").prepare(&notification)[0];
    assert_eq!(
        request.url.expose(),
        "http://localhost/hook?version=1.0&title=&message=Body&type=info"
    );
    assert_eq!(
        request.body.text(),
        crlf(
            "--tocsin-0
Content-Disposition: form-data; name=\"file01\"; filename=\"a.txt\"
Content-Type: text/plain

hello
--tocsin-0--
"
        )
    );
}

#[cfg(feature = "form")]
#[test]
fn without_attachments_the_form_stays_urlencoded() {
    let request = &service("form://localhost/hook").prepare(&Notification::new("Body"))[0];
    assert_eq!(
        request.headers["Content-Type"].expose(),
        "application/x-www-form-urlencoded"
    );
}

#[cfg(feature = "ntfy")]
#[test]
fn ntfy_publishes_every_file_as_a_body() {
    let notification = Notification::new("Body")
        .title("T")
        .attach(text("hello"))
        .attach(Attachment::new("b.bin", vec![1, 2, 3]));
    let url = "ntfys://user:pw@ntfy.example.com/alerts?priority=high&tags=a,b";
    let requests = service(url).prepare(&notification);
    assert_eq!(requests.len(), 2);
    // The title and the text go with the first file only.
    assert_eq!(
        requests[0].url.expose(),
        "https://ntfy.example.com/alerts?filename=a.txt&title=T&message=Body"
    );
    assert_eq!(requests[0].body.expose(), b"hello");
    assert_eq!(
        requests[1].url.expose(),
        "https://ntfy.example.com/alerts?filename=b.bin"
    );
    assert_eq!(requests[1].body.expose(), [1, 2, 3]);
    for request in &requests {
        assert!(!request.headers.contains_key("Content-Type"));
        assert!(
            request.headers["Authorization"]
                .expose()
                .starts_with("Basic ")
        );
        assert_eq!(request.headers["X-Priority"].expose(), "high");
        assert_eq!(request.headers["X-Tags"].expose(), "a,b");
    }
}

#[cfg(feature = "ntfy")]
#[test]
fn ntfy_cloud_publishes_files_to_the_topic_path() {
    let notification = Notification::new("Body").attach(Attachment::new("", vec![9]));
    let requests = service("ntfy://alerts").prepare(&notification);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.expose(),
        "https://ntfy.sh/alerts?filename=file001.dat&message=Body"
    );
}

#[cfg(feature = "ntfy")]
#[test]
fn ntfy_without_files_publishes_json_with_the_attach_url_and_tags() {
    let url = "ntfy://alerts?tags=a,b&attach=https://example.com/y.png&filename=y.png";
    let requests = service(url).prepare(&Notification::new("Body"));
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_str(&requests[0].body.text()).expect("JSON body");
    assert_eq!(
        body,
        json!({
            "topic": "alerts",
            "message": "Body",
            "attach": "https://example.com/y.png",
            "filename": "y.png",
        })
    );
    assert_eq!(requests[0].headers["X-Tags"].expose(), "a,b");
}

#[cfg(feature = "pushover")]
mod pushover {
    use tocsin::{Outcome, TransportError};

    use super::*;

    fn pushover() -> Service {
        service(&format!("pover://{}@{}", "u".repeat(30), "a".repeat(30)))
    }

    fn image(name: &str, content: &str) -> Attachment {
        Attachment::new(name, content.as_bytes().to_vec())
    }

    #[test]
    fn every_image_is_a_message_of_its_own() {
        let notification = Notification::new("Body")
            .title("T")
            .attach(image("one.png", "PNGDATA1"))
            .attach(image("two.jpg", "JPGDATA2"));
        let requests = pushover().prepare(&notification);
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(
                request.url.expose(),
                "https://api.pushover.net/1/messages.json"
            );
            assert!(
                request.headers["Content-Type"]
                    .expose()
                    .starts_with("multipart/form-data; boundary=")
            );
        }
        let first = requests[0].body.text();
        // The first message has the text, the title and the sound.
        assert!(first.contains("name=\"title\"\r\n\r\nT\r\n"), "{first}");
        assert!(first.contains("name=\"message\"\r\n\r\nBody\r\n"));
        assert!(first.contains("name=\"sound\"\r\n\r\npushover\r\n"));
        // The file has no media type of its own, as in Apprise.
        assert!(
            first.contains("name=\"attachment\"; filename=\"one.png\"\r\n\r\nPNGDATA1\r\n"),
            "{first}"
        );
        // The next one is named after its file, without a title or a sound.
        let second = requests[1].body.text();
        assert!(!second.contains("name=\"title\""));
        assert!(second.contains("name=\"message\"\r\n\r\ntwo.jpg\r\n"));
        assert!(second.contains("name=\"sound\"\r\n\r\nnone\r\n"));
        assert!(second.contains("filename=\"two.jpg\"\r\n\r\nJPGDATA2\r\n"));
    }

    #[test]
    fn an_attachment_that_is_no_image_only_sends_the_message() {
        let notification = Notification::new("Body").attach(text("hello"));
        let requests = pushover().prepare(&notification);
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].headers["Content-Type"].expose(),
            "application/x-www-form-urlencoded"
        );
        assert!(requests[0].body.text().contains("message=Body"));
        assert!(!requests[0].body.text().contains("hello"));
    }

    #[test]
    fn an_image_pushover_would_refuse_is_a_failure() {
        let too_big = Attachment::new("big.png", vec![0; 5_242_881]);
        let empty = Attachment::new("empty.png", Vec::new());
        let notification = Notification::new("Body")
            .attach(too_big)
            .attach(empty)
            .attach(image("ok.png", "PNGDATA"));
        assert_eq!(pushover().prepare(&notification).len(), 1);
        let outcomes = pushover().plan(&notification).finish();
        // Nothing was sent, so only the two failures are in the plan.
        assert_eq!(
            outcomes,
            [
                Outcome::Failed(TransportError::InvalidRequest),
                Outcome::Failed(TransportError::InvalidRequest)
            ]
        );
    }

    #[test]
    fn a_body_less_message_is_named_after_its_file() {
        let notification = Notification::new("").attach(image("only.png", "PNGDATA"));
        let requests = pushover().prepare(&notification);
        assert!(
            requests[0]
                .body
                .text()
                .contains("name=\"message\"\r\n\r\nonly.png\r\n")
        );
    }
}

#[cfg(feature = "telegram")]
mod telegram {
    use tocsin::{Outcome, TransportError};

    use super::*;

    const BOT: &str = "tgram://123456789:abcdefghijklmn";

    fn upload(name: &str, content: &str) -> Attachment {
        Attachment::new(name, content.as_bytes().to_vec())
    }

    /// `method`, the last segment of each request's URL.
    fn methods(requests: &[tocsin::PreparedRequest]) -> Vec<String> {
        requests
            .iter()
            .map(|r| r.url.expose().rsplit('/').next().unwrap_or("").to_owned())
            .collect()
    }

    #[test]
    fn a_short_text_is_the_caption_of_the_first_file() {
        let notification = Notification::new("Body").attach(upload("pic.png", "PNGDATA"));
        let requests = service(&format!("{BOT}/55")).prepare(&notification);
        assert_eq!(requests.len(), 1, "the caption replaces the message");
        assert_eq!(
            requests[0].url.expose(),
            "https://api.telegram.org/bot123456789:abcdefghijklmn/sendPhoto"
        );
        assert_eq!(
            requests[0].headers["Content-Type"].expose(),
            "multipart/form-data; boundary=tocsin-0"
        );
        let expected = "--tocsin-0\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\nBody\r\n\
             --tocsin-0\r\nContent-Disposition: form-data; name=\"show_caption_above_media\"\r\n\r\ntrue\r\n\
             --tocsin-0\r\nContent-Disposition: form-data; name=\"parse_mode\"\r\n\r\nHTML\r\n\
             --tocsin-0\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\npic.png\r\n\
             --tocsin-0\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n55\r\n\
             --tocsin-0\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"pic.png\"\r\n\r\nPNGDATA\r\n\
             --tocsin-0--\r\n";
        assert_eq!(requests[0].body.text(), expected);
    }

    #[test]
    fn the_caption_follows_the_format_and_the_placement() {
        let notification = Notification::new("*Body*")
            .format(tocsin::Format::Markdown)
            .attach(upload("a.txt", "x"));
        let body = service(&format!("{BOT}/55?content=after&mdv=2")).prepare(&notification)[0]
            .body
            .text()
            .into_owned();
        assert!(body.contains("name=\"show_caption_above_media\"\r\n\r\nfalse\r\n"));
        assert!(body.contains("name=\"parse_mode\"\r\n\r\nMarkdownV2\r\n"));
    }

    #[test]
    fn the_other_files_have_no_caption_and_are_routed_by_media_type() {
        let notification = Notification::new("Body")
            .attach(upload("a.png", "1"))
            .attach(upload("b.gif", "2"))
            .attach(upload("c.mp4", "3"))
            .attach(upload("d.ogg", "4"))
            .attach(upload("e.mp3", "5"))
            .attach(upload("f.txt", "6"))
            .attach(Attachment::new("big.png", vec![0; 10_000_001]));
        let requests = service(&format!("{BOT}/55")).prepare(&notification);
        assert_eq!(
            methods(&requests),
            [
                "sendPhoto",
                "sendAnimation",
                "sendVideo",
                "sendVoice",
                "sendAudio",
                "sendDocument",
                "sendDocument", // a photo over 10 MB goes as a document
            ]
        );
        let fields = [
            "photo",
            "animation",
            "video",
            "voice",
            "audio",
            "document",
            "document",
        ];
        for (request, field) in requests.iter().zip(fields) {
            let body = request.body.text();
            assert!(
                body.contains(&format!("name=\"{field}\"; filename=")),
                "{field}"
            );
            assert_eq!(
                body.contains("name=\"caption\""),
                field == "photo"
                    && request.url.expose().ends_with("sendPhoto")
                    && body.contains("a.png")
            );
        }
    }

    #[test]
    fn a_long_text_is_sent_before_the_files_or_after_them() {
        let long = "word ".repeat(300);
        let notification = Notification::new(long.as_str()).attach(upload("a.txt", "x"));
        let before = service(&format!("{BOT}/55")).prepare(&notification);
        assert_eq!(methods(&before), ["sendMessage", "sendDocument"]);
        assert!(!before[1].body.text().contains("name=\"caption\""));
        let after = service(&format!("{BOT}/55?content=after")).prepare(&notification);
        assert_eq!(methods(&after), ["sendDocument", "sendMessage"]);
    }

    #[test]
    fn without_a_text_only_the_files_are_sent() {
        let notification = Notification::new("").attach(upload("a.txt", "x"));
        let requests = service(&format!("{BOT}/55")).prepare(&notification);
        assert_eq!(methods(&requests), ["sendDocument"]);
        assert!(!requests[0].body.text().contains("name=\"caption\""));
    }

    #[test]
    fn the_files_go_with_one_part_of_a_split_message() {
        let notification = Notification::new("word ".repeat(1000)).attach(upload("a.txt", "x"));
        let requests = service(&format!("{BOT}/55?overflow=split")).prepare(&notification);
        // The text comes first, so the files go with the last part, whose text
        // is short enough to be their caption.
        assert_eq!(methods(&requests), ["sendMessage", "sendDocument"]);
        assert!(requests[1].body.text().contains("name=\"caption\""));
        let after =
            service(&format!("{BOT}/55?overflow=split&content=after")).prepare(&notification);
        // The files go with the first part, which is too long to be their
        // caption, so its text follows them; the other part is a message.
        assert_eq!(
            methods(&after),
            ["sendDocument", "sendMessage", "sendMessage"]
        );
    }

    #[test]
    fn every_chat_gets_the_files_and_a_topic_goes_along() {
        let notification = Notification::new("Body").attach(upload("a.txt", "x"));
        let requests = service(&format!("{BOT}/55/66?topic=7")).prepare(&notification);
        assert_eq!(requests.len(), 2);
        let body = requests[1].body.text();
        assert!(body.contains("name=\"chat_id\"\r\n\r\n66\r\n"));
        assert!(body.contains("name=\"message_thread_id\"\r\n\r\n7\r\n"));
    }

    #[test]
    fn a_file_telegram_refuses_is_a_failure_and_nothing_is_sent_for_it() {
        let notification =
            Notification::new("Body").attach(Attachment::new("huge.bin", vec![0; 50_000_001]));
        let svc = service(&format!("{BOT}/55"));
        assert!(svc.prepare(&notification).is_empty());
        assert_eq!(
            svc.plan(&notification).finish(),
            [Outcome::Failed(TransportError::InvalidRequest)]
        );
    }

    #[test]
    fn a_detected_chat_gets_the_files_too() {
        let notification = Notification::new("Body").attach(upload("a.txt", "x"));
        let mut plan = service(BOT).plan(&notification);
        let first = plan.next_request().expect("the lookup");
        assert!(first.url.expose().ends_with("/getUpdates"));
        plan.report(Ok(tocsin::Response::with_body(
            200,
            r#"{"ok":true,"result":[{"message":{"from":{"id":77}}}]}"#,
        )));
        let upload = plan.next_request().expect("the upload");
        assert!(upload.url.expose().ends_with("/sendDocument"));
        assert!(
            upload
                .body
                .text()
                .contains("name=\"chat_id\"\r\n\r\n77\r\n")
        );
    }
}

#[cfg(feature = "discord")]
mod discord {
    use super::*;

    const HOOK: &str = "discord://FakeBot@FAKE_id/FAKE_token";

    fn file(name: &str, size: usize) -> Attachment {
        Attachment::new(name, vec![b'x'; size])
    }

    #[test]
    fn files_follow_the_message_as_multipart() {
        let notification = Notification::new("Body")
            .attach(file("a.png", 3))
            .attach(Attachment::new("", b"yz".to_vec()));
        let requests = service(HOOK).prepare(&notification);
        assert_eq!(
            requests.len(),
            2,
            "the message, then one post of both files"
        );
        assert!(
            requests[0].headers["Content-Type"]
                .expose()
                .starts_with("application/json")
        );
        let upload = &requests[1];
        assert_eq!(
            upload.url.expose(),
            "https://discord.com/api/webhooks/FAKE%5Fid/FAKE%5Ftoken?wait=true"
        );
        assert_eq!(
            upload.headers["Content-Type"].expose(),
            "multipart/form-data; boundary=tocsin-0"
        );
        let expected = "--tocsin-0
Content-Disposition: form-data; name=\"payload_json\"

{\"tts\":false,\"username\":\"FakeBot\"}
--tocsin-0
Content-Disposition: form-data; name=\"files[0]\"; filename=\"a.png\"
Content-Type: image/png

xxx
--tocsin-0
Content-Disposition: form-data; name=\"files[1]\"; filename=\"file002.dat\"
Content-Type: application/octet-stream

yz
--tocsin-0--
"
        .replace('\n', "\r\n");
        assert_eq!(upload.body.text(), expected);
    }

    #[test]
    fn the_upload_waits_even_when_the_message_does_not() {
        let notification = Notification::new("Body").attach(file("a.txt", 1));
        let requests = service(&format!("{HOOK}?tts=yes")).prepare(&notification);
        assert!(requests[0].url.expose().ends_with("?wait=false"));
        assert!(requests[1].url.expose().ends_with("?wait=true"));
        // Text to speech is for the message only.
        assert!(requests[1].body.text().contains("{\"tts\":false"));
    }

    #[test]
    fn ten_files_or_25_mib_share_a_message() {
        let eleven: Vec<_> = (0..11).map(|n| file(&format!("{n}.txt"), 1)).collect();
        let notification = eleven
            .into_iter()
            .fold(Notification::new("Body"), Notification::attach);
        assert_eq!(service(HOOK).prepare(&notification).len(), 1 + 2);

        let big = Notification::new("Body")
            .attach(file("a.bin", 20 * 1024 * 1024))
            .attach(file("b.bin", 6 * 1024 * 1024));
        assert_eq!(service(HOOK).prepare(&big).len(), 1 + 2);

        // Without batching, every file is a message of its own.
        let two = Notification::new("Body")
            .attach(file("a", 1))
            .attach(file("b", 1));
        assert_eq!(
            service(&format!("{HOOK}?batch=no")).prepare(&two).len(),
            1 + 2
        );
        assert_eq!(service(HOOK).prepare(&two).len(), 1 + 1);
    }

    #[test]
    fn only_files_means_no_message() {
        let notification = Notification::new("").attach(file("a.txt", 1));
        let requests = service(HOOK).prepare(&notification);
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].headers["Content-Type"]
                .expose()
                .starts_with("multipart/")
        );
    }

    #[test]
    fn the_files_go_with_the_first_part_of_a_long_message() {
        let notification = Notification::new("word ".repeat(1000)).attach(file("a.txt", 1));
        let requests = service(&format!("{HOOK}?overflow=split")).prepare(&notification);
        let kinds: Vec<_> = requests
            .iter()
            .map(|r| {
                r.headers["Content-Type"]
                    .expose()
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "application/json",
                "multipart/form-data",
                "application/json",
                "application/json"
            ]
        );
    }
}

#[cfg(feature = "xml")]
#[test]
fn xml_webhook_carries_attachments_as_base64_elements() {
    let notification = Notification::new("Body")
        .attach(text("hello"))
        .attach(Attachment::new("", b"x".to_vec()).mime("a/b\"c"));
    let requests = service("xml://localhost/hook").prepare(&notification);
    assert_eq!(requests.len(), 1);
    let body = requests[0].body.text();
    assert!(
        body.contains(
            "<Attachments format=\"base64\">\
<Attachment filename=\"a.txt\" mimetype=\"text/plain\">aGVsbG8=</Attachment>\
<Attachment filename=\"file002.dat\" mimetype=\"a/b&quot;c\">eA==</Attachment>\
</Attachments>"
        ),
        "{body}"
    );

    // Only the first part of a long message has them.
    let long = Notification::new("word ".repeat(8000)).attach(text("hello"));
    let requests = service("xml://localhost/hook?overflow=split").prepare(&long);
    assert_eq!(requests.len(), 2);
    assert!(requests[0].body.text().contains("<Attachments"));
    assert!(!requests[1].body.text().contains("<Attachments"));
}
