# tocsin-reqwest

A non-blocking transport for [tocsin](https://github.com/zbl1998-sdjn/tocsin),
built on [reqwest](https://docs.rs/reqwest). tocsin turns
[Apprise](https://github.com/caronc/apprise) notification URLs into HTTP
requests; this crate sends them from async code.

```rust
use tocsin::{Notification, Notifier};
use tocsin_reqwest::ReqwestTransport;

let mut notifier = Notifier::new();
notifier.add("ntfy://my-topic")?;

let mut transport = ReqwestTransport::new();
let report = notifier
    .send_async(&Notification::new("Backup finished"), &mut transport)
    .await;
assert!(report.is_success());
```

`Notifier::send_async` takes its services one after the other. To send to
several at once, give each service its own `Notifier` and join the futures.

It behaves like tocsin's blocking `UreqTransport`: it never turns TLS
verification off (`verify=no` in a URL is refused), it keeps at most 1 MiB of a
response body, it follows at most five redirects, and no error carries a URL, a
header or a body. Clients are reused for requests that share their timeouts and
redirect setting, so connections are pooled.

## Features

`default = ["rustls", "system-proxy"]`: `rustls` is reqwest's TLS with the
platform's certificate verifier, and `system-proxy` reads `HTTP_PROXY`,
`HTTPS_PROXY` and `ALL_PROXY`. For another TLS backend, use
`default-features = false` and depend on `reqwest` yourself with the feature you
want; Cargo gives both crates the same reqwest.

Like most HTTP clients, reqwest can write URLs to its logs at the debug and trace
levels, and tocsin URLs contain tokens. Keep those levels off for it.

Licensed under MIT or Apache-2.0.
