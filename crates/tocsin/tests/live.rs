//! Checks against real servers over TLS: `httpbin.org`, which parses what it
//! receives with its own HTTP stack and echoes it, and `ntfy.sh`, which takes
//! anonymous messages.
//!
//! They need the network and leave messages on a public service, so they are
//! ignored unless asked for:
//!
//! ```sh
//! cargo test -p tocsin --all-features --test live -- --ignored --test-threads=1
//! ```
//!
//! Every message is harmless text sent to a topic with a random name, and no
//! credential is involved.
#![cfg(all(
    feature = "ureq",
    feature = "json",
    feature = "form",
    feature = "xml",
    feature = "ntfy"
))]

use std::{thread::sleep, time::Duration};

use serde_json::Value;
use tocsin::{Attachment, Notification, Notifier, Outcome, UreqTransport};

/// Send `notification` to `url` and return what the server answered, as JSON.
fn deliver(url: &str, notification: &Notification) -> Value {
    let mut notifier = Notifier::new();
    notifier.add(url).expect("URL");
    let report = notifier.send(notification, &mut UreqTransport);
    let receipt = report.receipts().first().expect("a receipt");
    let Outcome::Delivered(response) = &receipt.outcome else {
        panic!("not delivered: {:?}", report.receipts());
    };
    serde_json::from_str(&response.body.text()).expect("the server echoes JSON")
}

fn attachment() -> Attachment {
    Attachment::new("note.txt", b"hello from tocsin".to_vec())
}

#[test]
#[ignore = "needs the network"]
fn the_json_webhook_reaches_a_real_server_with_its_file() {
    let notification = Notification::new("the body")
        .title("the title")
        .attach(attachment());
    let echo = deliver("jsons://httpbin.org/post", &notification);
    assert_eq!(echo["json"]["message"], "the body");
    assert_eq!(echo["json"]["title"], "the title");
    assert_eq!(echo["json"]["type"], "info");
    assert_eq!(echo["json"]["attachments"][0]["filename"], "note.txt");
    // "hello from tocsin" in base64
    assert_eq!(
        echo["json"]["attachments"][0]["base64"],
        "aGVsbG8gZnJvbSB0b2NzaW4="
    );
    assert!(
        echo["headers"]["Content-Type"]
            .as_str()
            .is_some_and(|value| value.starts_with("application/json"))
    );
}

#[test]
#[ignore = "needs the network"]
fn the_form_webhook_is_read_by_a_real_server_urlencoded_and_multipart() {
    let plain = deliver("forms://httpbin.org/post", &Notification::new("a & b = c"));
    assert_eq!(plain["form"]["message"], "a & b = c");
    assert_eq!(plain["form"]["type"], "info");

    // With a file, the body is multipart/form-data, which the server parses
    // itself: the file comes back under its field name, with its content.
    let notification = Notification::new("with a file").attach(attachment());
    let multipart = deliver("forms://httpbin.org/post", &notification);
    assert_eq!(multipart["form"]["message"], "with a file");
    assert_eq!(multipart["files"]["file01"], "hello from tocsin");
    assert!(
        multipart["headers"]["Content-Type"]
            .as_str()
            .is_some_and(|value| value.starts_with("multipart/form-data; boundary="))
    );
}

#[test]
#[ignore = "needs the network"]
fn a_form_get_and_basic_authentication_work_against_a_real_server() {
    let echo = deliver(
        "forms://httpbin.org/get?method=get",
        &Notification::new("via get"),
    );
    assert_eq!(echo["args"]["message"], "via get");

    let echo = deliver(
        "jsons://user:pa%20ss@httpbin.org/post",
        &Notification::new("x"),
    );
    // "user:pa ss" in base64
    assert_eq!(echo["headers"]["Authorization"], "Basic dXNlcjpwYSBzcw==");
}

#[test]
#[ignore = "needs the network"]
fn the_xml_webhook_reaches_a_real_server() {
    let notification = Notification::new("a <b> & c")
        .title("t")
        .attach(attachment());
    let echo = deliver("xmls://httpbin.org/post", &notification);
    let data = echo["data"].as_str().expect("the XML text");
    assert!(
        data.contains("<Message>a &lt;b&gt; &amp; c</Message>"),
        "{data}"
    );
    assert!(data.contains("<Attachment filename=\"note.txt\" mimetype=\"text/plain\">aGVsbG8gZnJvbSB0b2NzaW4=</Attachment>"));
    assert_eq!(echo["headers"]["Content-Type"], "application/xml");
}

/// A topic nobody else will use.
fn random_topic() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    format!("tocsin-live-{}-{nanos}", std::process::id())
}

/// What ntfy.sh holds for a topic, one JSON object per line.
fn ntfy_events(topic: &str) -> Vec<Value> {
    let mut response = ureq::get(&format!("https://ntfy.sh/{topic}/json?poll=1&since=all"))
        .call()
        .expect("ntfy.sh answers");
    response
        .body_mut()
        .read_to_string()
        .expect("text")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["event"] == "message")
        .collect()
}

/// Poll until the topic holds `count` messages (the service needs a moment).
fn wait_for_messages(topic: &str, count: usize) -> Vec<Value> {
    for _ in 0..10 {
        let events = ntfy_events(topic);
        if events.len() >= count {
            return events;
        }
        sleep(Duration::from_secs(1));
    }
    ntfy_events(topic)
}

#[test]
#[ignore = "needs the network"]
fn a_message_reaches_ntfy_sh_and_can_be_read_back() {
    let topic = random_topic();
    let url = format!("ntfy://{topic}?priority=high&tags=tocsin,live");
    let mut notifier = Notifier::new();
    notifier.add(&url).expect("URL");
    let notification = Notification::new("a live message").title("tocsin live test");
    let report = notifier.send(&notification, &mut UreqTransport);
    assert!(report.is_success(), "{:?}", report.receipts());

    let events = wait_for_messages(&topic, 1);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["title"], "tocsin live test");
    assert_eq!(events[0]["message"], "a live message");
    assert_eq!(events[0]["priority"], 4);
    // Apprise sorts the tags, and so does tocsin.
    assert_eq!(events[0]["tags"], serde_json::json!(["live", "tocsin"]));
}

#[test]
#[ignore = "needs the network"]
fn a_file_reaches_ntfy_sh_and_comes_back_unchanged() {
    let topic = random_topic();
    let mut notifier = Notifier::new();
    notifier.add(&format!("ntfy://{topic}")).expect("URL");
    let bytes: Vec<u8> = (0..=255).collect();
    let notification = Notification::new("a file for you")
        .title("live file")
        .attach(Attachment::new("bytes.bin", bytes.clone()));
    let report = notifier.send(&notification, &mut UreqTransport);
    assert!(report.is_success(), "{:?}", report.receipts());

    let events = wait_for_messages(&topic, 1);
    assert_eq!(events.len(), 1, "{events:?}");
    let attachment = &events[0]["attachment"];
    assert_eq!(attachment["name"], "bytes.bin", "{events:?}");
    assert_eq!(attachment["size"], 256);
    assert_eq!(events[0]["title"], "live file");
    assert_eq!(events[0]["message"], "a file for you");

    let url = attachment["url"]
        .as_str()
        .expect("the attachment's address");
    let mut downloaded = ureq::get(url).call().expect("the attachment downloads");
    let content = downloaded.body_mut().read_to_vec().expect("the content");
    assert_eq!(content, bytes);
}
