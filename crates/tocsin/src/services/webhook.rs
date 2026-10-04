//! What the JSON and form webhooks share: the HTTP method, the path as it was
//! written, and the arguments that change the request. `+key=value` adds a
//! header, `-key=value` adds a query parameter and `:key=value` changes the
//! body.

use std::fmt::Write as _;

use base64::{Engine, engine::general_purpose::STANDARD};

use crate::{
    Method, ParseError, PreparedRequest, SecretString,
    grammar::{self, Pairs, Raw},
    options::Options,
};

/// The methods Apprise accepts.
fn method_from(name: &str) -> Result<Method, ParseError> {
    Ok(match name {
        "POST" => Method::Post,
        "GET" => Method::Get,
        "DELETE" => Method::Delete,
        "PUT" => Method::Put,
        "HEAD" => Method::Head,
        "PATCH" => Method::Patch,
        "UPDATE" => Method::Update,
        "OPTIONS" => Method::Options,
        _ => return Err(ParseError::InvalidOption),
    })
}

/// Apprise decodes the keys and values of these arguments a second time.
fn decoded(pairs: &Pairs) -> Pairs {
    let mut decoded = Pairs::default();
    for (key, value) in pairs.iter() {
        decoded.set(&grammar::decode(key), &grammar::decode(value));
    }
    decoded
}

#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Webhook {
    pub(crate) method: Method,
    /// The path as written, quoted, or empty.
    pub(crate) path: String,
    pub(crate) headers: Pairs,
    pub(crate) params: Pairs,
    pub(crate) payload: Pairs,
}

impl Webhook {
    pub(crate) fn from_raw(raw: &Raw) -> Result<Self, ParseError> {
        let method = match raw.query.get("method").filter(|s| !s.is_empty()) {
            Some(name) => method_from(&grammar::decode(name).to_uppercase())?,
            None => Method::Post,
        };
        Ok(Self {
            method,
            path: raw.fullpath.clone(),
            headers: decoded(&raw.headers),
            params: decoded(&raw.params),
            payload: decoded(&raw.payload),
        })
    }

    /// `scheme://host[:port]` and the path as written.
    pub(crate) fn origin(&self, options: &Options) -> String {
        let mut url = format!(
            "{}://{}",
            if options.secure { "https" } else { "http" },
            options.host
        );
        if let Some(port) = options.port {
            write!(url, ":{port}").expect("writing to a String works");
        }
        url.push_str(&self.path);
        url
    }

    /// Add the headers set with `+`, and the credentials of the URL.
    ///
    /// A header replaces the default of the same name whatever its case, and
    /// the credentials replace any `Authorization` header. Where Apprise sends
    /// the text `None` as the password of a URL that only has a user, this
    /// sends an empty password.
    pub(crate) fn finish(
        &self,
        mut request: PreparedRequest,
        options: &Options,
    ) -> PreparedRequest {
        request.method = self.method;
        request.policy = options.policy();
        for (name, value) in self.headers.iter() {
            set_header(&mut request, name, value.to_owned());
        }
        if let Some(user) = options.user.as_deref().filter(|u| !u.is_empty()) {
            let password = options.password.as_ref().map_or("", SecretString::expose);
            set_header(
                &mut request,
                "Authorization",
                format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
            );
        }
        request
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, serde_json::Value>) {
        use serde_json::json;

        for (key, value) in [
            ("method", json!(self.method.as_str())),
            ("path", json!(self.path)),
            ("headers", json!(map_of(&self.headers))),
            ("params", json!(map_of(&self.params))),
            ("payload", json!(map_of(&self.payload))),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}

fn set_header(request: &mut PreparedRequest, name: &str, value: String) {
    request
        .headers
        .retain(|key, _| !key.eq_ignore_ascii_case(name));
    request
        .headers
        .insert(name.to_owned(), SecretString::new(value));
}

/// The arguments as a map, for the compatibility report.
#[cfg(feature = "compat")]
pub(crate) fn map_of(pairs: &Pairs) -> std::collections::BTreeMap<&str, &str> {
    pairs.iter().collect()
}
