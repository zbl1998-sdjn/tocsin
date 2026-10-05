# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

- Bark (`bark://`, `barks://`), Signal through signal-cli-rest-api (`signal://`,
  `signals://`) and Home Assistant (`hassio://`, `hassios://`), each with at
  least 40 fixtures that agree with Apprise. Their differences are in
  `DESIGN.md`: a Bark URL with `key=` sends nothing, because tocsin does not
  encrypt; Home Assistant gets the token as a bearer token and ignores the
  user and password of the URL.
- `tests/live.rs` in `tocsin` and `tocsin-reqwest`: opt-in tests
  (`cargo test --test live -- --ignored`) that send over TLS to `httpbin.org`
  and `ntfy.sh` with both transports and read what arrived.

## [0.2.0] - 2026-10-05

This release adds attachments, an async transport, seven more services, and the
answers that some services need before the next request. It breaks the API in
three places, listed first.

### Changed (breaking)

- `Service::plan` returns a `Plan`, a state machine that hands out the next
  request and reads the result, for deliveries in which a request depends on the
  answer to the one before. `Notifier::send` uses it. `Service::prepare` still
  builds the requests that need no answer.
- Request and response bodies are bytes. `PreparedRequest::body` is a
  `SecretBytes` (redacted in `Debug` and `Display`), and the `http` feature
  converts to `http::Request<Vec<u8>>`.
- `Response` carries the start of the response body (`UreqTransport` keeps up to
  1 MiB), is no longer `Copy`, and is `#[non_exhaustive]`. `Outcome` and
  `Receipt` are no longer `Copy`. `Notification` has an `attachments` field.
- `TransportError` has two new variants: `InvalidResponse` (a service answered,
  but not in a way the next request could be built from) and `Rejected` (it
  answered without an error status but said it did not take the message, as
  Slack does with `"ok": false`).

### Added

- Attachments. `Notification::attach` takes an `Attachment` (a name, a media
  type guessed from the extension, and the bytes). Telegram, Discord, ntfy,
  Pushover, Pushbullet, Mattermost (bot), Slack (bot), and the JSON, XML and
  form webhooks send them, the way Apprise does. A file goes with the first part
  of a long message, and `overflow=truncate` keeps only the first file. A file
  that a service would refuse is a failure in the report.
- `tocsin --attach FILE` (repeatable); a file can be the whole message.
- `Notifier::send_async` and the `AsyncTransport` trait. `MockTransport`
  implements both transports.
- The `tocsin-reqwest` crate: `ReqwestTransport`, an `AsyncTransport` built on
  reqwest 0.13.
- Seven services, each with at least 30 URLs compared against Apprise: Prowl,
  IFTTT (and the `maker.ifttt.com/use` address), Pushbullet, Zulip, an XML
  webhook, PagerDuty and Google Chat (and the `chat.googleapis.com` address).
- Rocket.Chat `basic` mode (user name and password): log in, post to every
  target, log out.
- Mattermost bot mode: channels written as names are looked up in the team
  before posting.
- Slack bot mode: e-mail address targets are resolved to users first.
- Telegram without a chat id (`detect`): the bot is asked who wrote to it last.
- ntfy sends `tags` (`X-Tags`), `attach` and `filename`.
- Slack reads the answer, as Apprise does: a webhook has to answer `ok` and the
  Web API has to say `"ok": true`, otherwise the message is reported as
  rejected. A Slack target that cannot be used is a failure in the report
  instead of being skipped without a word.

### Notes

- Files, lookups, logins and detection only work through `Service::plan` (and so
  through `Notifier`); `prepare` returns what it can without them.
- IFTTT's `+key` and `-key` work as documented. Apprise's URL never applies them
  (a naming mismatch inside its plugin), so the differential test leaves them out.
- A dry run of the command line does not count the requests that depend on an
  answer as failures.
- The bot token of Slack and Pushbullet is not sent to the upload address that
  the service returns, unlike Apprise.

## [0.1.0] - 2026-10-05

First public version: the `tocsin` library and the `tocsin-cli` command line,
both on crates.io.

### Added

- `tocsin` library: parse Apprise notification URLs into typed services, build
  the HTTP requests without I/O (`Service::prepare`), and send them with a
  `Transport` (`UreqTransport`, `MockTransport`, or your own).
- Services, each behind a feature: Telegram, Discord, ntfy, Gotify, Slack,
  Mattermost, Rocket.Chat, Pushover, Microsoft Teams (workflows), a JSON webhook
  and a form webhook. The web addresses that Slack, Discord, ntfy.sh,
  Mattermost and Power Automate show are accepted too.
- `http` feature: `TryFrom<&PreparedRequest>` for `http::Request<String>`, so an
  async client such as `reqwest` or `hyper` can send what `prepare` builds.
- `Notifier` and `Report` for sending to several services without stopping at
  the first failure.
- Secrets are redacted in `Debug`, `Display`, errors and `tracing` fields.
- `tocsin` command line: `tocsin [OPTIONS] [URL]...` with `--body`, `--title`,
  `--notification-type`, `--input-format`, `--dry-run`, standard input and
  `TOCSIN_URLS`. Exit status 0, 1 (delivery failed or nothing to send) or 2
  (usage error or invalid URL).
- Differential tests against Apprise 81739e9: nearly 400 URLs, every parsed
  field compared, with a script and CI job that regenerate the expected
  answers. Deliberate differences are recorded in the fixtures. A second test
  damages every fixture URL about 24,000 times in all and checks that parsing
  never panics.

### Known gaps

- Some paths need a second request whose answer the next one uses, and send
  nothing: Rocket.Chat with a user name and password, Mattermost bot channels
  written as names, Slack e-mail targets. Pushover end-to-end encryption is not
  implemented, and a URL that asks for it sends nothing.
- No attachments, body format conversion, retries or built-in async transport
  (the `http` feature hands requests to one).

[Unreleased]: https://github.com/zbl1998-sdjn/tocsin/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/zbl1998-sdjn/tocsin/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/zbl1998-sdjn/tocsin/releases/tag/v0.1.0
