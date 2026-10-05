//! The real blocking transport, against a server on the loopback interface.
#![cfg(feature = "ureq")]

use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};

use tocsin::{Method, PreparedRequest, SecretString, Transport, TransportError, UreqTransport};

/// Serve exactly one request with `status` and return what was received.
///
/// The response echoes secrets on purpose: the transport must discard it.
fn loopback(status: u16) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("nonblocking");
    let worker = std::thread::spawn(move || {
        let started = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        started.elapsed() < Duration::from_secs(5),
                        "no loopback request"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        // On Windows the accepted stream inherits the listener's non-blocking mode.
        stream.set_nonblocking(false).expect("blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut data = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let count = stream.read(&mut buffer).expect("request read");
            assert!(count > 0, "early EOF");
            data.extend_from_slice(&buffer[..count]);
            if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break;
                }
            }
            assert!(data.len() < 16384, "request too large");
        }
        let echo = "FAKE_query_secret FAKE_body_secret";
        write!(
            stream,
            "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{echo}",
            echo.len()
        )
        .expect("response");
        String::from_utf8(data).expect("request UTF-8")
    });
    (format!("http://{addr}/?token=FAKE_query_secret"), worker)
}

#[test]
fn success_and_http_failure_are_typed_and_leak_nothing() {
    for status in [200, 400] {
        let (url, worker) = loopback(status);
        let request =
            PreparedRequest::json(url, &serde_json::json!({"message": "FAKE_body_secret"}));
        let result = UreqTransport.send(&request);
        let received = worker.join().expect("worker");
        assert!(received.starts_with("POST /?token=FAKE_query_secret HTTP/1.1\r\n"));
        assert!(received.contains("FAKE_body_secret"));
        // The server echoes secrets in its answer; the answer is kept for plans
        // that need it, but formatting it must not show it.
        let output = format!("{result:?}");
        assert!(!output.contains("FAKE_query_secret") && !output.contains("FAKE_body_secret"));
        if status == 200 {
            let response = result.expect("success");
            assert_eq!(response.status, 200);
            assert_eq!(response.body.text(), "FAKE_query_secret FAKE_body_secret");
        } else {
            assert_eq!(
                result.expect_err("HTTP failure"),
                TransportError::HttpStatus(400)
            );
        }
    }
}

/// Apprise's JSON webhook lets the user pick any of these methods, and it sends
/// the body whatever the method is.
fn method_reaches_the_server_with_its_body(method: Method) {
    let (url, worker) = loopback(200);
    let mut request =
        PreparedRequest::json(url, &serde_json::json!({"message": "FAKE_body_secret"}));
    request.method = method;
    let result = UreqTransport.send(&request);
    let received = worker.join().expect("worker");
    let expected = format!("{method} /?token=FAKE_query_secret HTTP/1.1");
    assert_eq!(received.lines().next(), Some(expected.as_str()));
    assert!(received.contains("FAKE_body_secret"), "{received}");
    assert_eq!(result.expect("delivered").status, 200);
}

macro_rules! method_tests {
    ($($name:ident: $method:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                method_reaches_the_server_with_its_body(Method::$method);
            }
        )*
    };
}

method_tests! {
    get_carries_its_body: Get,
    put_carries_its_body: Put,
    patch_carries_its_body: Patch,
    delete_carries_its_body: Delete,
    head_carries_its_body: Head,
    options_carries_its_body: Options,
}

#[test]
fn update_is_refused_because_it_is_not_a_registered_method() {
    let mut request = PreparedRequest::json("http://127.0.0.1:1/", &serde_json::json!({}));
    request.method = Method::Update;
    assert_eq!(
        UreqTransport.send(&request),
        Err(TransportError::UnsupportedMethod)
    );
}

#[test]
fn unsafe_or_malformed_requests_are_refused_before_any_connection() {
    let mut request = PreparedRequest::json(
        "http://127.0.0.1:1/?token=FAKE_query_secret",
        &serde_json::json!({}),
    );
    request.policy.verify_tls = false;
    assert_eq!(
        UreqTransport.send(&request),
        Err(TransportError::UnsupportedPolicy)
    );
    request.policy.verify_tls = true;
    request.policy.read_timeout = f64::NAN;
    assert_eq!(
        UreqTransport.send(&request),
        Err(TransportError::UnsupportedPolicy)
    );
    request.policy.read_timeout = 1.0;
    request.url = SecretString::new("ftp://FAKE_secret@example.com/");
    assert_eq!(
        UreqTransport.send(&request),
        Err(TransportError::InvalidRequest)
    );
}
