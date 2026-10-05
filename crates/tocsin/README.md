# tocsin

Send one notification to many services from a single URL. tocsin reads the
notification URLs of [Apprise](https://github.com/caronc/apprise) (`tgram://...`,
`slack://...`, `ntfy://...`) and turns them into HTTP requests.

Telegram, Discord, ntfy, Gotify, Slack, Mattermost, Rocket.Chat, Pushover,
Pushbullet, Prowl, IFTTT, Zulip, PagerDuty, Google Chat, Microsoft Teams
(workflows) and a JSON, an XML and a form webhook are supported today. The URL
parser is checked against Apprise itself: the tests compare every field of more
than 600 URLs with what Apprise parses.

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
- `Service::plan` hands out the requests one at a time without I/O, so you can
  send them with your own HTTP client; some services need an answer before the
  next request, and a plan knows how to ask for it. `Service::prepare` is the
  list form for the services that never do.
- `Notification::attach` sends files with the services that take them.
- Each service is a Cargo feature. `ureq` adds a blocking transport, and the
  `tocsin-reqwest` crate an async one (`Notifier::send_async`).

See the [repository README](../../README.md) for the command line, the list of
options each service applies, and how tocsin differs from Apprise.

Licensed under MIT or Apache-2.0.
