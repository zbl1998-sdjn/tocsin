//! `PagerDuty`: `pagerduty://<integration key>@<api key>[/<source>[/<component>]]`.
//!
//! Every message triggers an event through the Events API v2. `PagerDuty` has no
//! title, so the title goes in front of the text, and the severity follows the
//! kind of notification unless `severity=` sets it. `+key=value` adds a custom
//! detail.

use std::collections::BTreeMap;

use serde_json::json;

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// The severities, in the order Apprise looks a prefix up in.
const SEVERITIES: [&str; 4] = ["info", "warning", "critical", "error"];

/// What Apprise says when the URL names none.
#[cfg(feature = "compat")]
const DEFAULT_SOURCE: &str = "Apprise";
const DEFAULT_COMPONENT: &str = "Notification";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Us,
    Eu,
}

impl Region {
    #[cfg(feature = "compat")]
    fn as_str(self) -> &'static str {
        match self {
            Self::Us => "us",
            Self::Eu => "eu",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Self::Us => "https://events.pagerduty.com/v2/enqueue",
            Self::Eu => "https://events.eu.pagerduty.com/v2/enqueue",
        }
    }
}

/// A parsed `pagerduty://` URL.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct PagerDuty {
    api_key: SecretString,
    integration_key: SecretString,
    /// `None` for Apprise's own default.
    source: Option<String>,
    component: Option<String>,
    region: Region,
    /// The severity the URL forces, if it does.
    severity: Option<&'static str>,
    click: Option<String>,
    class: Option<String>,
    group: Option<String>,
    image: bool,
    details: BTreeMap<String, String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, PagerDuty), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Optional,
        FormatMode::Fixed(Format::Text),
    )?;
    let option = |key: &str| {
        raw.query
            .get(key)
            .filter(|value| !value.is_empty())
            .map(|value| grammar::decode(value))
    };
    let options = &raw.options;
    let api_key = grammar::token(&option("apikey").unwrap_or_else(|| options.host.clone()))
        .ok_or(ParseError::InvalidToken)?;
    let integration_key = grammar::token(
        &option("integrationkey")
            .or_else(|| options.user.clone())
            .unwrap_or_default(),
    )
    .ok_or(ParseError::InvalidToken)?;

    // The source and the component are the options, or else the first two
    // elements of the path.
    let mut path = raw.paths.clone().into_iter();
    let source = option("source").or_else(|| path.next());
    let component = option("component").or_else(|| path.next());
    let checked = |value: Option<String>| {
        value
            .map(|text| grammar::token(&text).ok_or(ParseError::InvalidToken))
            .transpose()
    };
    let (source, component) = (checked(source)?, checked(component)?);

    let region = match option("region")
        .map(|region| region.to_lowercase())
        .as_deref()
    {
        None | Some("us") => Region::Us,
        Some("eu") => Region::Eu,
        Some(_) => return Err(ParseError::InvalidOption),
    };
    // Apprise looks for a severity that starts with the text as it was
    // written, so `Critical` finds none.
    let severity = match option("severity") {
        None => None,
        Some(wanted) => Some(
            SEVERITIES
                .into_iter()
                .find(|severity| severity.starts_with(wanted.as_str()))
                .ok_or(ParseError::InvalidOption)?,
        ),
    };
    let mut details = BTreeMap::new();
    for (key, value) in raw.headers.iter() {
        details.insert(grammar::decode(key), grammar::decode(value));
    }
    let pagerduty = PagerDuty {
        api_key: SecretString::new(api_key),
        integration_key: SecretString::new(integration_key),
        source,
        component,
        region,
        severity,
        click: option("click"),
        class: option("class"),
        group: option("group"),
        image: raw.flag("image", true),
        details,
    };
    Ok((raw.options, pagerduty))
}

impl PagerDuty {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let severity = self.severity.unwrap_or(match notification.kind {
            crate::Kind::Info | crate::Kind::Success => "info",
            crate::Kind::Warning => "warning",
            crate::Kind::Failure => "critical",
        });
        message::chunks(&text, 32768, options.overflow)
            .into_iter()
            .map(|summary| {
                let mut event = json!({
                    "summary": summary,
                    "severity": severity,
                    "source": self.source.as_deref().unwrap_or("tocsin"),
                    "component": self.component.as_deref().unwrap_or(DEFAULT_COMPONENT),
                });
                if let Some(group) = &self.group {
                    event["group"] = json!(group);
                }
                if let Some(class) = &self.class {
                    event["class"] = json!(class);
                }
                if !self.details.is_empty() {
                    event["custom_details"] = json!(self.details);
                }
                let mut payload = json!({
                    "routing_key": self.integration_key.expose(),
                    "payload": event,
                    "client": "tocsin",
                    "event_action": "trigger",
                });
                if let Some(click) = &self.click {
                    payload["links"] = json!([{"href": click}]);
                }
                let mut request = PreparedRequest::json(self.region.url(), &payload)
                    .with_policy(options.policy());
                request.headers.insert(
                    "Authorization".to_owned(),
                    SecretString::new(format!("Token token={}", self.api_key.expose())),
                );
                request
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        for (key, value) in [
            ("apikey", json!(self.api_key.expose())),
            ("integration_key", json!(self.integration_key.expose())),
            (
                "source",
                json!(self.source.as_deref().unwrap_or(DEFAULT_SOURCE)),
            ),
            (
                "component",
                json!(self.component.as_deref().unwrap_or(DEFAULT_COMPONENT)),
            ),
            ("region_name", json!(self.region.as_str())),
            ("severity", json!(self.severity)),
            ("click", json!(self.click)),
            ("class_id", json!(self.class)),
            ("group", json!(self.group)),
            ("include_image", json!(self.image)),
            ("details", json!(self.details)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
