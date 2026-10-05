# Design

tocsin turns a notification URL such as `tgram://<bot>/<chat>` into the HTTP
requests that deliver a message, and optionally sends them. This file explains
the choices that are not obvious from the code, and lists where tocsin differs
from Apprise.

## Goals and non-goals

Goals:

- Read the URLs people already have for [Apprise](https://github.com/caronc/apprise)
  and get the same parse, field for field.
- Keep secrets out of logs, errors and `Debug` output by construction.
- Let each service be an optional feature, so a build only carries what it uses.
- Make the request-building step pure, so an async application can use its own
  HTTP client.

Non-goals, for now: attachments, body format conversion, e-mail and other
non-HTTP services, retries, and an async transport.

## How the pieces fit

```
URL ──Service::parse──▶ Service ──prepare(&Notification)──▶ Vec<PreparedRequest>
                                                                 │
                       Notifier (many services) ── Transport::send ◀┘
```

- `Service::parse` reads a URL into a typed per-service struct. It does no I/O.
- `Service::prepare` builds the requests for one notification. It does no I/O
  either, and returns an empty list when the URL is valid but has nothing to
  send to (a Telegram URL without a chat id).
- `Transport` sends one `PreparedRequest`. `UreqTransport` is a blocking
  implementation, `MockTransport` records requests for tests, and a caller can
  implement the trait for any other client.
- `Notifier` holds several services, sends to all of them, never stops at the
  first failure, and returns a `Report` with one `Receipt` per request. An empty
  notifier or a service with nothing to send is not a success.

Each service has its own struct instead of a bag of key-value pairs. A field
that does not exist for a service cannot be read by mistake, and the compiler
tells you when one is unused.

A `prepare` is one step: it builds the requests that need no answer. A service
whose protocol needs an answer before the next request (Rocket.Chat's login,
Mattermost's channel lookup, Slack's e-mail lookup) builds a `Plan` instead,
through `Service::plan`. A plan hands out one request, takes the result, and
decides what comes next, without sending anything itself. `Notifier::send`,
`Notifier::send_async` and an application that drives a plan with its own client
all use it, so the logic exists once. Inside a sequence every request has a
role: a delivery (its result is a receipt), a setup such as a login or lookup
(only a failure is a receipt), or a cleanup such as a logout (never reported).
A failed delivery does not keep the other deliveries or the logout from going
out, as in Apprise.

## URL grammar parity

Apprise's own parser is the reference. `crates/tocsin/compat` holds:

- `fixtures.json`: URLs, at least 30 per service. They come from Apprise's test
  suite or were written to reach a branch of its parsing; each says which file
  and lines it exercises.
- `oracle.json`: what Apprise (commit `81739e9`) parses for each URL, about 30
  normalized fields per URL, including the exact token values.
- `apprise_oracle.py`: the script that produces it. It instantiates each URL
  and never sends anything; a Python audit hook turns any socket use into an
  error.

`tests/compat.rs` (feature `compat`) compares every field of every fixture, and
CI regenerates the oracle from the pinned Apprise commit and fails if the file
differs. A fixture that no service claims fails the test instead of being
skipped, and so does an oracle that lacks a fixture.

The comparison is only as good as the fixtures. To check that it can fail, the
tests were run against deliberately broken code (a wrong default, a wrong
priority letter, the old path splitting); each change was caught and named the
fixture. Adding fixtures that probe the URL itself rather than one service found
real differences more than once.

The grammar code mirrors Apprise's own steps, including its odd ones:

- Before it picks a plugin, Apprise turns every `/#` into `/%23`, accepts
  whitespace in front of the scheme, and treats all slashes after `scheme:` as
  its separator, so `ntfys:///topic` is `ntfys://topic`.
- A path is tidied, decoded once, quoted again and then split, so `%2F`
  separates segments while a space or comma stays inside one. `;params` after
  the last slash are dropped as `urlparse` does, and a URL that ends in a slash
  gets one on its path even when the slash belongs to the query.
- A host is checked with Apprise's `is_hostname` for the services that verify
  it (a trailing dot is removed, IPv6 comes back in brackets), and a port that
  is not a number makes the URL invalid there but part of the host elsewhere.
- Query keys are read the way `parse_qsd` reads them, including the `+`, `-` and
  `:` prefixes.
- A web address a service shows you, such as a Slack or Discord webhook, is
  turned into the notification URL by the same rule Apprise uses, and only when
  the scheme is not one tocsin knows.

A fixture can also record a deliberate difference with a `divergence` field. It
must say why, and tocsin must reject a URL that Apprise accepts. The test checks
both. Today that is the `template=` option of Teams and Slack, which points at a
file that Apprise reads while it parses.

## Secrets

Everything that can hold a secret is a `SecretString`, whose `Debug` and
`Display` print `[REDACTED]`: the URL and header values and body of a
`PreparedRequest`, a parsed password or token, and the notification's own text.
`Service` prints only its name. Errors say what kind of thing was wrong and
nothing else; they never carry the URL or text from a server. Tests format the
public types with fake secrets and fail if one shows up, including a test that
records them as `tracing` event fields, which is how an application would log
them.

The one deliberate exception is `PreparedRequest::summary`, for `--dry-run`.

## Dependencies

- `serde_json` (with `arbitrary_precision`, so a Telegram chat id of any length
  stays exact), `percent-encoding`, and `regex` or `base64` only for the services
  that need them.
- `ureq` 3 with rustls (`ring`), behind the `ureq` feature. TLS roots come from
  the bundled web PKI set, not from the operating system.
- No `log` level hack. Capping `log` with `max_level_off` would apply to every
  crate in the build through feature unification, so tocsin documents the
  `ureq` trace-logging caveat instead.

## Where tocsin differs from Apprise

The request layer follows each provider's documentation, not Apprise's habits.
These are the differences, so you can judge them.

Everywhere:

- tocsin does not convert a body between text, Markdown and HTML, interpolate
  emoji, or send attachments. It picks the format the service supports and
  sends the text as given. Slack and Pushover would need their own dialects
  (`mrkdwn`, a small set of HTML tags), so Markdown goes out as written.
- Apprise links pictures that live in its own repository (a Teams card image,
  a Mattermost icon, a Slack icon, a Rocket.Chat avatar). tocsin sends none, so
  `image`, `avatar` and the like are read and reported only. The same goes for
  the Slack `timestamp`.
- Where Apprise uses its own name as a default (a bot name, a footer), tocsin
  uses `tocsin`.
- The `+key=value` (header), `-key=value` (query parameter) and `:key=value`
  (body) arguments are applied by the JSON and form webhooks only. The other
  services parse them, and Teams and Slack report `:key=value` as template
  tokens, but nothing uses them.
- `retry`, `wait`, `optional`, `emojis`, `store` and `tz` are read and checked
  like Apprise does, and exposed on `Options`, but not acted on.
- Apprise's Windows rules for backslashes in paths are not reproduced.
- When a time zone alias is ambiguous Apprise picks one from an unordered set;
  tocsin rejects the URL.
- `UreqTransport` refuses `verify=no` instead of turning verification off.

By service:

- Telegram: `image`, `album` and `detect` are ignored; with no chat id there is
  nothing to send, where Apprise would ask the bot for its last chat.
- Discord: `wait` is a query parameter, as in Discord's API, not a body field.
  Apprise's internal `allow_mentions` is not sent. `image`, `fields`,
  `footer_logo`, `ping` and `batch` are ignored.
- ntfy: `tags`, `attach` and `filename` are ignored.
- Gotify: an empty title is left out instead of sent as `""`.
- JSON and form webhooks: with a user but no password, Apprise's HTTP library
  sends the text `None` as the password; tocsin sends an empty one.
  `UreqTransport` cannot send the unregistered method `UPDATE` (the URL still
  parses) and returns `TransportError::UnsupportedMethod`. Form `attach-as` is
  read only.
- Teams (workflows) and Slack: the `template=` option, a file of cards or blocks,
  is refused. Slack e-mail targets need a lookup request and are skipped.
- Mattermost: in bot mode a channel written as a name is looked up in the team
  first, once for each name and before the first post (Apprise interleaves the
  lookups with the posts and caches them). A name that cannot be resolved is a
  failure of its own; the other channels still get the message.
- Rocket.Chat: the `basic` mode (user and password) logs in, posts to every
  target and logs out, once for every piece of a long message, as Apprise does.
  A login answer without a user id and a token is a failure; Apprise sends the
  posts without the headers and lets the server refuse them.
- Pushover: a URL with `key=` asks for end-to-end encryption, which tocsin does
  not do; it sends nothing, where Apprise falls back to plain text when it has no
  crypto library. The `Authorization` header is left out because the token is
  already in the body, and `device` is left out when the message goes to every
  device instead of sending the word `ALL_DEVICES`.

## Feature hygiene

A helper that only some services use is dead code in a build without them, and
`-D warnings` turns that into an error. The earlier prototype failed 10 of 16
feature combinations for this reason. Every service feature is compiled with
clippy and tested with no other feature, with each other feature, and with all
of them; CI does the same with `cargo hack --depth 2`. The minimum supported
Rust version is 1.85 (edition 2024) and CI builds with it.
