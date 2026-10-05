//! Signal through signal-cli-rest-api:
//! `signal://[<user>[:<password>]@]<host>[:<port>]/<from number>[/<number or @group>...]`
//! and `signals://...` for HTTPS.
//!
//! The first path element, or `from=`, is the number the messages come from.
//! A target is a phone number of 10 to 14 digits, or a group (`@id` or
//! `group.id`), and without any the message goes to the sender. The server has
//! no title, so it goes in front of the body. With `batch=yes` ten recipients
//! share a request. A file is sent as base64 with the first part of a message.
//!
//! The way Apprise finds phone numbers in a text is reproduced, including that
//! it loses the numbers after one that is followed by something that is not a
//! number. Markdown is sent as text with `text_mode` set to `styled` and is not
//! converted to Signal's own dialect.

use std::{fmt::Write as _, sync::LazyLock};

use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;
use serde_json::json;

use crate::{
    Format, Kind, Notification, ParseError, PreparedRequest, SecretString, grammar, message,
    options::{FormatMode, Options},
};

/// A group, written `@id`, `%40id`, `group.id` or `@group.id`. Only the start
/// has to match.
static GROUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:(?:@|%40)?group\.|@|%40)(?P<group>[a-z0-9_=-]+)").expect("static regex")
});

/// How many recipients share a request with `batch=yes`.
const BATCH_SIZE: usize = 10;

/// A parsed `signal://` or `signals://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct SignalApi {
    /// `+` and the digits.
    source: SecretString,
    /// Numbers as `+` and digits, groups as `group.id`, in the order given and
    /// with duplicates; the source alone when the URL names none.
    targets: Vec<SecretString>,
    /// What was given as a target and is neither.
    invalid_targets: Vec<SecretString>,
    batch: bool,
    status: bool,
}

/// Apprise's `is_phone_no`: only digits, white space and `)(+-`, with 10 to 14
/// digits. The result is `+` and the digits.
fn phone_number(text: &str) -> Option<String> {
    let allowed = |c: char| c.is_ascii_digit() || grammar::is_python_space(c) || ")(+-".contains(c);
    if text.is_empty() || !text.chars().all(allowed) {
        return None;
    }
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    (10..=14)
        .contains(&digits.len())
        .then(|| format!("+{digits}"))
}

fn is_separator(c: char) -> bool {
    c == ',' || c == '+' || c == '(' || grammar::is_python_space(c)
}

/// Whether a number that ends at `end` is followed as Apprise's look-ahead
/// wants: by the end of the text (or a last line break), or by separators and
/// then a digit.
fn ends_a_number(chars: &[char], end: usize) -> bool {
    if end == chars.len() || (end + 1 == chars.len() && chars[end] == '\n') {
        return true;
    }
    let mut next = end;
    while chars.get(next).is_some_and(|c| is_separator(*c)) {
        next += 1;
    }
    next > end && chars.get(next).is_some_and(char::is_ascii_digit)
}

/// Where the number that starts at `start` ends, as the pattern
/// `((?:[+(][+(\s]{0,32})?[0-9][0-9()\s-]{1,32}[0-9])(?=$|[\s,+(]+[0-9])` finds
/// it: the longest match that the look-ahead accepts.
fn number_at(chars: &[char], start: usize) -> Option<usize> {
    let digit = |at: usize| chars.get(at).is_some_and(char::is_ascii_digit);
    let mut first = start;
    if matches!(chars.get(start), Some('+' | '(')) {
        // Nothing in the prefix is a digit, so only the longest prefix can be
        // followed by one.
        first += 1;
        while first < start + 33
            && chars
                .get(first)
                .is_some_and(|c| *c == '+' || *c == '(' || grammar::is_python_space(*c))
        {
            first += 1;
        }
    }
    if !digit(first) {
        return None;
    }
    let inner = chars[first + 1..]
        .iter()
        .take(32)
        .take_while(|c| {
            c.is_ascii_digit() || matches!(c, '(' | ')' | '-') || grammar::is_python_space(**c)
        })
        .count();
    (1..=inner).rev().find_map(|length| {
        let last = first + 1 + length;
        (digit(last) && ends_a_number(chars, last + 1)).then_some(last + 1)
    })
}

/// Apprise's `parse_phone_no` for one text: the numbers in it, or when there are
/// none, the text split at white space and `[ ] ; ,`.
fn phone_numbers(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if let Some(end) = number_at(&chars, start) {
            found.push(chars[start..end].iter().collect());
            start = end;
        } else {
            start += 1;
        }
    }
    if found.is_empty() {
        text.split(|c: char| matches!(c, '[' | ']' | ';' | ',') || grammar::is_python_space(c))
            .filter(|piece| !piece.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        found
    }
}

/// The targets of a URL: numbers, then groups, and what is neither.
fn sort_targets(elements: &[String]) -> (Vec<String>, Vec<String>) {
    let mut targets = Vec::new();
    let mut invalid_targets = Vec::new();
    for target in elements.iter().flat_map(|element| phone_numbers(element)) {
        if let Some(number) = phone_number(&target) {
            targets.push(number);
        } else if let Some(found) = GROUP.captures(&target) {
            targets.push(format!("group.{}", &found["group"]));
        } else {
            invalid_targets.push(target);
        }
    }
    (targets, invalid_targets)
}

pub(crate) fn parse(input: &str) -> Result<(Options, SignalApi), ParseError> {
    let raw = grammar::parse(input, grammar::Hosts::Any, FormatMode::Fixed(Format::Text))?;
    let mut elements = raw.paths.clone();
    // `from=` is the source, and then every path element is a target.
    let source = match raw.query.get("from").filter(|from| !from.is_empty()) {
        Some(from) => grammar::decode(from),
        None if elements.is_empty() => return Err(ParseError::InvalidOption),
        None => elements.remove(0),
    };
    let source = phone_number(&source).ok_or(ParseError::InvalidOption)?;
    if let Some(to) = raw.query.get("to") {
        elements.extend(phone_numbers(to));
    }

    let (mut targets, invalid_targets) = sort_targets(&elements);
    if elements.is_empty() {
        targets.push(source.clone());
    }
    let signal = SignalApi {
        source: SecretString::new(source),
        targets: targets.into_iter().map(SecretString::new).collect(),
        invalid_targets: invalid_targets.into_iter().map(SecretString::new).collect(),
        batch: raw.flag("batch", false),
        status: raw.flag("status", false),
    };
    Ok((raw.options, signal))
}

impl SignalApi {
    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        if self.targets.is_empty() {
            return Vec::new();
        }
        let mut url = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port {
            write!(url, ":{port}").expect("writing to a String works");
        }
        url.push_str("/v2/send");

        let format = message::format(options, notification);
        let text = message::merged(notification, format);
        let text_mode = if format == Format::Markdown {
            "styled"
        } else {
            "normal"
        };
        let status = if self.status {
            match notification.kind {
                Kind::Info => "[i] ",
                Kind::Success => "[+] ",
                Kind::Failure => "[!] ",
                Kind::Warning => "[~] ",
            }
        } else {
            ""
        };
        let files: Vec<String> = notification
            .carried(options.overflow)
            .iter()
            .map(|file| STANDARD.encode(file.data()))
            .collect();
        let batch_size = if self.batch { BATCH_SIZE } else { 1 };

        let mut requests = Vec::new();
        for (part, chunk) in message::chunks(&text, 32768, options.overflow)
            .iter()
            .enumerate()
        {
            let text = format!("{status}{chunk}");
            for recipients in self.targets.chunks(batch_size) {
                let mut payload = json!({
                    "message": text.trim_end(),
                    "number": self.source.expose(),
                    "text_mode": text_mode,
                    "recipients": recipients.iter().map(SecretString::expose).collect::<Vec<_>>(),
                });
                // The files go with the first part of a long message only.
                if part == 0 && !files.is_empty() {
                    payload["base64_attachments"] = json!(files);
                }
                requests.push(
                    PreparedRequest::json(&url, &payload)
                        .with_policy(options.policy())
                        .with_basic_auth(options),
                );
            }
        }
        requests
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        let list = |secrets: &[SecretString]| -> Vec<String> {
            secrets.iter().map(|s| s.expose().to_owned()).collect()
        };
        for (key, value) in [
            ("source", json!(self.source.expose())),
            ("targets", json!(list(&self.targets))),
            ("invalid_targets", json!(list(&self.invalid_targets))),
            ("batch", json!(self.batch)),
            ("status", json!(self.status)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}
