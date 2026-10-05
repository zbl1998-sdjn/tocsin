# tocsin-cli

The command line of [tocsin](https://github.com/zbl1998-sdjn/tocsin): send one
notification to many services from [Apprise](https://github.com/caronc/apprise)
notification URLs. It installs a program called `tocsin`.

```sh
cargo install tocsin-cli

tocsin -b "Backup finished" -t nightly ntfy://my-topic
echo "disk is 95% full" | tocsin -n warning "tgram://<bot_token>/<chat_id>"
tocsin -b "Report attached" -a report.pdf "tgram://<bot_token>/<chat_id>"
TOCSIN_URLS="ntfy://a ntfy://b" tocsin -b "hello"      # URLs from the environment
tocsin --dry-run -b hi discord://<id>/<token>          # prints "POST https://discord.com"
```

Options: `-b/--body` (or standard input), `-t/--title`, `-n/--notification-type`
(`info`, `success`, `warning`, `failure`), `-i/--input-format` (`text`,
`markdown`, `html`), `-a/--attach FILE` (repeatable, up to 50 MiB each; a file
can be the whole message), `-d/--dry-run`. Exit status is 0 when every request
was delivered, 1 when a delivery failed or a URL had nothing to send to, and 2
for a usage error or an invalid URL. A dry run reads no answers, so the requests
that depend on one (a login, a lookup, an upload) are not shown.

URLs given on the command line are visible to other users of the machine; put
URLs that hold tokens in `TOCSIN_URLS`.

Eighteen services are supported: Telegram, Discord, ntfy, Gotify, Slack,
Mattermost, Rocket.Chat, Pushover, Pushbullet, Prowl, IFTTT, Zulip, PagerDuty,
Google Chat, Microsoft Teams (workflows), and a JSON, an XML and a form webhook.
See the [repository](https://github.com/zbl1998-sdjn/tocsin) for
the options each one applies and how tocsin differs from Apprise.

Licensed under MIT or Apache-2.0.
