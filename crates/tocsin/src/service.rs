//! A parsed notification URL.

use std::{fmt, str::FromStr};

use crate::{Notification, ParseError, Plan, PreparedRequest, options::Options};

/// The concrete services. Empty when no service feature is enabled.
#[derive(Clone)]
enum Inner {
    #[cfg(feature = "telegram")]
    Telegram(crate::services::telegram::Telegram),
    #[cfg(feature = "discord")]
    Discord(crate::services::discord::Discord),
    #[cfg(feature = "ntfy")]
    Ntfy(crate::services::ntfy::Ntfy),
    #[cfg(feature = "gotify")]
    Gotify(crate::services::gotify::Gotify),
    #[cfg(feature = "json")]
    Json(crate::services::json::Json),
    #[cfg(feature = "workflows")]
    Workflows(crate::services::workflows::Workflows),
    #[cfg(feature = "form")]
    Form(crate::services::form::Form),
    #[cfg(feature = "mattermost")]
    Mattermost(crate::services::mattermost::Mattermost),
    #[cfg(feature = "rocketchat")]
    RocketChat(crate::services::rocketchat::RocketChat),
    #[cfg(feature = "pushover")]
    Pushover(crate::services::pushover::Pushover),
    #[cfg(feature = "slack")]
    Slack(crate::services::slack::Slack),
}

/// One notification destination, parsed from an Apprise-compatible URL such as
/// `tgram://<bot_token>/<chat_id>`.
///
/// Parsing and [`prepare`](Self::prepare) perform no I/O. The URL holds
/// credentials, so [`Debug`] and [`Display`](fmt::Display) show only the
/// service name.
#[derive(Clone)]
pub struct Service {
    options: Options,
    inner: Inner,
}

impl Service {
    /// Parse a notification URL.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] that never contains the URL itself.
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        #[cfg(not(feature = "_services"))]
        {
            let _ = input;
            Err(ParseError::UnsupportedService)
        }
        #[cfg(feature = "_services")]
        {
            let (url, scheme) = crate::grammar::prepare_url(input)?;
            match Self::from_scheme(&scheme, &url) {
                // Apprise reads the web address of a service, such as a
                // webhook URL, only when the scheme is not one of its own.
                Err(ParseError::UnsupportedService) => {
                    let rewritten =
                        crate::native::rewrite(&url).ok_or(ParseError::UnsupportedService)?;
                    let scheme = rewritten
                        .split("://")
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    Self::from_scheme(&scheme, &rewritten)
                }
                parsed => parsed,
            }
        }
    }

    #[cfg(feature = "_services")]
    fn from_scheme(scheme: &str, input: &str) -> Result<Self, ParseError> {
        match scheme {
            #[cfg(feature = "telegram")]
            "tgram" => crate::services::telegram::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Telegram(service))),
            #[cfg(feature = "discord")]
            "discord" => crate::services::discord::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Discord(service))),
            #[cfg(feature = "ntfy")]
            "ntfy" | "ntfys" => crate::services::ntfy::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Ntfy(service))),
            #[cfg(feature = "gotify")]
            "gotify" | "gotifys" => crate::services::gotify::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Gotify(service))),
            #[cfg(feature = "json")]
            "json" | "jsons" => crate::services::json::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Json(service))),
            #[cfg(feature = "workflows")]
            "workflow" | "workflows" => crate::services::workflows::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Workflows(service))),
            #[cfg(feature = "form")]
            "form" | "forms" => crate::services::form::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Form(service))),
            #[cfg(feature = "mattermost")]
            "mmost" | "mmosts" => crate::services::mattermost::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Mattermost(service))),
            #[cfg(feature = "rocketchat")]
            "rocket" | "rockets" => crate::services::rocketchat::parse(input)
                .map(|(options, service)| Self::new(options, Inner::RocketChat(service))),
            #[cfg(feature = "pushover")]
            "pover" => crate::services::pushover::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Pushover(service))),
            #[cfg(feature = "slack")]
            "slack" => crate::services::slack::parse(input)
                .map(|(options, service)| Self::new(options, Inner::Slack(service))),
            _ => Err(ParseError::UnsupportedService),
        }
    }

    #[cfg(feature = "_services")]
    fn new(options: Options, inner: Inner) -> Self {
        Self { options, inner }
    }

    /// The service name, such as `"telegram"`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.inner {
            #[cfg(feature = "telegram")]
            Inner::Telegram(_) => "telegram",
            #[cfg(feature = "discord")]
            Inner::Discord(_) => "discord",
            #[cfg(feature = "ntfy")]
            Inner::Ntfy(_) => "ntfy",
            #[cfg(feature = "gotify")]
            Inner::Gotify(_) => "gotify",
            #[cfg(feature = "json")]
            Inner::Json(_) => "json",
            #[cfg(feature = "workflows")]
            Inner::Workflows(_) => "workflows",
            #[cfg(feature = "form")]
            Inner::Form(_) => "form",
            #[cfg(feature = "mattermost")]
            Inner::Mattermost(_) => "mattermost",
            #[cfg(feature = "rocketchat")]
            Inner::RocketChat(_) => "rocketchat",
            #[cfg(feature = "pushover")]
            Inner::Pushover(_) => "pushover",
            #[cfg(feature = "slack")]
            Inner::Slack(_) => "slack",
        }
    }

    /// The options shared by every service URL.
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Build the HTTP requests that deliver `notification` and that can be sent
    /// without reading an answer first.
    ///
    /// The list is empty when the URL is valid but there is nothing to send to,
    /// for example a Telegram URL without a chat id. It is also empty when the
    /// delivery needs an answer to come back first, such as a login. Use
    /// [`plan`](Self::plan) for those.
    #[must_use]
    #[cfg_attr(
        not(feature = "_services"),
        allow(unused_variables, reason = "no service exists to read it")
    )]
    pub fn prepare(&self, notification: &Notification) -> Vec<PreparedRequest> {
        // Matching the place (not a reference) keeps the zero-service build
        // exhaustive: `Inner` has no variants there.
        match self.inner {
            #[cfg(feature = "telegram")]
            Inner::Telegram(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "discord")]
            Inner::Discord(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "ntfy")]
            Inner::Ntfy(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "gotify")]
            Inner::Gotify(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "json")]
            Inner::Json(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "workflows")]
            Inner::Workflows(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "form")]
            Inner::Form(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "mattermost")]
            Inner::Mattermost(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "rocketchat")]
            Inner::RocketChat(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "pushover")]
            Inner::Pushover(ref service) => service.prepare(&self.options, notification),
            #[cfg(feature = "slack")]
            Inner::Slack(ref service) => service.prepare(&self.options, notification),
        }
    }

    /// Everything it takes to deliver `notification`, including the requests
    /// that depend on the answer to an earlier one. See [`Plan`].
    #[must_use]
    #[cfg_attr(
        not(feature = "_services"),
        allow(unused_variables, reason = "no service exists to read it")
    )]
    pub fn plan(&self, notification: &Notification) -> Plan {
        match self.inner {
            #[cfg(feature = "mattermost")]
            Inner::Mattermost(ref service) => service.plan(&self.options, notification),
            #[cfg(feature = "rocketchat")]
            Inner::RocketChat(ref service) => service.plan(&self.options, notification),
            #[allow(unreachable_patterns, reason = "every service may need a plan")]
            _ => Plan::requests(self.prepare(notification)),
        }
    }

    /// Every normalized field of the parsed URL, secrets included. This exists
    /// for the differential tests against Apprise. Never log the result.
    #[cfg(feature = "compat")]
    #[cfg_attr(
        not(feature = "_services"),
        allow(unreachable_code, reason = "no service exists to describe")
    )]
    #[doc(hidden)]
    #[must_use]
    pub fn compat_fields(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        self.options.compat(&mut map);
        map.insert("service".to_owned(), serde_json::json!(self.name()));
        match self.inner {
            #[cfg(feature = "telegram")]
            Inner::Telegram(ref service) => service.compat(&mut map),
            #[cfg(feature = "discord")]
            Inner::Discord(ref service) => service.compat(&mut map),
            #[cfg(feature = "ntfy")]
            Inner::Ntfy(ref service) => service.compat(&mut map),
            #[cfg(feature = "gotify")]
            Inner::Gotify(ref service) => service.compat(&mut map),
            #[cfg(feature = "json")]
            Inner::Json(ref service) => service.compat(&mut map),
            #[cfg(feature = "workflows")]
            Inner::Workflows(ref service) => service.compat(&mut map),
            #[cfg(feature = "form")]
            Inner::Form(ref service) => service.compat(&mut map),
            #[cfg(feature = "mattermost")]
            Inner::Mattermost(ref service) => service.compat(&mut map),
            #[cfg(feature = "rocketchat")]
            Inner::RocketChat(ref service) => service.compat(&mut map),
            #[cfg(feature = "pushover")]
            Inner::Pushover(ref service) => service.compat(&mut map),
            #[cfg(feature = "slack")]
            Inner::Slack(ref service) => service.compat(&mut map),
        }
        serde_json::Value::Object(map)
    }
}

impl FromStr for Service {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Debug for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Service({}, [REDACTED])", self.name())
    }
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [REDACTED]", self.name())
    }
}
