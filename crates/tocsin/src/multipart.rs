//! `multipart/form-data` bodies, laid out the way Python's `requests` writes
//! them: the fields first, then the files, a part's `Content-Type` only when it
//! has one.

/// What one part holds.
enum Content<'a> {
    #[allow(
        dead_code,
        reason = "Slack only sends files, so a build with only Slack has no field"
    )]
    Field(&'a str),
    File {
        filename: &'a str,
        mime: Option<&'a str>,
        data: &'a [u8],
    },
}

struct Part<'a> {
    name: &'a str,
    content: Content<'a>,
}

/// A body that is being put together.
#[derive(Default)]
pub(crate) struct Multipart<'a> {
    parts: Vec<Part<'a>>,
}

/// A name or file name as it goes in a header: the characters that would end
/// the quoted string or the line are percent-encoded, as browsers and
/// `urllib3` do.
fn quoted(text: &str) -> String {
    text.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

impl<'a> Multipart<'a> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A plain field.
    #[must_use]
    #[allow(
        dead_code,
        reason = "Slack only sends files, so a build with only Slack has no field"
    )]
    pub(crate) fn field(mut self, name: &'a str, value: &'a str) -> Self {
        self.parts.push(Part {
            name,
            content: Content::Field(value),
        });
        self
    }

    /// A file.
    #[must_use]
    pub(crate) fn file(
        mut self,
        name: &'a str,
        filename: &'a str,
        mime: Option<&'a str>,
        data: &'a [u8],
    ) -> Self {
        self.parts.push(Part {
            name,
            content: Content::File {
                filename,
                mime,
                data,
            },
        });
        self
    }

    /// The `Content-Type` header value and the body.
    ///
    /// The boundary is the first of `tocsin-0`, `tocsin-1`, ... that does not
    /// occur in any part, so the same input always gives the same bytes.
    pub(crate) fn finish(self) -> (String, Vec<u8>) {
        let mut count = 0_u32;
        let boundary = loop {
            let delimiter = format!("--tocsin-{count}");
            let taken = self.parts.iter().any(|part| match part.content {
                Content::Field(value) => contains(value.as_bytes(), delimiter.as_bytes()),
                Content::File { data, .. } => contains(data, delimiter.as_bytes()),
            });
            if !taken {
                break delimiter;
            }
            count += 1;
        };
        let size: usize = self
            .parts
            .iter()
            .map(|part| match part.content {
                Content::Field(value) => value.len(),
                Content::File { data, .. } => data.len(),
            } + 160)
            .sum();
        let mut body = Vec::with_capacity(size + boundary.len() + 8);
        for part in &self.parts {
            body.extend_from_slice(boundary.as_bytes());
            body.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"");
            body.extend_from_slice(quoted(part.name).as_bytes());
            body.push(b'"');
            match part.content {
                Content::Field(value) => {
                    body.extend_from_slice(b"\r\n\r\n");
                    body.extend_from_slice(value.as_bytes());
                }
                Content::File {
                    filename,
                    mime,
                    data,
                } => {
                    body.extend_from_slice(b"; filename=\"");
                    body.extend_from_slice(quoted(filename).as_bytes());
                    body.push(b'"');
                    if let Some(mime) = mime {
                        body.extend_from_slice(b"\r\nContent-Type: ");
                        body.extend_from_slice(mime.as_bytes());
                    }
                    body.extend_from_slice(b"\r\n\r\n");
                    body.extend_from_slice(data);
                }
            }
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(boundary.as_bytes());
        body.extend_from_slice(b"--\r\n");
        let content_type = format!("multipart/form-data; boundary={}", &boundary[2..]);
        (content_type, body)
    }
}

#[cfg(test)]
mod tests {
    use super::Multipart;

    #[test]
    fn fields_and_files_are_laid_out_like_requests_does() {
        let (content_type, body) = Multipart::new()
            .field("chat_id", "42")
            .file("photo", "a \"b\".png", Some("image/png"), b"\x89PNG")
            .file("raw", "r.bin", None, b"xyz")
            .finish();
        assert_eq!(content_type, "multipart/form-data; boundary=tocsin-0");
        assert_eq!(
            body,
            b"--tocsin-0\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n42\r\n\
              --tocsin-0\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"a %22b%22.png\"\r\n\
              Content-Type: image/png\r\n\r\n\x89PNG\r\n\
              --tocsin-0\r\nContent-Disposition: form-data; name=\"raw\"; filename=\"r.bin\"\r\n\r\nxyz\r\n\
              --tocsin-0--\r\n"
        );
    }

    #[test]
    fn the_boundary_never_occurs_in_the_content() {
        let (content_type, body) = Multipart::new()
            .field("note", "--tocsin-0 and --tocsin-1")
            .file("f", "f.txt", None, b"x--tocsin-2y")
            .finish();
        assert_eq!(content_type, "multipart/form-data; boundary=tocsin-3");
        let text = String::from_utf8(body).expect("text");
        assert!(text.ends_with("--tocsin-3--\r\n"));
        assert_eq!(text.matches("--tocsin-3\r\n").count(), 2);
    }

    #[test]
    fn a_body_without_parts_is_just_the_closing_boundary() {
        let (_, body) = Multipart::new().finish();
        assert_eq!(body, b"--tocsin-0--\r\n");
    }
}
