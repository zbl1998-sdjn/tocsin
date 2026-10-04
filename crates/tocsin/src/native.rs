//! The web addresses that services give you, such as
//! `https://hooks.slack.com/services/T/B/C`, rewritten into the notification
//! URL Apprise would turn them into.
//!
//! Apprise tries these only when the scheme is not one of its own, and so does
//! tocsin. Each rewrite follows the plugin's `parse_native_url`.

#[cfg(any(
    feature = "discord",
    feature = "ntfy",
    feature = "workflows",
    feature = "mattermost",
    feature = "slack"
))]
use std::sync::LazyLock;

#[cfg(any(
    feature = "discord",
    feature = "ntfy",
    feature = "workflows",
    feature = "mattermost",
    feature = "slack"
))]
use regex::Regex;

#[cfg(feature = "discord")]
static DISCORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^https?://discord(?:app)?\.com/api/webhooks/(?P<id>[0-9]+)/(?P<token>[A-Z0-9_-]+)/?(?P<params>\?.+)?$",
    )
    .expect("static regex")
});
#[cfg(feature = "ntfy")]
static NTFY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:http|ntfy)s?://ntfy\.sh(?P<topics>/[^?]+)?(?P<params>\?.+)?$")
        .expect("static regex")
});
#[cfg(feature = "workflows")]
static WORKFLOWS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^https?://(?P<host>[A-Z0-9_.-]+)(?P<port>:[1-9][0-9]{0,5})?(?P<pa>/powerautomate/automations/direct(?:/cu/(?P<route>[0-9]+))?)?/workflows/(?P<workflow>[A-Z0-9_-]+)/triggers/manual/paths/invoke/?(?P<params>\?.+)$",
    )
    .expect("static regex")
});
#[cfg(feature = "mattermost")]
static MATTERMOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^http(?P<secure>s?)://(?P<host>mattermost\.[A-Z0-9_.-]+)(?::(?P<port>[1-9][0-9]{0,5}))?/hooks/(?P<token>[A-Z0-9_-]+)/?(?P<params>\?.+)?$",
    )
    .expect("static regex")
});
#[cfg(feature = "slack")]
static SLACK_WORKFLOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^https?://hooks\.slack\.com/(?P<kind>workflows|triggers)/(?P<path>[A-Z0-9][A-Z0-9/_-]+)/?(?P<params>\?.+)?$",
    )
    .expect("static regex")
});
#[cfg(feature = "slack")]
static SLACK_HOOK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^https?://hooks\.slack(?P<gov>-gov)?\.com/services/(?P<a>[A-Z0-9]+)/(?P<b>[A-Z0-9]+)/(?P<c>[A-Z0-9]+)/?(?P<params>\?.+)?$",
    )
    .expect("static regex")
});

/// The notification URL for a web address, if a service claims it.
#[allow(
    unused_variables,
    reason = "without a service that has web addresses there is nothing to match"
)]
pub(crate) fn rewrite(url: &str) -> Option<String> {
    #[cfg(feature = "discord")]
    if let Some(found) = DISCORD.captures(url) {
        return Some(format!(
            "discord://{}/{}/{}",
            &found["id"],
            &found["token"],
            found.name("params").map_or("", |m| m.as_str())
        ));
    }
    #[cfg(feature = "ntfy")]
    if let Some(found) = NTFY.captures(url) {
        let topics = found.name("topics").map_or("", |m| m.as_str());
        let params = match found.name("params") {
            None => "?mode=cloud".to_owned(),
            Some(params) => format!("{}&mode=cloud", params.as_str()),
        };
        return Some(format!("ntfys://{topics}{params}"));
    }
    #[cfg(feature = "workflows")]
    if let Some(found) = WORKFLOWS.captures(url) {
        let mut params = found["params"].to_owned();
        if found.name("pa").is_some() {
            params.push_str("&pa=yes");
        }
        // Apprise sets the routing id after parsing, so it beats the query.
        if let Some(route) = found.name("route") {
            params.push_str("&route=");
            params.push_str(route.as_str());
        }
        return Some(format!(
            "workflow://{}{}/{}/{params}",
            &found["host"],
            found.name("port").map_or("", |m| m.as_str()),
            &found["workflow"],
        ));
    }
    #[cfg(feature = "mattermost")]
    if let Some(found) = MATTERMOST.captures(url) {
        // Apprise drops a port that is written in the address.
        return Some(format!(
            "mmost{}://{}/{}/{}",
            &found["secure"],
            &found["host"],
            &found["token"],
            found.name("params").map_or("", |m| m.as_str())
        ));
    }
    #[cfg(feature = "slack")]
    {
        if let Some(found) = SLACK_WORKFLOW.captures(url) {
            let mode = if found["kind"].eq_ignore_ascii_case("triggers") {
                "trigger"
            } else {
                "workflow"
            };
            let params = found.name("params").map_or("", |m| m.as_str());
            let separator = if params.is_empty() { '?' } else { '&' };
            return Some(format!(
                "slack://{}/{params}{separator}mode={mode}",
                found["path"].trim_matches('/'),
            ));
        }
        if let Some(found) = SLACK_HOOK.captures(url) {
            let mut params = found.name("params").map_or("", |m| m.as_str()).to_owned();
            if found.name("gov").is_some() {
                // Apprise replaces the query with this, which drops it.
                params = format!("{}mode=gov-hook", if params.is_empty() { '?' } else { '&' });
            }
            return Some(format!(
                "slack://{}/{}/{}/{params}",
                &found["a"], &found["b"], &found["c"]
            ));
        }
    }
    None
}
