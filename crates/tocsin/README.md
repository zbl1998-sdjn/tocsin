# tocsin

Send one notification to many services from a single URL. tocsin reads the
notification URLs of [Apprise](https://github.com/caronc/apprise) (`tgram://...`,
`slack://...`, `ntfy://...`) and turns them into HTTP requests.

Telegram, Discord, ntfy, Gotify, Slack, Mattermost, Rocket.Chat, Pushover,
Microsoft Teams (workflows) and a JSON and a form webhook are supported today.
The URL parser is checked against Apprise itself: the tests compare every field
of nearly 400 URLs with what Apprise parses.

```rust
use tocsin::{Notification, Notifier, UreqTransport};

let mut notifier = Notifier::new();
notifier.add("ntfy://my-topic")?;
let report = notifier.send(&Notification::new("Backup finished"), &mut UreqTransport);
if !report.is_success() {
    eprintln!("{} requests failed", report.failures().count());
}
```

- Secrets stay out of `Debug`, `Display`, errors and logs.
- `Service::prepare` builds the requests without I/O, so you can send them with
  your own HTTP client.
- Each service is a Cargo feature. `ureq` adds a blocking transport.

See the [repository README](../../README.md) for the command line, the list of
options each service applies, and how tocsin differs from Apprise.

Licensed under MIT or Apache-2.0.
