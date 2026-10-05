# Security policy

## Reporting a vulnerability

Please report security problems privately, using GitHub's "Report a
vulnerability" button on the repository's Security tab. Do not open a public
issue for them. Say which version you tested and how to reproduce the problem.
You can expect an answer within a week.

## What counts

tocsin handles notification URLs, which hold bot tokens and webhook secrets.
These are in scope:

- A secret that reaches `Debug` or `Display` output, an error value, a log
  line or a panic message. Notification URLs, tokens, passwords, request and
  response bodies, attachments and the notification text are meant to stay out
  of all of them.
- A panic, an infinite loop or unbounded memory use on a hostile URL or
  message.
- A transport that disables TLS verification or follows a request to a host
  the URL did not name, without being asked to.
- A request that sends a secret somewhere the service's own documentation
  would not.

Not in scope: a service that rejects a valid request, or a difference from
Apprise that is listed in `DESIGN.md`.

## What tocsin promises about secrets

- `SecretString`, `SecretBytes` and `Attachment` print `[REDACTED]`. `Service`,
  `PreparedRequest`, `Response`, `Notification` and `Options` never print URLs,
  tokens, credentials, bodies, file names or message text.
- `ParseError` and `TransportError` carry no URL and no text from a server.
- `PreparedRequest::summary` is the one place that shows a host, for dry runs
  on your own terminal. It leaves out credentials, paths, queries, headers
  and the body.
- `UreqTransport` verifies TLS against the bundled web PKI roots and refuses a
  request that asks for `verify=no`. It honours `HTTP_PROXY`, `HTTPS_PROXY`
  and `ALL_PROXY`. `ReqwestTransport` (the `tocsin-reqwest` crate) refuses
  `verify=no` too and verifies against the platform's roots.
- A response body is kept for the sequences that need it, up to 1 MiB, and is
  never formatted. A token is not sent to an address that a service's own API
  returned (Slack's and Pushbullet's upload addresses).

## Two things to know

- `ureq` writes request URLs to the `log` crate at `trace` level, and tocsin
  URLs contain tokens. Do not enable `trace` logging for `ureq`, and keep the
  debug and trace levels off for `reqwest`, which can log URLs too (when it
  follows a redirect, for one).
- Command-line arguments are visible to other users of the machine. Put URLs
  that hold tokens in `TOCSIN_URLS` instead of on the command line.

## Supported versions

Only the latest release gets fixes while the version is below 1.0.
