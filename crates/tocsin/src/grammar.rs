//! The Apprise URL grammar shared by every service.
//!
//! The behaviour here mirrors Apprise's own parser, and the differential tests
//! in `tests/compat.rs` check it against Apprise itself.

use std::collections::BTreeMap;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

use crate::{
    Format, ParseError, SecretString,
    options::{FormatMode, Options, Overflow},
};

/// What Apprise keeps unquoted in a path: Python's `quote` always leaves
/// letters, digits and `_.-~` alone, and Apprise adds `/:@!$&'()*+=`. Commas
/// and semicolons stay quoted because Apprise reserves them as list separators.
const PATH_QUOTE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'.')
    .remove(b'-')
    .remove(b'~')
    .remove(b'/')
    .remove(b':')
    .remove(b'@')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b'=');

/// What Python's `requests` leaves alone when it encodes a query string.
#[cfg(any(feature = "_query", feature = "_quote"))]
const FORM_QUOTE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'.')
    .remove(b'-')
    .remove(b'~');

/// Encode a path segment like Python's `quote(s, safe="")`.
#[cfg(feature = "_quote")]
pub(crate) fn quote(s: &str) -> String {
    utf8_percent_encode(s, FORM_QUOTE).to_string()
}

/// Encode a query-string key or value like `requests` does: spaces become `+`.
#[cfg(feature = "_query")]
pub(crate) fn form_encode(s: &str) -> String {
    utf8_percent_encode(s, FORM_QUOTE)
        .to_string()
        .replace("%20", "+")
}

/// `key=value&key=value`, encoded like `requests` does.
#[cfg(feature = "_pairs")]
pub(crate) fn encode_pairs(pairs: &Pairs) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", form_encode(key), form_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

pub(crate) fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

#[cfg(feature = "discord")]
pub(crate) fn encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// Python's `str.isspace` also counts the four information-separator controls.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Apprise's `is_ipaddr`: an IPv4 address, or an IPv6 address which comes back
/// in square brackets.
#[cfg(feature = "_verify")]
fn is_ipaddr(address: &str, ipv4: bool, ipv6: bool) -> Option<String> {
    use std::sync::LazyLock;

    use regex::Regex;

    static IPV4: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$",
        )
        .expect("static regex")
    });
    static IPV6: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)^(?:(?:[0-9a-f]{1,4}:){7,7}[0-9a-f]{1,4}|(?:[0-9a-f]{1,4}:){1,7}:",
            r"|(?:[0-9a-f]{1,4}:){1,6}:[0-9a-f]{1,4}|(?:[0-9a-f]{1,4}:){1,5}(?::[0-9a-f]{1,4}){1,2}",
            r"|(?:[0-9a-f]{1,4}:){1,4}(?::[0-9a-f]{1,4}){1,3}|(?:[0-9a-f]{1,4}:){1,3}(?::[0-9a-f]{1,4}){1,4}",
            r"|(?:[0-9a-f]{1,4}:){1,2}(?::[0-9a-f]{1,4}){1,5}|[0-9a-f]{1,4}:(?:(?::[0-9a-f]{1,4}){1,6})",
            r"|:(?:(?::[0-9a-f]{1,4}){1,7}|:)|fe80:(?::[0-9a-f]{0,4}){0,4}%[0-9a-z]{1,}",
            r"|::(?:ffff(?::0{1,4}){0,1}:){0,1}(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}",
            r"(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])",
            r"|(?:[0-9a-f]{1,4}:){1,4}:(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}",
            r"(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9]))$",
        ))
        .expect("static regex")
    });
    if ipv4 && IPV4.is_match(address) {
        return Some(address.to_owned());
    }
    if ipv6 {
        let inside = if let Some(rest) = address.strip_prefix('[') {
            rest.strip_suffix(']')?
        } else if address.ends_with(']') {
            return None;
        } else {
            address
        };
        if IPV6.is_match(inside) {
            return Some(format!("[{inside}]"));
        }
    }
    None
}

/// Apprise's `is_hostname`: a host name, or an IP address, that comes back
/// without a trailing dot.
#[cfg(feature = "_verify")]
pub(crate) fn is_hostname(hostname: &str) -> Option<String> {
    use std::sync::LazyLock;

    use regex::Regex;

    static LABEL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:[a-z0-9][a-z0-9_-]{1,62}|[a-z_-])$").expect("static regex")
    });
    let length = hostname.chars().count();
    if length == 0 || length > 253 {
        return None;
    }
    let hostname = hostname.strip_suffix('.').unwrap_or(hostname);
    let labels: Vec<&str> = hostname.split('.').collect();
    if labels.len() == 4 && hostname.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return is_ipaddr(hostname, true, false);
    }
    // Python's pattern also forbids a label that ends in `_` or `-`.
    if labels
        .iter()
        .all(|label| LABEL.is_match(label) && !label.ends_with(['_', '-']))
    {
        Some(hostname.to_owned())
    } else {
        is_ipaddr(hostname, true, true)
    }
}

/// What Apprise does before it chooses a plugin: every `/#` becomes `/%23` (so
/// `#channel` can be written in a path), and the URL must start, after any
/// whitespace, with a scheme of one to 32 letters or digits and `://`.
///
/// Returns the rewritten URL and the lower-cased scheme.
pub(crate) fn prepare_url(input: &str) -> Result<(String, String), ParseError> {
    let url = input.replace("/#", "/%23");
    let trimmed = url.trim_start_matches(is_python_space);
    let scheme: String = trimmed
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    let rest = trimmed[scheme.len()..]
        .strip_prefix("://")
        .ok_or(ParseError::InvalidUrl)?;
    // Python's `.*$` cannot cross a line break, except for a final one.
    if scheme.is_empty()
        || scheme.len() > 32
        || rest.strip_suffix('\n').unwrap_or(rest).contains('\n')
    {
        return Err(ParseError::InvalidUrl);
    }
    let scheme = scheme.to_ascii_lowercase();
    Ok((url, scheme))
}

/// Python's `str.strip()`.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_python_space)
}

/// What Python's `urlparse` calls the path: it drops the `;params` that follow
/// the last slash.
fn without_params(path: &str) -> &str {
    let start = path.rfind('/').unwrap_or(0);
    path[start..]
        .find(';')
        .map_or(path, |semicolon| &path[..start + semicolon])
}

/// Apprise's `tidy_path` for URL paths: strip, then collapse runs of slashes.
/// Its Windows rules for backslashes are not reproduced.
fn tidy_path(path: &str) -> String {
    let mut tidy = String::new();
    for c in strip(path).chars() {
        if c != '/' || !tidy.ends_with('/') {
            tidy.push(c);
        }
    }
    tidy
}

/// Apprise's `fullpath`: the path tidied, decoded once and quoted again.
///
/// When the whole URL ends with a separator but the quoted path does not (for
/// example because the separator belongs to the query), Apprise appends that
/// separator anyway, and so does this.
fn quoted_path(path: &str, url: &str) -> String {
    let mut quoted =
        utf8_percent_encode(&decode(&tidy_path(without_params(strip(path)))), PATH_QUOTE)
            .to_string();
    if let (Some(last), Some(url_last)) = (quoted.chars().last(), url.chars().last()) {
        if last != '/' && matches!(url_last, '/' | '\\') {
            quoted.push(url_last);
        }
    }
    quoted
}

/// Apprise's `split_path`: break on spaces, commas, slashes and backslashes,
/// then decode each segment.
fn split_path(fullpath: &str) -> Vec<String> {
    fullpath
        .trim_start_matches('/')
        .split([' ', '\t', '\r', '\n', ',', '\\', '/'])
        .filter(|s| !s.is_empty())
        .map(decode)
        .collect()
}

/// A token as Apprise's `validate_regex` accepts it. Its default pattern is a
/// prefix match on `\S+`, so the first character must not be whitespace, and
/// the surrounding whitespace is stripped.
#[cfg(feature = "_token")]
pub(crate) fn token(value: &str) -> Option<String> {
    value
        .chars()
        .next()
        .is_some_and(|c| !is_python_space(c))
        .then(|| strip(value).to_owned())
}

/// Apprise's `validate_regex` with a service's own pattern. Python's `$` also
/// matches before a final newline, and the accepted value is stripped.
#[cfg(feature = "_validate")]
pub(crate) fn validate(pattern: &regex::Regex, value: &str) -> Option<String> {
    let candidate = value.strip_suffix('\n').unwrap_or(value);
    pattern.is_match(candidate).then(|| strip(value).to_owned())
}

/// Arguments written as `+key=value`, `-key=value` or `:key=value`, in the
/// order of the URL. A repeated key keeps its first position, as in a Python
/// dictionary.
#[derive(Clone, Default)]
pub(crate) struct Pairs(Vec<(String, String)>);

impl Pairs {
    pub(crate) fn set(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.0.iter_mut().find(|(k, _)| k == key) {
            value.clone_into(&mut entry.1);
        } else {
            self.0.push((key.to_owned(), value.to_owned()));
        }
    }

    #[cfg(feature = "pushover")]
    pub(crate) fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    #[cfg(feature = "_pairs_iter")]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    #[cfg(feature = "_webhook")]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The arguments as a map, for services that only report them.
    #[cfg(feature = "_to_map")]
    pub(crate) fn to_map(&self) -> BTreeMap<String, String> {
        self.0.iter().cloned().collect()
    }
}

/// The query string as Apprise's `parse_qsd` reads it.
struct Query {
    /// Every argument, with lower-cased keys.
    all: BTreeMap<String, String>,
    /// Keys that start with `+` (or a space, which is what `+` decodes to).
    plus: Pairs,
    /// Keys that start with `-`.
    minus: Pairs,
    /// Keys that start with `:`.
    colon: Pairs,
}

/// Split on whitespace and `[ ] ; ,`, drop empties and duplicates, sort.
#[cfg(feature = "_list")]
pub(crate) fn list(s: &str) -> Vec<String> {
    use std::collections::BTreeSet;

    s.split(|c: char| c.is_whitespace() || "[];,".contains(c))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Split on Apprise's `CHANNEL_LIST_DELIM`: whitespace, `,`, `#`, `\` and `/`.
/// Unlike [`list`], this neither sorts nor removes duplicates.
#[cfg(feature = "_channels")]
pub(crate) fn channel_list(text: &str) -> Vec<String> {
    text.split([' ', '\t', '\r', '\n', ',', '#', '\\', '/'])
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Apprise's lenient boolean: only the first two letters matter.
pub(crate) fn bool_value(value: &str) -> bool {
    let lower: String = value.chars().take(2).flat_map(char::to_lowercase).collect();
    matches!(
        lower.as_str(),
        "en" | "al" | "t" | "y" | "ye" | "on" | "1" | "tr"
    )
}

fn timezone(name: &str) -> Result<String, ParseError> {
    let lower = name.trim().to_lowercase();
    if ["utc", "z", "gmt", "etc/utc", "etc/gmt", "gmt0", "utc0"].contains(&lower.as_str()) {
        return Ok("UTC".into());
    }
    let zones = include_str!("../compat/timezones.txt");
    if let Some(zone) = zones.lines().find(|s| *s == name) {
        return Ok(zone.into());
    }
    if let Some(zone) = zones.lines().find(|s| s.eq_ignore_ascii_case(&lower)) {
        return Ok(zone.into());
    }
    let mut aliases = zones.lines().filter(|zone| {
        zone.split('/')
            .skip(1)
            .take(2)
            .collect::<Vec<_>>()
            .join("/")
            .eq_ignore_ascii_case(&lower)
    });
    let first = aliases.next().ok_or(ParseError::InvalidOption)?;
    // Apprise iterates an unordered set for ambiguous location aliases. A
    // deterministic implementation rejects the ambiguity instead.
    if aliases.next().is_some() {
        return Err(ParseError::InvalidOption);
    }
    Ok(first.into())
}

/// A URL split into the pieces every service needs.
pub(crate) struct Raw {
    pub(crate) options: Options,
    /// Query arguments with lower-cased keys, percent-decoded and trimmed.
    pub(crate) query: BTreeMap<String, String>,
    /// Arguments written as `:key=value`.
    #[cfg_attr(
        not(feature = "_payload"),
        allow(dead_code, reason = "not every service keeps these arguments")
    )]
    pub(crate) payload: Pairs,
    /// Arguments written as `+key=value`.
    #[cfg_attr(
        not(feature = "_webhook"),
        allow(dead_code, reason = "only the webhooks send these")
    )]
    pub(crate) headers: Pairs,
    /// Arguments written as `-key=value`.
    #[cfg_attr(
        not(feature = "_webhook"),
        allow(dead_code, reason = "only the webhooks send these")
    )]
    pub(crate) params: Pairs,
    /// The path as Apprise quotes it, `""` when the URL has none.
    #[cfg_attr(
        not(feature = "_webhook"),
        allow(dead_code, reason = "only the webhooks use the path as written")
    )]
    pub(crate) fullpath: String,
    /// Decoded path segments.
    #[cfg_attr(
        not(feature = "_paths"),
        allow(dead_code, reason = "the webhooks keep the path as written")
    )]
    pub(crate) paths: Vec<String>,
    /// The host exactly as written, without percent-decoding.
    #[cfg(feature = "ntfy")]
    pub(crate) original_host: String,
}

impl Raw {
    #[cfg(feature = "_flag")]
    pub(crate) fn flag(&self, key: &str, default: bool) -> bool {
        self.query.get(key).map_or(default, |v| bool_value(v))
    }

    #[cfg(feature = "_raw_optional")]
    pub(crate) fn optional(&self, key: &str) -> Option<String> {
        self.query.get(key).map(|v| decode(v))
    }
}

/// What a service accepts as the host part of its URL.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hosts {
    /// Any text but not nothing, for services whose URL starts with an id or
    /// a token and where Apprise does not check the host.
    #[cfg(feature = "_any_host")]
    Any,
    /// Any text, or none at all.
    #[cfg(feature = "_optional_host")]
    Optional,
    /// A valid host name or address with a numeric port, as Apprise checks it.
    #[cfg(feature = "_verify")]
    Verified,
}

/// Parse a notification URL.
pub(crate) fn parse(input: &str, hosts: Hosts, formats: FormatMode) -> Result<Raw, ParseError> {
    let allow_empty_host = match hosts {
        #[cfg(feature = "_any_host")]
        Hosts::Any => false,
        #[cfg(feature = "_optional_host")]
        Hosts::Optional => true,
        #[cfg(feature = "_verify")]
        Hosts::Verified => false,
    };
    let (scheme, rest) = input
        .trim()
        .split_once("://")
        .ok_or(ParseError::InvalidUrl)?;
    // Apprise's `VALID_URL_RE` takes every slash or backslash after `scheme:`
    // as part of the separator, so `ntfys:///topic` is `ntfys://topic`.
    let rest = rest.trim_start_matches(['/', '\\']);
    let (location, query_string) = rest.split_once('?').unwrap_or((rest, ""));
    let location = location.split('#').next().unwrap_or("");
    let (authority, path) = location.split_once('/').unwrap_or((location, ""));
    if authority.is_empty() && !allow_empty_host {
        return Err(ParseError::InvalidUrl);
    }
    let Query {
        all: query,
        plus: headers,
        minus: params,
        colon: payload,
    } = parse_query(query_string);
    let (hostport, mut user, mut password) = split_userinfo(authority);
    let (host, port, bad_port) = split_host_port(hostport);
    if host.is_empty() && !allow_empty_host {
        return Err(ParseError::InvalidUrl);
    }
    #[cfg(feature = "ntfy")]
    let original_host = host;
    #[cfg(feature = "_verify")]
    let verified;
    #[cfg(feature = "_verify")]
    let host = if hosts == Hosts::Verified {
        if bad_port {
            return Err(ParseError::InvalidUrl);
        }
        verified = is_hostname(host).ok_or(ParseError::InvalidUrl)?;
        verified.as_str()
    } else {
        host
    };
    #[cfg(not(feature = "_verify"))]
    let _ = bad_port;
    apply_credential_overrides(&query, &mut user, &mut password);
    let options = options_from(scheme, (host, port), (user, password), &query, formats)?;
    // Apprise decodes the path once and quotes it again. That leaves the slash
    // as the only separator: `%2F` splits a segment, while spaces, commas and
    // semicolons stay inside it.
    let fullpath = if location.contains('/') {
        quoted_path(&format!("/{path}"), input)
    } else {
        String::new()
    };
    let paths = split_path(&fullpath);
    Ok(Raw {
        options,
        query,
        payload,
        headers,
        params,
        fullpath,
        paths,
        #[cfg(feature = "ntfy")]
        original_host: original_host.to_owned(),
    })
}

/// Read the query string like Apprise's `parse_qsd`: the first character of a
/// key is kept as it is (it may be `+`), later `+` become spaces, and values
/// keep their `+`.
fn parse_query(query_string: &str) -> Query {
    let mut query = Query {
        all: BTreeMap::new(),
        plus: Pairs::default(),
        minus: Pairs::default(),
        colon: Pairs::default(),
    };
    for entry in query_string.split(['&', ';']).filter(|s| !s.is_empty()) {
        let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
        let mut chars = key.chars();
        let first = chars.next().unwrap_or_default();
        let key = decode(&format!("{first}{}", chars.as_str().replace('+', " ")));
        let value = strip(&decode(value)).to_owned();
        query.all.insert(strip(&key).to_lowercase(), value.clone());
        // Python's `.` stops at a newline, so a key ends there.
        let name = |prefix: &[char]| {
            let mut chars = key.chars();
            chars
                .next()
                .filter(|c| prefix.contains(c))
                .map(|_| chars.as_str().split('\n').next().unwrap_or(""))
        };
        if let Some(name) = name(&[' ', '+']) {
            query.plus.set(name, &value);
        }
        if let Some(name) = name(&['-']) {
            query.minus.set(name, &value);
        }
        if let Some(name) = name(&[':']) {
            query.colon.set(name, &value);
        }
    }
    query
}

/// Split `user:password@host:port` into the host part and the credentials.
fn split_userinfo(authority: &str) -> (&str, Option<String>, Option<String>) {
    let parts: Vec<_> = authority.split('@').collect();
    if parts.len() > 1 {
        let mut split = parts[0].split(':');
        let user = split.next().map(str::to_owned);
        let password = split.next().map(str::to_owned);
        (parts[1], user, password)
    } else {
        (authority, None, None)
    }
}

/// `?user=`, `?pass=` and `?password=` win over the URL's own credentials.
fn apply_credential_overrides(
    query: &BTreeMap<String, String>,
    user: &mut Option<String>,
    password: &mut Option<String>,
) {
    if let Some(value) = query.get("password") {
        *password = Some(value.clone());
    }
    if let Some(value) = query.get("pass") {
        *password = Some(value.clone());
    }
    if let Some(value) = query.get("user") {
        if password.is_none() {
            password.clone_from(user);
        }
        *user = Some(value.clone());
    }
}

/// The options common to every service.
fn options_from(
    scheme: &str,
    (host, port): (&str, Option<i64>),
    (user, password): (Option<String>, Option<String>),
    query: &BTreeMap<String, String>,
    formats: FormatMode,
) -> Result<Options, ParseError> {
    let override_format = query.get("format").and_then(|s| s.parse::<Format>().ok());
    let (format, format_override) = match formats {
        FormatMode::Supported(_) => (formats, override_format),
        FormatMode::Fixed(default) => (FormatMode::Fixed(override_format.unwrap_or(default)), None),
    };
    let overflow = match query.get("overflow").map(|s| s.to_lowercase()).as_deref() {
        Some("truncate") => Overflow::Truncate,
        Some("split") => Overflow::Split,
        _ => Overflow::Upstream,
    };
    let retry = query
        .get("retry")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(0)
        .min(10);
    let wait = query
        .get("wait")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.5)
        .min(20.0);
    let timeout = |key: &str| {
        query
            .get(key)
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(4.0)
    };
    let flag = |key: &str, default: bool| query.get(key).map_or(default, |s| bool_value(s));
    Ok(Options {
        host: decode(host),
        port,
        user: user.map(|s| decode(&s)),
        password: password.map(|s| SecretString::new(decode(&s))),
        secure: scheme.to_ascii_lowercase().ends_with('s'),
        format,
        format_override,
        overflow,
        verify_tls: flag("verify", true),
        redirects: flag("redirect", true),
        connect_timeout: timeout("cto"),
        read_timeout: timeout("rto"),
        retry: u8::try_from(retry).unwrap_or(10),
        wait,
        optional: flag("optional", false),
        emojis: query.get("emojis").map(|s| bool_value(s)),
        store: flag("store", true),
        tz: query.get("tz").map(|s| timezone(s)).transpose()?,
    })
}

/// Apprise splits `host:port` with `^(\[[0-9a-f:]+\]|[^:]+):([^:]*)$`.
///
/// The last value says the port was there but not a number. The host is then
/// the whole text, which is what a service that does not verify hosts gets.
fn split_host_port(hostport: &str) -> (&str, Option<i64>, bool) {
    let bracketed = hostport.strip_prefix('[').and_then(|rest| {
        let end = rest.find(']')?;
        let inside = &rest[..end];
        let port = rest[end + 1..].strip_prefix(':')?;
        let valid = !inside.is_empty()
            && inside.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
            && !port.contains(':');
        valid.then(|| (&hostport[..end + 2], port))
    });
    let plain = || {
        let (host, port) = hostport.split_once(':')?;
        (!host.is_empty() && !port.contains(':')).then_some((host, port))
    };
    match bracketed.or_else(plain) {
        None => (hostport, None, false),
        Some((host, port)) => match port
            .chars()
            .any(|c| c.is_ascii_digit())
            .then(|| port.parse::<i64>().ok())
            .flatten()
        {
            Some(port) => (host, Some(port), false),
            None => (hostport, None, true),
        },
    }
}

/// Apprise's check for an e-mail address, which also reads `Name <address>` and
/// `label+address`. It only looks at the start of the text, like Python's
/// `re.match`.
#[cfg(feature = "_email")]
mod email {
    use std::sync::LazyLock;

    use regex::Regex;

    /// The group `full` is the address itself.
    static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r#"(?i)^(?:(?:[\s"']{0,32})?(?P<name>[^:<'"]{0,128})?[:<\s'"]{1,32})?(?P<full>(?:(?P<label>[^+\s]{1,128})\+)?(?P<email>(?P<userid>[a-z0-9_!#$%&*/=?%`{|}~^-]+(?:\.[a-z0-9_!#$%&'*/=?%`{|}~^-]+)*)@(?P<domain>(?:(?:[a-z0-9](?:[a-z0-9_-]*[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9_-]*[a-z0-9]))|[a-z0-9][a-z0-9_-]{5,})))\s*>?"#,
        )
        .expect("static regex")
    });

    /// The address in `target`, if it is an e-mail address.
    pub(crate) fn email_of(target: &str) -> Option<String> {
        EMAIL.captures(target).map(|found| found["full"].to_owned())
    }
}

#[cfg(feature = "_email")]
pub(crate) use email::email_of;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "_list")]
    fn list_sorts_and_deduplicates() {
        assert_eq!(list("b, a;[a] c"), ["a", "b", "c"]);
    }

    #[test]
    #[cfg(feature = "_email")]
    fn an_email_address_is_found_the_way_apprise_finds_it() {
        for (target, address) in [
            ("bob@example.com", Some("bob@example.com")),
            ("label+bob@example.com", Some("label+bob@example.com")),
            ("Bob <bob@example.com>", Some("bob@example.com")),
            ("user@localhost", Some("user@localhost")),
            ("#general", None),
            ("@bob", None),
            ("general", None),
            ("+C123", None),
            ("bob@x", None),
        ] {
            assert_eq!(email_of(target).as_deref(), address, "target: {target}");
        }
    }

    #[test]
    fn booleans_follow_apprise() {
        for yes in ["yes", "True", "on", "1", "enable", "always"] {
            assert!(bool_value(yes), "{yes}");
        }
        for no in ["no", "false", "off", "0", "never", ""] {
            assert!(!bool_value(no), "{no}");
        }
    }

    #[test]
    fn host_and_port_split() {
        assert_eq!(
            split_host_port("example.com:8080"),
            ("example.com", Some(8080), false)
        );
        assert_eq!(split_host_port("example.com"), ("example.com", None, false));
        assert_eq!(split_host_port("[::1]:99"), ("[::1]", Some(99), false));
        assert_eq!(split_host_port("[::1]"), ("[::1]", None, false));
        assert_eq!(split_host_port("a:b:c"), ("a:b:c", None, false));
        assert_eq!(
            split_host_port("example.com:abc"),
            ("example.com:abc", None, true)
        );
        assert_eq!(
            split_host_port("example.com:"),
            ("example.com:", None, true)
        );
    }
}
