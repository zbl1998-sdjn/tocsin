//! Pushbullet: `pbul://<access token>[/<target>...]`.
//!
//! A target is a device, a channel (`#tag`) or an e-mail address; without one
//! the message goes to every device. Every message is a `note` push.
//!
//! A file is uploaded first, in two requests (the upload address, then the
//! file), and then pushed as a message of its own after the note, to every
//! target. Nothing is sent when an upload fails, as in Apprise. That needs the
//! answers to the uploads, so files only work through
//! [`Service::plan`](crate::Service::plan). Unlike Apprise, the access token is
//! not sent to the upload address.

use std::collections::VecDeque;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::{
    Attachment, Format, Method, Notification, ParseError, Plan, PreparedRequest, RequestPolicy,
    Response, SecretString, TransportError, grammar, message,
    multipart::Multipart,
    options::{FormatMode, Options},
    plan::{Next, Sequence, Single, Step},
};

const PUSHES: &str = "https://api.pushbullet.com/v2/pushes";
const UPLOAD_REQUEST: &str = "https://api.pushbullet.com/v2/upload-request";

/// What Apprise puts in `targets` when none is given.
const ALL_DEVICES: &str = "ALL_DEVICES";

/// A parsed `pbul://` URL.
#[derive(Clone)]
#[cfg_attr(not(feature = "compat"), allow(dead_code))]
pub(crate) struct Pushbullet {
    access_token: SecretString,
    /// Unique and sorted, or `ALL_DEVICES` alone when the URL gave none.
    targets: Vec<String>,
    /// Whether the URL gave no target at all. A target that is spelled
    /// `ALL_DEVICES` is a device with that name.
    everyone: bool,
}

pub(crate) fn parse(input: &str) -> Result<(Options, Pushbullet), ParseError> {
    let raw = grammar::parse(input, grammar::Hosts::Any, FormatMode::Fixed(Format::Text))?;
    let access_token = grammar::token(&raw.options.host).ok_or(ParseError::InvalidToken)?;
    let mut candidates = raw.paths.clone();
    if let Some(to) = raw.query.get("to").filter(|to| !to.is_empty()) {
        candidates.push(to.clone());
    }
    let mut targets = grammar::list(&candidates.join(" "));
    let everyone = targets.is_empty();
    if everyone {
        targets.push(ALL_DEVICES.to_owned());
    }
    let pushbullet = Pushbullet {
        access_token: SecretString::new(access_token),
        targets,
        everyone,
    };
    Ok((raw.options, pushbullet))
}

impl Pushbullet {
    /// The `Authorization` header: the token is the user name of a basic
    /// authentication and the password is empty.
    fn authorization(&self) -> SecretString {
        SecretString::new(format!(
            "Basic {}",
            STANDARD.encode(format!("{}:", self.access_token.expose()))
        ))
    }

    /// A JSON post to Pushbullet.
    fn post(&self, url: &str, payload: &Value, policy: RequestPolicy) -> PreparedRequest {
        let mut request = PreparedRequest::json(url, payload).with_policy(policy);
        request
            .headers
            .insert("Authorization".to_owned(), self.authorization());
        request
    }

    /// The notes of one message, one for every target.
    fn notes(&self, policy: RequestPolicy, title: &str, body: &str) -> Vec<PreparedRequest> {
        // Apprise sends nothing when there is no text, for example with files
        // only.
        if body.is_empty() {
            return Vec::new();
        }
        self.targets
            .iter()
            .map(|target| {
                let mut payload = json!({"type": "note", "title": title, "body": body});
                if let Some(address) = grammar::email_of(target) {
                    payload["email"] = json!(address);
                } else if self.everyone {
                    // Without a target, Pushbullet sends to every device.
                } else if let Some(channel) = target.strip_prefix('#') {
                    payload["channel_tag"] = json!(channel);
                } else {
                    payload["device_iden"] = json!(target);
                }
                self.post(PUSHES, &payload, policy)
            })
            .collect()
    }

    pub(crate) fn prepare(
        &self,
        options: &Options,
        notification: &Notification,
    ) -> Vec<PreparedRequest> {
        message::parts(notification, 250, 32768, options.overflow)
            .into_iter()
            .flat_map(|(title, body)| self.notes(options.policy(), &title, &body))
            .collect()
    }

    /// [`prepare`](Self::prepare), plus the upload of files and the pushes that
    /// name them.
    pub(crate) fn plan(&self, options: &Options, notification: &Notification) -> Plan {
        let files: Vec<Attachment> = notification.carried(options.overflow).to_vec();
        if files.is_empty() {
            return Plan::requests(self.prepare(options, notification));
        }
        let policy = options.policy();
        let mut parts = message::parts(notification, 250, 32768, options.overflow).into_iter();
        let (title, body) = parts.next().unwrap_or_default();
        let pushbullet = self.clone();
        // What follows the uploads: for every target the note, then each file.
        let deliveries = move |pushes: Vec<Value>| -> Vec<Box<dyn Sequence>> {
            let notes = pushbullet.notes(policy, &title, &body);
            let mut steps = Vec::new();
            for target in 0..pushbullet.targets.len() {
                if let Some(note) = notes.get(target) {
                    steps.push(Step::delivery(note.clone()));
                }
                for push in &pushes {
                    steps.push(Step::delivery(pushbullet.post(PUSHES, push, policy)));
                }
            }
            Single::each(steps)
        };
        let mut plan = Plan::requests(Vec::new()).then(Uploads {
            pushbullet: self.clone(),
            policy,
            files: files.into_iter().enumerate().collect(),
            file: None,
            pushes: Vec::new(),
            upload: None,
            deliveries: Some(Box::new(deliveries)),
        });
        // Only the first part of a long message has the files.
        for (title, body) in parts {
            for note in self.notes(policy, &title, &body) {
                for sequence in Single::each(vec![Step::delivery(note)]) {
                    plan = plan.then(sequence);
                }
            }
        }
        plan
    }

    #[cfg(feature = "compat")]
    pub(crate) fn compat(&self, map: &mut serde_json::Map<String, Value>) {
        for (key, value) in [
            ("accesstoken", json!(self.access_token.expose())),
            ("targets", json!(self.targets)),
        ] {
            map.insert(key.to_owned(), value);
        }
    }
}

/// Builds what follows the uploads, from the push that names each file.
type Deliveries = Box<dyn FnOnce(Vec<Value>) -> Vec<Box<dyn Sequence>> + Send>;

/// Uploads every file, and then lets the pushes that name them go out. When a
/// request fails, nothing goes out.
struct Uploads {
    pushbullet: Pushbullet,
    policy: RequestPolicy,
    /// The files not asked for yet, with their numbers.
    files: VecDeque<(usize, Attachment)>,
    /// The file whose upload address was asked for.
    file: Option<(usize, Attachment)>,
    /// The push that names the file that is being sent.
    upload: Option<Value>,
    /// The push of every file that is up.
    pushes: Vec<Value>,
    deliveries: Option<Deliveries>,
}

impl Uploads {
    /// The request for the next file's upload address.
    fn ask(&mut self) -> Option<Step> {
        let (index, file) = self.files.pop_front()?;
        let payload = json!({
            "file_name": file.name_or_default(index + 1),
            "file_type": file.mime_type(),
        });
        self.file = Some((index, file));
        Some(Step::setup(self.pushbullet.post(
            UPLOAD_REQUEST,
            &payload,
            self.policy,
        )))
    }

    /// The next file's request, or what follows when every file is up.
    fn next(&mut self) -> Next {
        if let Some(step) = self.ask() {
            return Next::go(step);
        }
        let pushes = std::mem::take(&mut self.pushes);
        self.deliveries
            .take()
            .map_or_else(Next::done, |build| Next::then(build(pushes)))
    }
}

/// What Pushbullet answers to an upload request: the push that names the file
/// once it is up, and where the file goes.
fn read_upload_request(response: &Response) -> Option<(Value, String)> {
    let answer: Value = serde_json::from_slice(response.body.expose()).ok()?;
    let text = |key: &str| answer.get(key)?.as_str().map(str::to_owned);
    let (file_type, file_url) = (text("file_type")?, text("file_url")?);
    let mut push = json!({
        "type": "file",
        "file_name": text("file_name")?,
        "file_type": file_type,
        "file_url": file_url,
    });
    if file_type.starts_with("image/") {
        // An image is shown inline.
        push["image_url"] = json!(file_url);
    }
    Some((push, text("upload_url")?))
}

impl Sequence for Uploads {
    fn start(&mut self) -> Step {
        self.ask()
            .expect("uploads are planned only when there is a file")
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        // A failed request is already a failure of the plan, and with it
        // nothing is sent.
        let Ok(response) = result else {
            return Next::done();
        };
        if let Some(push) = self.upload.take() {
            // The file is up.
            self.pushes.push(push);
            return self.next();
        }
        let (Some((index, file)), Some((push, address))) =
            (self.file.take(), read_upload_request(response))
        else {
            return Next::fail(TransportError::InvalidResponse);
        };
        let name = file.name_or_default(index + 1);
        let (content_type, body) = Multipart::new()
            .file("file", &name, None, file.data())
            .finish();
        let request = PreparedRequest::with_body(Method::Post, address, Some(&content_type), body)
            .with_policy(self.policy);
        self.upload = Some(push);
        Next::go(Step::setup(request))
    }
}
