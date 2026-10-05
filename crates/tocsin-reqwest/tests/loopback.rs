//! The transport against a server on the loopback interface. Nothing leaves the
//! machine.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tocsin::{
    AsyncTransport, Attachment, Notification, Notifier, Outcome, PreparedRequest, Service,
    TransportError,
};
use tocsin_reqwest::ReqwestTransport;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
    time::timeout,
};

/// What the server saw of one request.
#[derive(Clone, Debug)]
struct Seen {
    head: String,
    body: Vec<u8>,
}

/// A server that answers `requests` requests, one connection each, with what
/// `respond` makes of the request line, and keeps what it received.
async fn serve<F>(requests: usize, respond: F) -> (u16, JoinHandle<Vec<Seen>>)
where
    F: Fn(&str) -> (u16, String) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let worker = tokio::spawn(async move {
        for _ in 0..requests {
            let (mut stream, _) = timeout(Duration::from_secs(10), listener.accept())
                .await
                .expect("a request in time")
                .expect("accept");
            let mut data = Vec::new();
            let mut buffer = [0u8; 4096];
            let (head, body) = loop {
                let count = stream.read(&mut buffer).await.expect("read");
                assert!(count > 0, "early EOF");
                data.extend_from_slice(&buffer[..count]);
                let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&data[..end]).into_owned();
                let length = head
                    .to_ascii_lowercase()
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break (head, data[end + 4..end + 4 + length].to_vec());
                }
            };
            let (status, answer) = respond(head.lines().next().unwrap_or(""));
            kept.lock().expect("lock").push(Seen { head, body });
            let response = format!(
                "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            stream.write_all(response.as_bytes()).await.expect("write");
            let _ = stream.shutdown().await;
        }
        seen.lock().expect("lock").clone()
    });
    (port, worker)
}

fn notifier(url: &str) -> Notifier {
    let mut notifier = Notifier::new();
    notifier.add(url).expect("URL");
    notifier
}

#[tokio::test]
async fn a_message_reaches_the_server() {
    let (port, worker) = serve(1, |_| (200, String::new())).await;
    let mut transport = ReqwestTransport::new();
    let report = notifier(&format!("json://127.0.0.1:{port}/hook?-flag=yes"))
        .send_async(
            &Notification::new("disk is full").title("server"),
            &mut transport,
        )
        .await;
    assert!(report.is_success(), "{report:?}");
    let seen = worker.await.expect("worker");
    assert!(
        seen[0].head.starts_with("POST /hook?flag=yes HTTP/1.1"),
        "{}",
        seen[0].head
    );
    let body = String::from_utf8_lossy(&seen[0].body);
    assert!(body.contains("\"message\":\"disk is full\""), "{body}");
    assert!(body.contains("\"title\":\"server\""), "{body}");
}

#[tokio::test]
async fn an_error_status_is_a_failure() {
    let (port, worker) = serve(1, |_| (500, "no".to_owned())).await;
    let mut transport = ReqwestTransport::new();
    let report = notifier(&format!("json://127.0.0.1:{port}/hook"))
        .send_async(&Notification::new("hi"), &mut transport)
        .await;
    worker.await.expect("worker");
    assert_eq!(
        report
            .failures()
            .map(|r| r.outcome.clone())
            .collect::<Vec<_>>(),
        [Outcome::Failed(TransportError::HttpStatus(500))]
    );
}

#[tokio::test]
async fn a_service_that_needs_answers_works_through_the_plan() {
    // Rocket.Chat's basic mode logs in, posts and logs out; the token of the
    // login answer has to travel in the headers of the next requests.
    let (port, worker) = serve(3, |line| {
        if line.contains("/api/v1/login") {
            (
                200,
                r#"{"status":"success","data":{"authToken":"FAKE_token","userId":"FAKE_id"}}"#
                    .to_owned(),
            )
        } else {
            (200, "{}".to_owned())
        }
    })
    .await;
    let mut transport = ReqwestTransport::new();
    let url = format!("rocket://user:pass@127.0.0.1:{port}/room1");
    let report = notifier(&url)
        .send_async(&Notification::new("Body"), &mut transport)
        .await;
    assert!(report.is_success(), "{report:?}");
    let seen = worker.await.expect("worker");
    assert!(seen[0].head.starts_with("POST /api/v1/login"));
    assert_eq!(seen[0].body, b"username=user&password=pass");
    let head = seen[1].head.to_ascii_lowercase();
    assert!(head.contains("x-auth-token: fake_token"), "{head}");
    assert!(head.contains("x-user-id: fake_id"), "{head}");
    assert!(seen[2].head.starts_with("POST /api/v1/logout"));
}

#[tokio::test]
async fn a_multipart_body_goes_through_unchanged() {
    let (port, worker) = serve(1, |_| (200, String::new())).await;
    let mut transport = ReqwestTransport::new();
    let notification = Notification::new("Body").attach(Attachment::new("a.bin", vec![0, 255, 7]));
    let report = notifier(&format!("form://127.0.0.1:{port}/hook"))
        .send_async(&notification, &mut transport)
        .await;
    assert!(report.is_success(), "{report:?}");
    let seen = worker.await.expect("worker");
    let head = seen[0].head.to_ascii_lowercase();
    assert!(
        head.contains("content-type: multipart/form-data; boundary=tocsin-0"),
        "{head}"
    );
    assert!(seen[0].body.windows(3).any(|w| w == [0, 255, 7]));
}

#[tokio::test]
async fn a_response_body_is_kept_but_never_shown() {
    let (port, worker) = serve(1, |_| (200, "FAKE_echoed_secret".to_owned())).await;
    let service: Service = format!("json://127.0.0.1:{port}/hook")
        .parse()
        .expect("service");
    let request: PreparedRequest = service.prepare(&Notification::new("x")).remove(0);
    let response = ReqwestTransport::new()
        .send(&request)
        .await
        .expect("response");
    worker.await.expect("worker");
    assert_eq!(response.body.text(), "FAKE_echoed_secret");
    assert!(!format!("{response:?}").contains("FAKE_echoed_secret"));
}

#[tokio::test]
async fn unsafe_or_malformed_requests_are_refused_before_any_connection() {
    let mut transport = ReqwestTransport::new();
    let service: Service = "json://127.0.0.1:1/hook?verify=no"
        .parse()
        .expect("service");
    let insecure = service.prepare(&Notification::new("x")).remove(0);
    assert_eq!(
        transport.send(&insecure).await.map(|_| ()),
        Err(TransportError::UnsupportedPolicy)
    );

    let service: Service = "json://127.0.0.1:1/hook?cto=0".parse().expect("service");
    let no_time = service.prepare(&Notification::new("x")).remove(0);
    assert_eq!(
        transport.send(&no_time).await.map(|_| ()),
        Err(TransportError::UnsupportedPolicy)
    );

    let mut odd = service.prepare(&Notification::new("x")).remove(0);
    odd.url = tocsin::SecretString::new("ftp://127.0.0.1/hook");
    odd.policy = tocsin::RequestPolicy::default();
    assert_eq!(
        transport.send(&odd).await.map(|_| ()),
        Err(TransportError::InvalidRequest)
    );
}

#[tokio::test]
async fn a_connection_that_fails_is_a_failure_without_details() {
    // Nothing listens on this port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    let secret = "FAKE_query_token";
    let url = format!("json://127.0.0.1:{port}/hook?-token={secret}&cto=2&rto=2");
    let report = notifier(&url)
        .send_async(&Notification::new("x"), &mut ReqwestTransport::new())
        .await;
    assert_eq!(
        report
            .failures()
            .map(|r| r.outcome.clone())
            .collect::<Vec<_>>(),
        [Outcome::Failed(TransportError::Connection)]
    );
    assert!(!format!("{report:?}").contains(secret));
}

/// A task that is spawned on a multi-threaded runtime has to be `Send`.
#[test]
fn sending_can_be_spawned_on_a_multi_threaded_runtime() {
    fn assert_send<T: Send>(_: &T) {}

    let notifier = notifier("json://127.0.0.1:1/hook");
    let notification = Notification::new("x");
    let mut transport = ReqwestTransport::new();
    let future = notifier.send_async(&notification, &mut transport);
    assert_send(&future);
}
