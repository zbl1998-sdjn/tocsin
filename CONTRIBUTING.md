# Contributing

Thanks for helping. The most useful contribution is a new service, and the
steps are below. For anything bigger than a bug fix or a service, open an issue
first so we can agree on the shape.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Every service is a feature, so also check the builds that leave one out. The
features that start with an underscore are internal helpers that a service
switches on, so they are left out of the combinations:

```sh
cargo install cargo-hack
internal=$(sed -n 's/^\(_[a-z_]*\) *=.*/\1/p' crates/tocsin/Cargo.toml | paste -sd, -)
cargo hack clippy -p tocsin --all-targets --no-default-features \
  --feature-powerset --depth 2 \
  --exclude-features all-services,compat,$internal -- -D warnings
cargo hack test -p tocsin --no-default-features \
  --feature-powerset --depth 2 \
  --exclude-features all-services,compat,$internal
```

`TOCSIN_FUZZ_ROUNDS=4000 cargo test -p tocsin --all-features --test robustness`
damages each fixture URL that many times and checks that nothing panics; the
default is 60.

Tests never touch the network: parsing and request building are pure, the CLI
tests use `--dry-run` or a server on `127.0.0.1`, and fixtures use fake secrets
that start with `FAKE_`.

## Adding a service

Start from `src/services/gotify.rs`, the smallest one, or from
`src/services/json.rs` if the service takes a body of its own.

1. **Read the service's API documentation and Apprise's plugin**
   (`apprise/plugins/<name>.py`, including `parse_url`, `__init__`, `send`, and
   `parse_native_url` if there is one). The URL grammar must match Apprise's; the
   request must match the provider's documentation. Write down any difference
   in `DESIGN.md`.
2. **Add the feature** in `crates/tocsin/Cargo.toml`: `<name> = ["_services", ...]`
   followed by the shared helpers it uses (`_list`, `_flag`, `_token`, `_verify`,
   `_format`, `_parts` and the rest, each explained next to its definition) and
   any optional dependency. Add it to `all-services`.
3. **Write `src/services/<name>.rs`** with a `pub(crate) fn parse(input: &str)
   -> Result<(Options, Service), ParseError>`, a `prepare(&self, &Options,
   &Notification) -> Vec<PreparedRequest>` method and, when a request needs the
   answer to an earlier one (a login, a lookup, an upload address), a
   `plan(&self, &Options, &Notification) -> Plan` method built from the
   sequences in `src/plan.rs` (`Lookups` for the common case of looking names
   up before sending, see `mattermost.rs`). Add, behind
   `#[cfg(feature = "compat")]`, a `compat(&self, &mut Map)` method that lists
   the parsed fields. Wrap tokens and anything else secret in `SecretString`.
   For files, `notification.carried(options.overflow)` is what to send, with the
   first part of the message only, and `multipart.rs` writes a multipart body.
   `grammar::parse` takes how the host is checked (`Hosts::Verified` for a real
   host name, `Any` for an id or token in its place, `Optional` when it may be
   missing); use the one Apprise's plugin uses (`verify_host`).
4. **Register it**: `src/services/mod.rs`, the `Inner` enum and the scheme match
   in `src/service.rs` (`name`, `prepare` and `compat_fields` too, and `plan` if
   the service has one). If the
   plugin has a `parse_native_url`, add the same rewrite to `src/native.rs`.
5. **Add at least 30 fixtures** to `crates/tocsin/compat/fixtures.json` with a
   short id prefix (`gf01`...). Take the URLs from
   `apprise/tests/test_plugin_<name>.py` and add some that reach each parsing
   branch: bad input, odd paths, every value of an enum option. Give each a
   `source` with the Apprise file and lines. Add the service to `SERVICES` in
   `crates/tocsin/tests/compat.rs`. If you decide tocsin should refuse a URL
   Apprise accepts, say so with a `divergence` field that gives the reason; the
   test then expects tocsin to reject it.
6. **Teach the oracle** the service: a class name and a field list in
   `normalize()` in `apprise_oracle.py`. The field names must match the ones
   your `compat` method writes.
7. **Regenerate the oracle** (you need the pinned Apprise checkout and its
   Python dependencies, see the docstring of `apprise_oracle.py`):

   ```sh
   git clone https://github.com/caronc/apprise ../apprise
   git -C ../apprise checkout 81739e9a1187f986a09281b5d0c25c806e8152e0
   pip install requests requests-oauthlib PyYAML markdown click certifi
   python crates/tocsin/compat/apprise_oracle.py --apprise-dir ../apprise
   cargo test -p tocsin --all-features --test compat
   ```

   When the test disagrees, the message names the fixture and the fields.
   Apprise is right; change the Rust.
8. **Add golden tests** in `crates/tocsin/tests/requests.rs`: the exact URL,
   headers and body of a request, written from the provider's documentation,
   and the new service's URL in `credentials_in_urls_never_show_up_in_formatting`.
   Tests of files go in `tests/attachments.rs` and tests of services that need
   answers in `tests/plans.rs`, which has a scripted transport.
9. **Run the feature checks above.** A helper that no service of a build uses is
   dead code there; declare it in the service's feature list rather than
   silencing the warning.

## Style

- Match the surrounding code. Comments say why, not what.
- Errors must not carry the URL, a token or text from a server.
- A parsed option that is not acted on gets a line in `DESIGN.md`.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org).
