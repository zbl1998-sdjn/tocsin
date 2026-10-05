//! Differential tests against Apprise.
//!
//! `compat/oracle.json` holds what Apprise itself (commit 81739e9a) parses for
//! each URL in `compat/fixtures.json`. Regenerate it with
//! `python compat/apprise_oracle.py`, see `CONTRIBUTING.md`.
#![cfg(feature = "compat")]

use serde_json::Value;
use tocsin::Service;

/// Every service with fixtures, and whether this build has it.
const SERVICES: [(&str, bool); 15] = [
    ("telegram", cfg!(feature = "telegram")),
    ("discord", cfg!(feature = "discord")),
    ("ntfy", cfg!(feature = "ntfy")),
    ("gotify", cfg!(feature = "gotify")),
    ("json", cfg!(feature = "json")),
    ("workflows", cfg!(feature = "workflows")),
    ("form", cfg!(feature = "form")),
    ("mattermost", cfg!(feature = "mattermost")),
    ("rocketchat", cfg!(feature = "rocketchat")),
    ("pushover", cfg!(feature = "pushover")),
    ("slack", cfg!(feature = "slack")),
    ("prowl", cfg!(feature = "prowl")),
    ("ifttt", cfg!(feature = "ifttt")),
    ("pushbullet", cfg!(feature = "pushbullet")),
    ("zulip", cfg!(feature = "zulip")),
];

fn enabled(service: &str) -> bool {
    let known = SERVICES.iter().find(|(name, _)| *name == service);
    known
        .unwrap_or_else(|| panic!("fixtures for {service} are not in SERVICES"))
        .1
}

#[test]
fn every_field_of_every_fixture_agrees_with_apprise() {
    let oracle: Value =
        serde_json::from_str(include_str!("../compat/oracle.json")).expect("oracle JSON");
    let mut checked = 0;
    for row in oracle["rows"].as_array().expect("rows") {
        if !enabled(row["service"].as_str().expect("service")) {
            continue;
        }
        checked += 1;
        let id = &row["id"];
        let parsed = Service::parse(row["url"].as_str().expect("URL"));
        // A fixture can record a deliberate difference from Apprise. It must
        // say why, and tocsin must reject the URL that Apprise accepts.
        if let Some(reason) = row.get("divergence") {
            assert!(
                reason.as_str().is_some_and(|r| !r.is_empty()),
                "{id}: reason"
            );
            assert!(parsed.is_err(), "{id}: declared divergence, but it parses");
            assert!(
                row["valid"].as_bool().expect("validity"),
                "{id}: not a divergence"
            );
            continue;
        }
        assert_eq!(
            parsed.is_ok(),
            row["valid"].as_bool().expect("validity"),
            "{id}: validity"
        );
        if let Ok(parsed) = parsed {
            // Compare the whole record, including exact token values, without
            // printing values into test failures.
            let fields = parsed.compat_fields();
            let expected = row["fields"].as_object().expect("fields");
            let differences: Vec<_> = expected
                .iter()
                .filter(|(key, value)| fields.get(key.as_str()) != Some(*value))
                .map(|(key, _)| key.as_str())
                .collect();
            assert!(differences.is_empty(), "{id}: fields {differences:?}");
            assert_eq!(
                fields.as_object().expect("object").len(),
                expected.len(),
                "{id}: field set"
            );
        }
    }
    // The oracle must cover every fixture, and every service needs at least 30.
    let fixtures: Value =
        serde_json::from_str(include_str!("../compat/fixtures.json")).expect("fixtures JSON");
    let wanted: Vec<_> = fixtures
        .as_array()
        .expect("fixtures")
        .iter()
        .filter(|fixture| enabled(fixture["service"].as_str().expect("service")))
        .collect();
    assert_eq!(checked, wanted.len(), "the oracle is out of date");
    for (service, on) in SERVICES {
        let count = wanted.iter().filter(|f| f["service"] == service).count();
        assert!(!on || count >= 30, "{service} has only {count} fixtures");
    }
}

#[test]
fn arbitrary_input_never_panics() {
    for value in [
        "",
        "://",
        "tgrams://123:FAKE/1",
        "discords://FAKE/FAKE",
        "🦀://%FF",
        "tgram://1:",
        "ntfy://[broken",
        "tgram://\u{0}",
        "discord://%00/%00",
    ] {
        let result = std::panic::catch_unwind(|| Service::parse(value));
        assert!(result.is_ok(), "panicked on {value:?}");
    }
}
