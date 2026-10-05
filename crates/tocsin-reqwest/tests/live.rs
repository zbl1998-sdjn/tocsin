//! `ReqwestTransport` against real servers over TLS (rustls with the platform's
//! certificate verifier): `httpbin.org`, which parses what it receives and echoes
//! it, and `ntfy.sh`, which takes anonymous messages.
//!
//! They need the network and leave a message on a public service, so they are
//! ignored unless asked for:
//!
//! ```sh
//! cargo test -p tocsin-reqwest --test live -- --ignored --test-threads=1
//! ```
//!
//! Every message is harmless text sent to a topic with a random name, and no
//! credential is involved.

use std::time::Duration;

use tocsin::{Attachment, Notification, Notifier, Outcome};
use tocsin_reqwest::ReqwestTransport;

async fn deliver(url: &str, notification: &Notification) -> String {
    let mut notifier = Notifier::new();
    notifier.add(url).expect("URL");
    let report = notifier
        .send_async(notification, &mut ReqwestTransport::new())
        .await;
    let Some(Outcome::Delivered(response)) = report.receipts().first().map(|r| r.outcome.clone())
    else {
        panic!("not delivered: {:?}", report.receipts());
    };
    response.body.text().into_owned()
}

#[tokio::test]
#[ignore = "needs the network"]
async fn a_multipart_form_is_read_by_a_real_server_over_tls() {
    let notification = Notification::new("over tls").attach(Attachment::new(
        "note.txt",
        b"hello from tocsin-reqwest".to_vec(),
    ));
    let echo = deliver("forms://httpbin.org/post", &notification).await;
    // httpbin parses the multipart body itself and shows the file under its field.
    assert!(
        echo.contains("\"file01\": \"hello from tocsin-reqwest\""),
        "{echo}"
    );
    assert!(echo.contains("\"message\": \"over tls\""), "{echo}");
}

#[tokio::test]
#[ignore = "needs the network"]
async fn a_server_that_rejects_us_is_a_failure_with_its_status() {
    let mut notifier = Notifier::new();
    // httpbin answers this path with the status in it.
    notifier.add("jsons://httpbin.org/status/418").expect("URL");
    let report = notifier
        .send_async(&Notification::new("x"), &mut ReqwestTransport::new())
        .await;
    assert_eq!(
        report
            .failures()
            .map(|r| r.outcome.clone())
            .collect::<Vec<_>>(),
        [Outcome::Failed(tocsin::TransportError::HttpStatus(418))]
    );
}

fn random_topic() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    format!("tocsin-reqwest-live-{}-{nanos}", std::process::id())
}

async fn read_back(topic: &str, wanted: usize) -> Vec<String> {
    let url = format!("https://ntfy.sh/{topic}/json?poll=1&since=all");
    for _ in 0..10 {
        let text = reqwest::get(&url)
            .await
            .expect("ntfy.sh answers")
            .text()
            .await
            .expect("text");
        let lines: Vec<String> = text
            .lines()
            .filter(|line| line.contains("\"event\":\"message\""))
            .map(str::to_owned)
            .collect();
        if lines.len() >= wanted {
            return lines;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Vec::new()
}

#[tokio::test]
#[ignore = "needs the network"]
async fn a_message_and_a_file_reach_ntfy_sh_over_tls() {
    let topic = random_topic();
    let mut notifier = Notifier::new();
    notifier.add(&format!("ntfy://{topic}")).expect("URL");
    let mut transport = ReqwestTransport::new();

    let text = Notification::new("a live message").title("tocsin-reqwest live test");
    assert!(
        notifier
            .send_async(&text, &mut transport)
            .await
            .is_success()
    );
    let file = Notification::new("a file for you")
        .attach(Attachment::new("bytes.bin", (0..=255).collect::<Vec<u8>>()));
    assert!(
        notifier
            .send_async(&file, &mut transport)
            .await
            .is_success()
    );

    let lines = read_back(&topic, 2).await;
    assert_eq!(lines.len(), 2, "{lines:?}");
    let all = lines.join("\n");
    assert!(
        all.contains("\"title\":\"tocsin-reqwest live test\""),
        "{all}"
    );
    assert!(all.contains("\"message\":\"a live message\""), "{all}");
    assert!(all.contains("\"name\":\"bytes.bin\""), "{all}");
    assert!(all.contains("\"size\":256"), "{all}");
}
