//! Turning a [`Notification`] into the text a service sends: choosing the
//! format, merging the title, and applying the overflow mode.

use crate::{Format, Notification, options::Overflow};

/// The format a service will use for this notification.
#[cfg(feature = "_format")]
pub(crate) fn format(options: &crate::options::Options, notification: &Notification) -> Format {
    use crate::options::FormatMode;

    let configured = options.format_override.or(match options.format {
        FormatMode::Fixed(format) => Some(format),
        FormatMode::Supported(_) => None,
    });
    configured.unwrap_or_else(|| match options.format {
        FormatMode::Supported(list) => {
            if list.contains(&notification.format) {
                notification.format
            } else {
                list.first().copied().unwrap_or(Format::Text)
            }
        }
        FormatMode::Fixed(format) => format,
    })
}

/// Title and body as one text, for services that have no separate title.
#[cfg(feature = "_merged")]
pub(crate) fn merged(notification: &Notification, format: Format) -> String {
    let title = notification.title.trim();
    let body = notification.body.trim_end();
    if title.is_empty() {
        return body.to_owned();
    }
    match format {
        Format::Html => format!("<b>{title}</b><br />\r\n{body}"),
        Format::Markdown => format!(
            "# {}\n{body}",
            title.trim_start_matches(['#', '-', ' ', '\t', '\r', '\n'])
        ),
        Format::Text => format!("{title}\r\n{body}"),
    }
}

/// The first `max` characters, without trailing whitespace.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    s.chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Cut a text into pieces of at most `max` characters, preferring to break at
/// whitespace.
pub(crate) fn chunks(s: &str, max: usize, overflow: Overflow) -> Vec<String> {
    if overflow == Overflow::Upstream || s.chars().count() <= max {
        return vec![s.to_owned()];
    }
    if overflow == Overflow::Truncate {
        return vec![truncate(s, max)];
    }
    let chars: Vec<_> = s.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + max).min(chars.len());
        if end < chars.len() {
            // Keep natural boundaries where available; hard split long words.
            let boundary = chars[start..end].iter().rposition(|c| c.is_whitespace());
            if let Some(boundary) = boundary.filter(|boundary| *boundary > 0) {
                end = start + boundary + 1;
            }
        }
        chunks.push(
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_owned(),
        );
        start = end;
    }
    chunks
}

/// Apprise's plain-text title reservation and repeated split counters, for
/// services with a separate title and body.
#[cfg(feature = "_parts")]
pub(crate) fn parts(
    notification: &Notification,
    title_max: usize,
    body_max: usize,
    overflow: Overflow,
) -> Vec<(String, String)> {
    let title = notification.title.trim();
    let body = notification.body.trim_end();
    if overflow == Overflow::Upstream {
        return vec![(title.to_owned(), body.to_owned())];
    }
    let reserved = (title.chars().count() + 12).min(title_max).min(body_max);
    let mut title = truncate(title, reserved);
    let max = body_max - if title.is_empty() { 0 } else { reserved };
    let mut pieces = chunks(body, max, overflow);
    if overflow == Overflow::Truncate && notification.format == Format::Markdown {
        for piece in &mut pieces {
            let trailing = piece.chars().rev().take_while(|c| *c == '\\').count();
            if trailing % 2 == 1 {
                piece.pop();
            }
        }
    }
    let digits = pieces.len().to_string().len();
    let counter = overflow == Overflow::Split
        && pieces.len() > 1
        && !title.is_empty()
        && reserved > 12
        && max >= 130
        && 4 + 2 * digits <= 12;
    if counter {
        title = truncate(&title, reserved - (4 + 2 * digits));
    }
    let count = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let suffix = if counter {
                format!(" [{:0digits$}/{:0digits$}]", i + 1, count)
            } else {
                String::new()
            };
            (
                format!("{title}{suffix}"),
                body.trim_start_matches(['\r', '\n', '\u{000b}', '\u{000c}'])
                    .to_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_keeps_everything() {
        assert_eq!(chunks("a b c", 1, Overflow::Upstream), ["a b c"]);
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("🦀🦀🦀", 2), "🦀🦀");
        assert_eq!(chunks("🦀🦀🦀", 2, Overflow::Truncate), ["🦀🦀"]);
    }

    #[test]
    fn split_prefers_whitespace() {
        assert_eq!(
            chunks("aaa bbb ccc", 7, Overflow::Split),
            ["aaa", "bbb ccc"]
        );
    }
}
