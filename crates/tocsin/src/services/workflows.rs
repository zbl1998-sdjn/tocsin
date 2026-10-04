//! Microsoft Teams through Power Automate workflows:
//! `workflows://<host>[:<port>]/<workflow id>/<signature>`.
//!
//! The message goes out as an Adaptive Card. Apprise also accepts a card
//! `template` file; tocsin does not read files while parsing, so a URL with
//! `template=` is rejected.

use std::{collections::BTreeMap, fmt::Write as _, sync::LazyLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Format, Notification, ParseError, PreparedRequest, SecretString,
    grammar::{self, form_encode},
    message,
    options::{FormatMode, Options},
};

static WORKFLOW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[A-Z0-9_-]+$").expect("static regex"));
static SIGNATURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z0-9_-]+$").expect("static regex"));
static ROUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+$").expect("static regex"));
/// A Teams mention, `<at>name</at>`. The pattern stops at a nested tag so that
/// a valid inner mention survives a broken outer one.
static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<at>([^<]+)</at>").expect("static regex"));

const WORKFLOW_API: &str = "2016-06-01";
const POWER_AUTOMATE_API: &str = "2022-03-01-preview";

/// A parsed `workflow://` or `workflows://` URL.
///
/// `image` is parsed and reported, but no image is added to the card: Apprise
/// points it at a picture in its own repository.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Workflows {
    workflow: SecretString,
    signature: SecretString,
    image: bool,
    power_automate: bool,
    routing_id: Option<String>,
    wrap: bool,
    api_version: String,
    tokens: BTreeMap<String, String>,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Workflows), ParseError> {
    let raw = grammar::parse(
        input,
        grammar::Hosts::Verified,
        FormatMode::Fixed(Format::Markdown),
    )?;
    // Apprise decodes these values a second time.
    let argument = |key: &str| {
        raw.query
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| grammar::decode(s))
    };
    let mut entries = raw.paths.iter().map(|s| grammar::decode(s));
    let workflow = argument("workflow")
        .or_else(|| argument("id"))
        .or_else(|| entries.next());
    let signature = argument("signature")
        .or_else(|| argument("sig"))
        .or_else(|| entries.next());
    let workflow = workflow
        .and_then(|value| grammar::validate(&WORKFLOW, &value))
        .ok_or(ParseError::InvalidToken)?;
    let signature = signature
        .and_then(|value| grammar::validate(&SIGNATURE, &value))
        .ok_or(ParseError::InvalidToken)?;
    let routing_id = argument("route")
        .or_else(|| argument("routeid"))
        .map(|value| grammar::validate(&ROUTE, &value).ok_or(ParseError::InvalidOption))
        .transpose()?;
    if raw.query.get("template").is_some_and(|s| !s.is_empty()) {
        return Err(ParseError::InvalidOption);
    }
    let flag = |keys: &[&str], default: bool| {
        keys.iter()
            .find(|key| raw.query.contains_key(**key))
            .map_or(default, |key| raw.flag(key, default))
    };
    let power_automate = flag(&["powerautomate", "pa"], false);
    let api_version = argument("api-version")
        .or_else(|| argument("ver"))
        .unwrap_or_else(|| {
            if power_automate {
                POWER_AUTOMATE_API
            } else {
                WORKFLOW_API
            }
            .to_owned()
        });
    let workflows = Workflows {
        workflow: SecretString::new(workflow),
        signature: SecretString::new(signature),
        image: raw.flag("image", true),
        power_automate,
        routing_id,
        wrap: raw.flag("wrap", true),
        api_version,
        tokens: raw.payload.to_map(),
    };
    Ok((raw.options, workflows))
}

impl Workflows {
    fn card(&self, title: &str, body: &str) -> Value {
        let mut content = Vec::new();
        if !title.is_empty() {
            content.push(json!({
                "type": "TextBlock",
                "text": title,
                "style": "heading",
                "weight": "Bolder",
                "size": "Large",
                "id": "title",
            }));
        }
        content.push(json!({
            "type": "TextBlock",
            "text": body,
            "style": "default",
            "wrap": self.wrap,
            "id": "body",
        }));
        // A card needs an entity for every mention it contains.
        let mut mentions: Vec<Value> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for found in MENTION.captures_iter(body) {
            let name = found[1].trim().to_owned();
            if name.is_empty() || seen.contains(&name) {
                continue;
            }
            mentions.push(json!({
                "type": "mention",
                "text": &found[0],
                "mentioned": {"id": name, "name": name},
            }));
            seen.push(name);
        }
        let mut teams = json!({"width": "full"});
        if !mentions.is_empty() {
            teams["entities"] = Value::Array(mentions);
        }
        json!({
            "type": "message",
            "attachments": [{
                "contentType": "application/vnd.microsoft.card.adaptive",
                "contentUrl": null,
                "content": {
                    "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                    "type": "AdaptiveCard",
                    "version": "1.4",
                    "body": content,
                    "msteams": teams,
                },
            }],
        })
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        let mut url = format!("https://{}", options.host);
        if let Some(port) = options.port {
            write!(url, ":{port}").expect("writing to a String works");
        }
        if self.power_automate {
            url.push_str("/powerautomate/automations/direct");
            if let Some(routing_id) = &self.routing_id {
                write!(url, "/cu/{routing_id}").expect("writing to a String works");
            }
        }
        write!(
            url,
            "/workflows/{}/triggers/manual/paths/invoke?api-version={}&sp={}&sv=1.0&sig={}",
            self.workflow.expose(),
            form_encode(&self.api_version),
            form_encode("/triggers/manual/run"),
            form_encode(self.signature.expose()),
        )
        .expect("writing to a String works");
        message::parts(notification, 250, 1000, options.overflow)
            .into_iter()
            .map(|(title, body)| {
                PreparedRequest::json(&url, &self.card(&title, &body)).with_policy(options.policy())
            })
            .collect()
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
        for (key, value) in [
            ("workflow", json!(self.workflow.expose())),
            ("signature", json!(self.signature.expose())),
            ("image", json!(self.image)),
            ("power_automate", json!(self.power_automate)),
            ("routing_id", json!(self.routing_id)),
            ("wrap", json!(self.wrap)),
            ("api_version", json!(self.api_version)),
            ("tokens", json!(self.tokens)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
