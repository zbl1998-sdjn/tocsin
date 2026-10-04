# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/) once it reaches 1.0.

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
- The minimum supported Rust version, 1.85, is not yet checked by a build.

[Unreleased]: https://github.com/zbl1998-sdjn/tocsin/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zbl1998-sdjn/tocsin/releases/tag/v0.1.0
