# tocsin

Send one notification to many services from a single URL, in Rust. tocsin reads
the notification URLs of [Apprise](https://github.com/caronc/apprise)
(`tgram://...`, `discord://...`, `ntfy://...`) and turns them into HTTP
requests. A tocsin is an alarm bell.

**Status: version 0.1.** The library is on [crates.io](https://crates.io/crates/tocsin)
(`cargo add tocsin`) and its documentation is on [docs.rs](https://docs.rs/tocsin).
Eleven services work today: Telegram, Discord, ntfy, Gotify, Slack, Mattermost,
Rocket.Chat, Pushover, Microsoft Teams (workflows), and a JSON and a form
webhook. Apprise has over a hundred, so check the list below before you depend
on this. Nothing has been sent to the real services in anyone's production yet:
the tests use a local server and compare the URLs with Apprise.

## Why it exists

If you already have Apprise URLs in a config file, a Docker label or a Python
tool, you can use the same strings from a Rust program or a single static
binary. The URL parser is checked against Apprise itself: the tests compare
every field of nearly 400 URLs with what Apprise parses, so
`ntfys://user:pass@host/topic` means the same thing in both.

Three more things it tries to do well:

- **Secrets stay out of logs.** URLs, tokens, request bodies and message text
  print as `[REDACTED]` in `Debug` and `Display`, and errors never contain them.
- **Pure request building.** `Service::prepare` returns the HTTP requests
  without any I/O, so an async program can send them with its own client.
- **Pay for what you use.** Each service is a Cargo feature, and the
  combinations of features are built and tested (no feature, each alone, every
  pair and all of them; CI repeats the same).

## Command line

The command line is not on crates.io yet; build it from a clone:

```sh
cargo install --path crates/tocsin-cli     # installs `tocsin`

tocsin -b "Backup finished" -t nightly ntfy://my-topic
echo "disk is 95% full" | tocsin -n warning "tgram://<bot_token>/<chat_id>"
TOCSIN_URLS="ntfy://a ntfy://b" tocsin -b "hello"      # URLs from the environment
tocsin --dry-run -b hi discord://<id>/<token>          # prints "POST https://discord.com"
```

Options: `-b/--body` (or standard input), `-t/--title`,
`-n/--notification-type` (`info`, `success`, `warning`, `failure`),
`-i/--input-format` (`text`, `markdown`, `html`), `-d/--dry-run`. Exit status
is 0 when every request was delivered, 1 when a delivery failed or a URL had
nothing to send to, and 2 for a usage error or an invalid URL. URLs given on the
command line are visible to other users of the machine; put URLs that hold
tokens in `TOCSIN_URLS`.

## Library

```rust
use tocsin::{Notification, Notifier, UreqTransport};

let mut notifier = Notifier::new();
notifier.add("ntfy://my-topic")?;
notifier.add("discord://<webhook_id>/<webhook_token>")?;

let notification = Notification::new("Backup finished").title("nightly");
let report = notifier.send(&notification, &mut UreqTransport);
if !report.is_success() {
    for failure in report.failures() {
        eprintln!("{}: {:?}", failure.service, failure.outcome);
    }
}
```

To use your own HTTP client, build the requests and send them yourself (the `http`
feature turns each one into an `http::Request`):

```rust
let service: tocsin::Service = "ntfys://user:password@ntfy.example.com/alerts".parse()?;
for request in service.prepare(&Notification::new("Disk almost full")) {
    // request.method, request.url.expose(), request.headers, request.body.expose()
}
```

## Services

| URL | Service | Applied options | Read but not acted on |
|---|---|---|---|
| `tgram://<bot_token>/<chat_id>...` | Telegram | `format`, `silent`, `preview`, `mdv`, `topic` (or `thread`), `to`, `overflow` | `image`, `album`, `detect` |
| `discord://<id>/<token>`, the `discord.com/api/webhooks` address | Discord webhook | `format`, `tts`, `avatar_url`, `avatar`, `flags`, `thread`, `href`, `footer`, `botname`, `overflow` | `image`, `fields`, `footer_logo`, `ping`, `batch` |
| `ntfy://<topic>`, `ntfys://[user:pass@]host[:port]/<topic>`, `ntfy.sh` addresses | ntfy | `priority`, `click`, `delay`, `email`, `actions`, `token`, `auth`, `mode`, `format=markdown`, `avatar_url`, `overflow` | `tags`, `attach`, `filename` |
| `gotify://host[:port][/path]/<token>`, `gotifys://` | Gotify | `priority`, `format=markdown`, `overflow` | |
| `slack://<a>/<b>/<c>/<channel>...`, `slack://xoxb-.../<channel>`, workflow and trigger URLs, the `hooks.slack.com` addresses | Slack | `mode`, `blocks`, `footer`, `to`, `token`, threads (`#channel:ts`), the user part as the bot name | `image`, `timestamp`, `:token`, `template` (refused), e-mail targets (skipped) |
| `mmost://[team@]host[:port][/path]/<token>`, `mmosts://`, `mattermost.*/hooks/` addresses | Mattermost | `mode` (`webhook`, `bot`), `to`/`channel`, `botname`/`team`, `icon_url` | `image`, bot-mode channel names (skipped) |
| `rocket://<id>/<token>@host/<target>...`, `rocket://user:<token>@host/...`, `rockets://` | Rocket.Chat | modes `webhook` and `token`, `to`, `webhook` | `avatar`; the `basic` mode sends nothing (it needs a login first) |
| `pover://<user key>@<app token>/<device or #group>...` | Pushover | `priority`, `sound`, `url`, `url_title`, `interval`, `expire`, `to`, `format=html` | `key` and `e2ee`: a URL that asks for encryption sends nothing |
| `workflows://host/<workflow id>/<signature>`, the Power Automate and Logic Apps addresses | Microsoft Teams | `pa`, `route`, `ver`, `wrap`, `id`, `sig` | `image`, `:token`, `template` (refused) |
| `json://host[:port][/path]`, `jsons://` | JSON webhook | `method`, `+header`, `-param`, `:body field`, user and password | |
| `form://host[:port][/path]`, `forms://` | Form webhook | `method`, `+header`, `-param`, `:field`, user and password | `attach-as` |

Options every URL accepts (`verify`, `cto`, `rto`, `redirect`, `overflow`,
`format`, ...) are applied; `retry`, `wait`, `optional`, `emojis`, `store` and
`tz` are checked like Apprise checks them and exposed, but not acted on.
Attachments, body format conversion (Markdown to HTML and back) and emoji
shortcodes are not supported. `DESIGN.md` lists every difference from Apprise.

## How it compares

These Rust crates cover similar ground. Each uses its own configuration instead
of Apprise URLs. This is from reading their source on 2026-10-04 and may have
changed:

- **chatterbox**: 14 services configured with typed values, all compiled into
  every build, and messages go through a background queue.
- **downlink**: 11 channels, one feature each; its repository description
  mentions unified URL schemas, but I could not confirm that they match
  Apprise's.
- **shoutrrr** (Go) and **shoutrrr-rs**: shoutrrr's own URL syntax, which differs
  from Apprise's (the Discord and Telegram URLs are not the same).

If you only need one service, its own crate is likely smaller. tocsin is for
the case where the URL is the configuration.

## Features

`default = ["all-services", "ureq"]`. Pick services one by one with
`default-features = false, features = ["ntfy", "ureq"]`. `ureq` provides
`UreqTransport`, a blocking client using rustls with the bundled web PKI roots;
it honours `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` and refuses `verify=no`.
With the `http` feature, a prepared request converts to an `http::Request`, which `hyper`
takes as it is and `reqwest` takes through `reqwest::Request::try_from`. The `compat` feature
is for tests only.

## Security

See `SECURITY.md`. In short: do not enable `trace` logging for `ureq`, because it
logs request URLs, and keep URLs that hold tokens out of command lines.

## Minimum Rust version

Rust 1.85 (edition 2024). CI builds with it.

## License

MIT or Apache-2.0, at your option. See `NOTICE` for how Apprise's license
applies to the test fixtures.
