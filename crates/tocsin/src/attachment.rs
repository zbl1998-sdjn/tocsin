//! Files that travel with a notification.

use std::{fmt, sync::Arc};

/// A file to send along with a notification.
///
/// The bytes are held in memory, and cloning an attachment does not copy them.
/// Its [`Debug`] output hides everything, because a file's name and content are
/// often sensitive.
///
/// ```
/// use tocsin::{Attachment, Notification};
///
/// let report = Attachment::new("report.pdf", b"%PDF-1.7".to_vec());
/// assert_eq!(report.mime_type(), "application/pdf");
///
/// let notification = Notification::new("The report is ready").attach(report);
/// assert_eq!(notification.attachments().len(), 1);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Attachment {
    name: String,
    mime: String,
    data: Arc<[u8]>,
}

impl Attachment {
    /// A file with this name and content. The media type is taken from the
    /// extension of the name, and is `application/octet-stream` when there is
    /// none or it is not known; set another with [`mime`](Self::mime).
    pub fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        let name = name.into();
        let mime = guess_mime(&name).to_owned();
        Self {
            name,
            mime,
            data: Arc::from(data.into()),
        }
    }

    /// Set the media type, such as `image/png`.
    #[must_use]
    pub fn mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = mime.into();
        self
    }

    /// The file name. A service that needs one names an attachment without it
    /// `file001.dat`, `file002.dat` and so on.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The media type.
    #[must_use]
    pub fn mime_type(&self) -> &str {
        &self.mime
    }

    /// The content. Handle the result with care.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The size in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the file is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The file name, or `file<no>.dat` for the `no`-th attachment (counting
    /// from 1) when it has none, as Apprise names them.
    #[cfg_attr(
        not(any(
            feature = "json",
            feature = "form",
            feature = "telegram",
            feature = "pushover",
            feature = "mattermost",
            feature = "slack"
        )),
        allow(dead_code, reason = "only the services that upload files name them")
    )]
    pub(crate) fn name_or_default(&self, no: usize) -> String {
        if self.name.is_empty() {
            format!("file{no:03}.dat")
        } else {
            self.name.clone()
        }
    }
}

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Attachment([REDACTED])")
    }
}

/// The media type that goes with a file name's extension.
fn guess_mime(name: &str) -> &'static str {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return "application/octet-stream";
    };
    match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpe" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/vnd.microsoft.icon",
        "tif" | "tiff" => "image/tiff",
        "mp4" => "video/mp4",
        "mpeg" | "mpg" => "video/mpeg",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mp3" | "mp2" => "audio/mpeg",
        "m4a" => "audio/mp4a-latm",
        "wav" => "audio/x-wav",
        "ogg" | "oga" => "audio/ogg",
        "aac" => "audio/aac",
        "txt" | "text" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "md" => "text/markdown",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::Attachment;

    #[test]
    fn the_media_type_comes_from_the_extension() {
        for (name, mime) in [
            ("a.png", "image/png"),
            ("A.JPG", "image/jpeg"),
            ("clip.mp4", "video/mp4"),
            ("notes.txt", "text/plain"),
            ("archive.tar.gz", "application/octet-stream"),
            ("no-extension", "application/octet-stream"),
            ("", "application/octet-stream"),
            ("trailing.", "application/octet-stream"),
        ] {
            assert_eq!(
                Attachment::new(name, Vec::new()).mime_type(),
                mime,
                "{name}"
            );
        }
        let custom = Attachment::new("a.png", Vec::new()).mime("image/x-custom");
        assert_eq!(custom.mime_type(), "image/x-custom");
    }

    #[test]
    fn debug_hides_name_and_content_and_clones_share_the_bytes() {
        let secret = Attachment::new("FAKE_secret_name.txt", b"FAKE_secret_content".to_vec());
        assert_eq!(format!("{secret:?}"), "Attachment([REDACTED])");
        let clone = secret.clone();
        assert!(std::ptr::eq(secret.data().as_ptr(), clone.data().as_ptr()));
        assert_eq!(clone.len(), 19);
        assert!(!clone.is_empty());
    }

    #[test]
    fn a_nameless_attachment_gets_a_numbered_name() {
        assert_eq!(
            Attachment::new("", Vec::new()).name_or_default(2),
            "file002.dat"
        );
        assert_eq!(
            Attachment::new("x.txt", Vec::new()).name_or_default(2),
            "x.txt"
        );
    }
}
