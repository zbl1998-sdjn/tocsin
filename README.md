# tocsin

Send one notification to many services from a single URL, in Rust. tocsin reads
the notification URLs of [Apprise](https://github.com/caronc/apprise)
(`tgram://...`, `discord://...`, `ntfy://...`) and turns them into HTTP
requests. A tocsin is an alarm bell.

**Status: version 0.3.** The library is on [crates.io](https://crates.io/crates/tocsin)
(`cargo add tocsin`) and its documentation is on [docs.rs](https://docs.rs/tocsin).
Twenty-six services work today: Telegram, Discord, ntfy, Gotify, Slack, Mattermost,
Rocket.Chat, Pushover, Pushbullet, Prowl, IFTTT, Zulip, PagerDuty, Google Chat,
Bark, Signal (through signal-cli-rest-api), Home Assistant, Feishu, Lark, WeCom,
ServerChan, DingTalk, Microsoft Teams (workflows), and a JSON, an XML and a form
webhook. Apprise has over a hundred, so check the list below before you depend on
this.

How far it has been checked against real servers (2026-10-05): the tests use a
local server and compare the URLs with Apprise. An opt-in test
(`cargo test --test live -- --ignored`) sends to `ntfy.sh` over TLS, with both
transports, and reads a message and a file back unchanged, and to `httpbin.org`,
which parses the JSON, form, multipart and XML bodies with its own HTTP stack.
Requests with deliberately fake credentials reached the real Telegram, Discord,
Slack, Pushover, Pushbullet, Prowl, IFTTT, PagerDuty, Google Chat, Zulip and Bark
(api.day.app) endpoints, which answered with credential errors only. Signal and
Home Assistant run on your own server, so they have only met the local test
server, and so have Feishu, Lark, WeCom, ServerChan and DingTalk. A delivery with real credentials has been verified for ntfy.sh and for
nothing else, so a service that needs an account may still have a difference
that only that account shows.

## Why it exists

If you already have Apprise URLs in a config file, a Docker label or a Python
tool, you can use the same strings from a Rust program or a single static
binary. The URL parser is checked against Apprise itself: the tests compare
every field of more than 1,000 URLs with what Apprise parses, so
`ntfys://user:pass@host/topic` means the same thing in both.

Four more things it tries to do well:

- **Secrets stay out of logs.** URLs, tokens, request and response bodies,
  attachments and message text print as `[REDACTED]` in `Debug` and `Display`,
  and errors never contain them.
- **Pure request building.** `Service::plan` hands out the HTTP requests one at a
  time without any I/O, so any client, blocking or async, can send them.
- **Files too.** Ten services take attachments, each the way its API wants
  them: base64 in a JSON body, a multipart upload, a raw body, an upload address.
- **Pay for what you use.** Each service is a Cargo feature, and the
  combinations of features are built and tested (no feature, each alone, every
  pair and all of them; CI repeats the same).

## Command line

Download a binary for Linux, macOS or Windows from the
[releases](https://github.com/zbl1998-sdjn/tocsin/releases/latest) (each has a
`.sha256` file), or build it:

```sh
cargo install tocsin-cli                 # installs `tocsin`

tocsin -b "Backup finished" -t nightly ntfy://my-topic
echo "disk is 95% full" | tocsin -n warning "tgram://<bot_token>/<chat_id>"
tocsin -b "Report attached" -a report.pdf "tgram://<bot_token>/<chat_id>"
TOCSIN_URLS="ntfy://a ntfy://b" tocsin -b "hello"      # URLs from the environment
tocsin --dry-run -b hi discord://<id>/<token>          # prints "POST https://discord.com"
```

Options: `-b/--body` (or standard input), `-t/--title`,
`-n/--notification-type` (`info`, `success`, `warning`, `failure`),
`-i/--input-format` (`text`, `markdown`, `html`), `-a/--attach FILE` (repeatable;
up to 50 MiB each, and a file can be the whole message), `-d/--dry-run`. Exit
status is 0 when every request was delivered, 1 when a delivery failed or a URL
had nothing to send to, and 2 for a usage error or an invalid URL. URLs given on
the command line are visible to other users of the machine; put URLs that hold
tokens in `TOCSIN_URLS`. A dry run reads no answers, so the requests that depend
on one (a login, a lookup, an upload) are not shown.

## From a coding agent

A coding agent that runs for minutes is a good reason to want a message on your
phone. Put your URLs in `TOCSIN_URLS` (for example `ntfy://my-topic`) in the
environment the agent starts in, and give its hook the command `tocsin --hook`.
It reads what the agent sends and builds the message: the title says what
happened and the body names the project.

Claude Code runs a command on `Stop` (it finished) and `Notification` (it needs
you) when `~/.claude/settings.json` has
([hooks reference](https://code.claude.com/docs/en/hooks)):

```json
{
  "hooks": {
    "Stop": [
      { "hooks": [{ "type": "command", "command": "tocsin", "args": ["--hook", "claude-code"] }] }
    ],
    "Notification": [
      { "hooks": [{ "type": "command", "command": "tocsin", "args": ["--hook", "claude-code"] }] }
    ]
  }
}
```

Codex CLI runs the program named by `notify` in `~/.codex/config.toml` with the
turn as one JSON argument added at the end
([configuration](https://learn.chatgpt.com/docs/config-file/config-advanced.md)):

```toml
notify = ["tocsin", "--hook", "codex"]
```

What the agent wrote last (`last_assistant_message`) stays out of the
notification, because it can hold code or secrets and a notification service is
not private; add `--include-message` to send the first 280 characters of it.
`-t`, `-b` and `-n` still override the title, body and kind. With `--hook` the
exit status is 0 or 1, never 2: in a Claude Code hook 2 blocks the action, and
`tocsin` returns it for a URL it cannot read when it is not run as a hook.

## Library

```rust
use tocsin::{Attachment, Notification, Notifier, UreqTransport};

let mut notifier = Notifier::new();
notifier.add("ntfy://my-topic")?;
notifier.add("discord://<webhook_id>/<webhook_token>")?;

let notification = Notification::new("Backup finished")
    .title("nightly")
    .attach(Attachment::new("backup.log", std::fs::read("backup.log")?));
let report = notifier.send(&notification, &mut UreqTransport);
if !report.is_success() {
    for failure in report.failures() {
        eprintln!("{}: {:?}", failure.service, failure.outcome);
    }
}
```

From async code, use `tocsin-reqwest` (`cargo add tocsin-reqwest`):

```rust
let mut transport = tocsin_reqwest::ReqwestTransport::new();
let report = notifier.send_async(&notification, &mut transport).await;
```

To use your own HTTP client, ask a `Plan` for the requests and tell it how each
one went. Some services need an answer before the next request (a login, a
channel lookup, an upload address), which is why a plan is not a list:

```rust
let service: tocsin::Service = "ntfys://user:password@ntfy.example.com/alerts".parse()?;
let mut plan = service.plan(&Notification::new("Disk almost full"));
while let Some(request) = plan.next_request() {
    // request.method, request.url.expose(), request.headers, request.body.expose()
    let result = my_client.send(&request);          // Result<Response, TransportError>
    plan.report(result);
}
let outcomes = plan.finish();
```

`Service::prepare` is the simpler form for the services that never need an
answer: it returns the requests as a list. The `http` feature turns each one
into an `http::Request`.

## Services

| URL | Service | Applied options | Read but not acted on |
|---|---|---|---|
| `tgram://<bot_token>/<chat_id>...` | Telegram | `format`, `silent`, `preview`, `mdv`, `topic` (or `thread`), `to`, `overflow`, `detect`, `content`, files | `image`, `album` |
| `discord://<id>/<token>`, the `discord.com/api/webhooks` address | Discord webhook | `format`, `tts`, `avatar_url`, `avatar`, `flags`, `thread`, `href`, `footer`, `botname`, `overflow`, `batch`, files | `image`, `fields`, `footer_logo`, `ping` |
| `ntfy://<topic>`, `ntfys://[user:pass@]host[:port]/<topic>`, `ntfy.sh` addresses | ntfy | `priority`, `click`, `delay`, `email`, `actions`, `tags`, `attach`, `filename`, `token`, `auth`, `mode`, `format=markdown`, `avatar_url`, `overflow`, files | |
| `gotify://host[:port][/path]/<token>`, `gotifys://` | Gotify | `priority`, `format=markdown`, `overflow` | |
| `slack://<a>/<b>/<c>/<channel>...`, `slack://xoxb-.../<channel or e-mail>...`, workflow and trigger URLs, the `hooks.slack.com` addresses | Slack | `mode`, `blocks`, `footer`, `to`, `token`, threads (`#channel:ts`), the user part as the bot name, e-mail targets and files (bot) | `image`, `timestamp`, `:token`, `template` (refused) |
| `mmost://[team@]host[:port][/path]/<token>`, `mmosts://`, `mattermost.*/hooks/` addresses | Mattermost | `mode` (`webhook`, `bot`), `to`/`channel`, `botname`/`team`, `icon_url`, channel names and files (bot) | `image` |
| `rocket://<id>/<token>@host/<target>...`, `rocket://user:<token>@host/...`, `rockets://` | Rocket.Chat | modes `webhook`, `token` and `basic` (logs in, posts, logs out), `to`, `webhook` | `avatar` |
| `pover://<user key>@<app token>/<device or #group>...` | Pushover | `priority`, `sound`, `url`, `url_title`, `interval`, `expire`, `to`, `format=html`, images | `key` and `e2ee`: a URL that asks for encryption sends nothing |
| `pbul://<access token>[/<device, #channel or e-mail>...]` | Pushbullet | `to`, files | |
| `prowl://<apikey>[/<providerkey>]` | Prowl | `priority` | |
| `ifttt://<webhook id>@<event>[/<event>...]`, `maker.ifttt.com/use` addresses | IFTTT | `to`, `+key=value`, `-key` | |
| `zulip://<bot>@<organization>/<token>[/<stream or e-mail>...]` | Zulip | `token`, `to` | |
| `pagerduty://<integration key>@<api key>[/<source>[/<component>]]` | PagerDuty | `region`, `severity`, `group`, `class`, `click`, `+detail=value`, `apikey`, `integrationkey`, `source`, `component` | `image` |
| `gchat://<workspace>/<key>/<token>[/<thread>]`, the `chat.googleapis.com` webhook address | Google Chat | `workspace`, `key`, `token`, `thread` (`threadKey`) | |
| `bark://[user:pass@]host[:port]/<device key>...`, `barks://` | Bark | `sound`, `level`, `badge`, `volume`, `call`, `group`, `category`, `click`, `icon`, `to`, `format=markdown`, `overflow` | `image`; `key`: a URL that asks for encryption sends nothing |
| `signal://[user:pass@]host[:port]/<from number>[/<number or @group>...]`, `signals://` | Signal (signal-cli-rest-api) | `from`, `to`, `batch`, `status`, `format=markdown` (sent as styled text), `overflow`, files | |
| `hassio://host[:port]/<token>[/<service>...]`, `hassios://` | Home Assistant | `token` (`accesstoken`), `to`, `prefix`, `nid`, `batch`, `overflow` | the user and password of the URL |
| `feishu://<token>` | Feishu | `token` | |
| `lark://<token>`, the `open.larksuite.com` bot address | Lark | `token` | |
| `wecombot://<key>`, the `qyapi.weixin.qq.com` webhook address | WeCom bot | `key` | |
| `schan://<token>` | ServerChan | | `token` is not read, as in Apprise |
| `dingtalk://[<secret>@]<token>[/<phone>...]` | DingTalk | `token`, `secret` (signs the request), `to`, `format=markdown` | |
| `workflows://host/<workflow id>/<signature>`, the Power Automate and Logic Apps addresses | Microsoft Teams | `pa`, `route`, `ver`, `wrap`, `id`, `sig` | `image`, `:token`, `template` (refused) |
| `json://host[:port][/path]`, `jsons://` | JSON webhook | `method`, `+header`, `-param`, `:body field`, user and password, files | |
| `xml://host[:port][/path]`, `xmls://` | XML webhook | `method`, `+header`, `-param`, `:element`, user and password, files | |
| `form://host[:port][/path]`, `forms://` | Form webhook | `method`, `+header`, `-param`, `:field`, `attach-as`, user and password, files | |

Options every URL accepts (`verify`, `cto`, `rto`, `redirect`, `overflow`,
`format`, ...) are applied; `retry`, `wait`, `optional`, `emojis`, `store` and
`tz` are checked like Apprise checks them and exposed, but not acted on. Body
format conversion (Markdown to HTML and back) and emoji shortcodes are not
supported. `DESIGN.md` lists every difference from Apprise.

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
`tocsin-reqwest` provides `ReqwestTransport` for async code, with the same
promises. With the `http` feature, a prepared request converts to an
`http::Request`, which `hyper` takes as it is. The `compat` feature is for tests
only.

## Security

See `SECURITY.md`. In short: do not enable `trace` logging for `ureq`, and keep
debug and trace logging off for `reqwest`, because they can log request URLs; and
keep URLs that hold tokens out of command lines.

## Minimum Rust version

Rust 1.85 (edition 2024). CI builds with it.

## License

MIT or Apache-2.0, at your option. See `NOTICE` for how Apprise's license
applies to the test fixtures.
