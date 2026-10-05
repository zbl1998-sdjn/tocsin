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
- Make the request-building step pure, so any HTTP client, blocking or async, can
  send the requests.

Non-goals, for now: body format conversion, e-mail and other non-HTTP services,
and retries.

## How the pieces fit

```
URL ──Service::parse──▶ Service ──plan(&Notification)──▶ Plan
                                                          │ next_request()
                        Notifier ── Transport::send ◀─────┤
                                    AsyncTransport::send  │ report(result)
                                                          ▼
                                                    Vec<Outcome>
```

- `Service::parse` reads a URL into a typed per-service struct. It does no I/O.
- `Service::prepare` builds the requests for one notification that need no answer
  first. It does no I/O either, and returns an empty list when the URL is valid
  but has nothing to send to (a Mattermost bot with no channel, a Pushover URL
  that asks for encryption).
- `Service::plan` is `prepare` plus everything that depends on an answer. A
  `Plan` is a small state machine: it hands out one request, takes the result of
  sending it, and decides what comes next. It sends nothing itself.
- `Transport` sends one `PreparedRequest` and blocks; `AsyncTransport` does the
  same without blocking. `UreqTransport` (in `tocsin`) and `ReqwestTransport` (in
  `tocsin-reqwest`) are the two real ones, `MockTransport` records requests for
  tests and implements both traits, and a caller can implement either trait for
  any other client.
- `Notifier` holds several services and sends to all of them, with `send` or
  `send_async`. It never stops at the first failure, and returns a `Report` with
  one `Receipt` per delivery. An empty notifier, or a service with nothing to
  send to, is not a success. The services are taken one after the other; to send
  to several at once, give each its own `Notifier` and join the futures.

Each service has its own struct instead of a bag of key-value pairs. A field
that does not exist for a service cannot be read by mistake, and the compiler
tells you when one is unused.

### Plans and sequences

Most services need nothing but `prepare`. Some need an answer before the next
request: Rocket.Chat's login, a Mattermost channel name or Slack e-mail address
that has to be looked up, Telegram's `getUpdates` for a chat id, a file that is
uploaded before a message points at it. Those build their `Plan` out of
sequences. Inside a sequence every request has a role:

- a **delivery**: its result is a receipt;
- a **setup** (a login, a lookup, an upload address): only a failure is a
  receipt;
- a **cleanup** (a logout): never reported.

A sequence sees failures as well as answers, so a failed delivery does not keep
the others, or the logout, from going out, as in Apprise. A sequence can also
end by handing the plan more sequences, which is how the posts to channels that
were just looked up are planned. Because a plan only asks for a request when the
last one is reported, the same code drives the blocking and the async notifier,
and an application that sends with its own client.

A request can carry a check on its answer. Slack answers `200` to a message it
did not accept, so a plan reads the body as Apprise does: a webhook has to
answer `ok`, the Web API has to say `"ok": true`. Anything else is
`TransportError::Rejected`, where a client that reads only the status would call
the message delivered.

### Attachments

`Notification::attach` takes an `Attachment`: a name, a media type (guessed from
the extension) and the bytes, held in an `Arc` so cloning is cheap. Each service
sends it the way Apprise does, and all of them send it with the first part of a
long message only (Telegram puts the files with the last part when the text
comes first). `overflow=truncate` keeps only the first file. A file a service
would refuse (Pushover's images over 5 MiB, Telegram's files over 50 MB) is a
failure in the report, not silence. `multipart/form-data` bodies are laid out
the way Python's `requests` writes them, with a boundary that does not occur in
the content and that is the same for the same input.

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
- Some services read a value twice: once when the query string is decoded, and
  again in the plugin. tocsin decodes twice exactly where Apprise does, and the
  fixtures include a `%25` in those places.

A fixture can also record a deliberate difference with a `divergence` field. It
must say why, and tocsin must reject a URL that Apprise accepts. The test checks
both. Today that is the `template=` option of Teams and Slack, which points at a
file that Apprise reads while it parses.

A different kind of difference is not a rejection: the `+key` and `-key` of an
IFTTT URL never reach Apprise's plugin (`parse_url` returns them as `add_token`
and `del_token`, and `__init__` reads `add_tokens` and `del_tokens`), so at the
pinned commit they do nothing. tocsin applies them as documented, and the oracle
leaves those two fields out of what it compares.

## Secrets

Everything that can hold a secret redacts itself in `Debug` and `Display`: a
`SecretString` (the URL and header values of a `PreparedRequest`, a parsed
password or token, the notification's own text), a `SecretBytes` (request and
response bodies), and an `Attachment` (its name and its bytes). `Service` prints
only its name. Errors say what kind of thing was wrong and nothing else; they
never carry the URL or text from a server. Tests format the public types with
fake secrets and fail if one shows up, including a test that records them as
`tracing` event fields, which is how an application would log them.

A response body is kept (up to 1 MiB) because the next request of a sequence can
depend on it, and because services sometimes echo what they were sent. It is a
`SecretBytes`, so formatting a response never shows it.

The one deliberate exception is `PreparedRequest::summary`, for `--dry-run`.

## Dependencies

- `serde_json` (with `arbitrary_precision`, so a Telegram chat id of any length
  stays exact), `percent-encoding`, and `regex` or `base64` only for the services
  that need them.
- `ureq` 3 with rustls (`ring`), behind the `ureq` feature. TLS roots come from
  the bundled web PKI set, not from the operating system.
- `tocsin-reqwest` is a separate crate, so `tocsin` itself stays free of an async
  runtime and of reqwest's TLS choice.
- No `log` level hack. Capping `log` with `max_level_off` would apply to every
  crate in the build through feature unification, so tocsin documents the
  `ureq` trace-logging caveat instead.

## Where tocsin differs from Apprise

The request layer follows each provider's documentation, not Apprise's habits.
These are the differences, so you can judge them.

Everywhere:

- tocsin does not convert a body between text, Markdown and HTML, or interpolate
  emoji. It picks the format the service supports and sends the text as given.
  Slack, Pushover and Google Chat would need their own dialects (`mrkdwn`, a
  small set of HTML tags), so Markdown goes out as written.
- Apprise links pictures that live in its own repository (a Teams card image,
  a Mattermost icon, a Slack icon, a Rocket.Chat avatar). tocsin sends none, so
  `image`, `avatar` and the like are read and reported only. The same goes for
  the Slack `timestamp`.
- Where Apprise uses its own name as a default (a bot name, a footer, the
  `application` of Prowl, the `client` of PagerDuty), tocsin uses `tocsin`.
- The `+key=value` (header), `-key=value` (query parameter) and `:key=value`
  (body) arguments are applied by the JSON, form and XML webhooks. IFTTT applies
  `+` and `-`, and PagerDuty applies `+`. The other services parse them, and
  Teams and Slack report `:key=value` as template tokens, but nothing uses them.
- `retry`, `wait`, `optional`, `emojis`, `store` and `tz` are read and checked
  like Apprise does, and exposed on `Options`, but not acted on. In particular
  nothing is remembered between runs (Apprise's persistent store caches a
  Telegram owner and the channel ids that Mattermost looks up).
- Apprise's Windows rules for backslashes in paths are not reproduced.
- When a time zone alias is ambiguous Apprise picks one from an unordered set;
  tocsin rejects the URL.
- `UreqTransport` and `ReqwestTransport` refuse `verify=no` instead of turning
  verification off.
- When a service asks for a token to be sent to an address that its own API
  returned (Slack's and Pushbullet's upload addresses), tocsin leaves the token
  out; Apprise sends it.
- A dry run of the command line reads no answers, so the requests that depend on
  one are not shown, and that is not a failure.

By service:

- Telegram: `image` and `album` are ignored. With no chat id (and `detect` on)
  the message goes to the sender of the first update `getUpdates` returns, as in
  Apprise. Files are uploaded with `sendPhoto`, `sendVideo`, `sendAudio`,
  `sendVoice`, `sendAnimation` or `sendDocument`; the text is their caption when
  it is under 1024 characters, as in Apprise, and every file is a message of its
  own. A photo over 10 MB goes as a document and a file over 50 MB is refused.
  The boolean `show_caption_above_media` is sent as `true` or `false`, where
  Apprise's HTTP library writes `True` or `False`.
- Discord: `wait` is a query parameter, as in Discord's API, not a body field,
  and the post of files always waits. Apprise's internal `allow_mentions` is not
  sent. `image`, `fields`, `footer_logo` and `ping` are ignored. Files go after
  the message, up to 10 or 25 MiB to a message (`batch=no`: one each), with the
  webhook's fields in a `payload_json` part.
- ntfy: `tags` (`X-Tags`), `attach` and `filename` are sent. A message with files
  is sent the other way ntfy knows, each file as the body of a request to the
  topic, with the text in the query of the first. Apprise's rule that turns a
  JSON message of more than 8000 characters into a text attachment is not
  implemented.
- Gotify: an empty title is left out instead of sent as `""`.
- JSON, form and XML webhooks: with a user but no password, Apprise's HTTP
  library sends the text `None` as the password; tocsin sends an empty one.
  `UreqTransport` cannot send the unregistered method `UPDATE` (the URL still
  parses) and returns `TransportError::UnsupportedMethod`; `ReqwestTransport`
  sends it. Files go as base64 in the JSON and XML bodies, and as
  `multipart/form-data` with `attach-as` naming the fields in the form body.
  Apprise parses `-key` for the XML webhook but never sends it; tocsin adds it to
  the query string.
- Teams (workflows) and Slack: the `template=` option, a file of cards or blocks,
  is refused.
- Slack: a bot resolves an e-mail address target with `users.lookupByEmail`
  first, once for each address and before the first message (Apprise caches it).
  A target that cannot be used (an address without a bot, or something that is no
  channel name) is a failure in the report, as in Apprise, not silence. The Slack
  e-mail address pattern is Apprise's `GET_EMAIL_RE`. A bot sends files in three
  steps (upload address, upload, share into every channel that the messages
  went to); a file that fails does not keep the others, where Apprise stops.
- Mattermost: in bot mode a channel written as a name is looked up in the team
  first, once for each name and before the first post (Apprise interleaves the
  lookups with the posts and caches them). A name that cannot be resolved is a
  failure of its own; the other channels still get the message. A bot uploads
  each file to each channel and names them in the post; a channel whose upload
  failed gets no post.
- Rocket.Chat: the `basic` mode (user and password) logs in, posts to every
  target and logs out, once for every piece of a long message, as Apprise does.
  A login answer without a user id and a token is a failure; Apprise sends the
  posts without the headers and lets the server refuse them.
- Pushover: a URL with `key=` asks for end-to-end encryption, which tocsin does
  not do; it sends nothing, where Apprise falls back to plain text when it has no
  crypto library. The `Authorization` header is left out because the token is
  already in the body, and `device` is left out when the message goes to every
  device instead of sending the word `ALL_DEVICES`. Every image (up to 5 MiB) is
  a message of its own; a file that is no image is not sent (only its message),
  and an image that is empty or too large is a failure.
- Prowl: the `application` is `tocsin`.
- IFTTT: see above for `+key` and `-key`.
- Pushbullet: files are uploaded first (an upload request, then the file) and
  pushed after the note to each target; nothing is sent when an upload fails.
- Zulip, PagerDuty, Google Chat: as Apprise, with the markdown and image
  differences above.

## Feature hygiene

A helper that only some services use is dead code in a build without them, and
`-D warnings` turns that into an error. The earlier prototype failed 10 of 16
feature combinations for this reason. Every service feature is compiled with
clippy and tested with no other feature, with each other feature, and with all
of them (`--depth 2`); CI does the same with `cargo hack`. Each service lists the
shared helpers it uses as internal features (`_token`, `_sequence`, `_multipart`
and so on), so a helper exists exactly when a service that uses it does. The
minimum supported Rust version is 1.85 (edition 2024) and CI builds with it.
