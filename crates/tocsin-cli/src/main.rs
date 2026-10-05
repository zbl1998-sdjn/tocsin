//! `tocsin`: send one notification to many services from Apprise-style URLs.

use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, ValueEnum};
use tocsin::{
    Attachment, Format, Kind, Notification, Notifier, Outcome, PreparedRequest, Response,
    Transport, TransportError, UreqTransport,
};

/// The most text read from standard input.
const MAX_STDIN_BYTES: u64 = 1 << 20;

/// The largest file that is attached. It is held in memory, and Telegram, the
/// service with the most generous limit, takes 50 MB.
const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024;

/// Send one notification to every service URL.
///
/// Exit status: 0 when every request was delivered, 1 when a delivery failed
/// or a URL had nothing to send to, 2 for a usage error or an invalid URL.
#[derive(Parser)]
#[command(name = "tocsin", version, max_term_width = 100)]
struct Cli {
    /// Notification URLs, such as `ntfy://my-topic` or `tgram://<bot>/<chat>`.
    /// When none are given, the whitespace-separated `TOCSIN_URLS` is used.
    /// Arguments are visible to other users of the machine, so prefer the
    /// environment variable for URLs that hold tokens.
    #[arg(value_name = "URL")]
    urls: Vec<String>,

    /// The message. Read from standard input when omitted.
    #[arg(short, long)]
    body: Option<String>,

    /// The message title.
    #[arg(short, long)]
    title: Option<String>,

    /// The kind of notification.
    #[arg(short = 'n', long, value_enum, default_value_t = KindArg::Info)]
    notification_type: KindArg,

    /// The format of the body.
    #[arg(short, long, value_enum, default_value_t = FormatArg::Text)]
    input_format: FormatArg,

    /// A file to send along; give it more than once for more files. Services
    /// that cannot carry files send the message without them, and the message
    /// may be left out when there is a file.
    #[arg(short, long, value_name = "FILE")]
    attach: Vec<PathBuf>,

    /// Print the host of each request instead of sending it.
    #[arg(short, long)]
    dry_run: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Info,
    Success,
    Warning,
    Failure,
}

impl From<KindArg> for Kind {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Info => Self::Info,
            KindArg::Success => Self::Success,
            KindArg::Warning => Self::Warning,
            KindArg::Failure => Self::Failure,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Text,
    Markdown,
    Html,
}

impl From<FormatArg> for Format {
    fn from(format: FormatArg) -> Self {
        match format {
            FormatArg::Text => Self::Text,
            FormatArg::Markdown => Self::Markdown,
            FormatArg::Html => Self::Html,
        }
    }
}

/// Prints where each request would go and reports success without any I/O.
struct DryRun;

impl Transport for DryRun {
    fn send(&mut self, request: &PreparedRequest) -> Result<Response, TransportError> {
        // A closed stdout must not turn a dry run into a failure.
        let _ = writeln!(io::stdout(), "{}", request.summary());
        Ok(Response::new(200))
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(io::stderr(), "tocsin: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let urls = if cli.urls.is_empty() {
        std::env::var("TOCSIN_URLS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    } else {
        cli.urls
    };
    if urls.is_empty() {
        return Err("no URL given (pass one or set TOCSIN_URLS)".to_owned());
    }

    let mut notifier = Notifier::new();
    for (index, url) in urls.iter().enumerate() {
        // The error never contains the URL, so only its position is shown.
        notifier
            .add(url)
            .map_err(|error| format!("URL {}: {error}", index + 1))?;
    }

    let attachments = cli
        .attach
        .iter()
        .map(|path| read_attachment(path))
        .collect::<Result<Vec<_>, _>>()?;
    let body = match cli.body {
        Some(body) => body,
        // A file can be sent on its own, so a terminal is not an error then.
        None if !attachments.is_empty() && io::stdin().is_terminal() => String::new(),
        None => read_stdin()?,
    };
    if body.trim().is_empty() && attachments.is_empty() {
        return Err("the message is empty".to_owned());
    }
    let mut notification = Notification::new(body)
        .kind(cli.notification_type.into())
        .format(cli.input_format.into());
    if let Some(title) = cli.title {
        notification = notification.title(title);
    }
    for attachment in attachments {
        notification = notification.attach(attachment);
    }

    let report = if cli.dry_run {
        notifier.send(&notification, &mut DryRun)
    } else {
        notifier.send(&notification, &mut UreqTransport)
    };
    let mut failed = false;
    for receipt in report.failures() {
        let reason = match &receipt.outcome {
            // A dry run gets no real answers, so the requests that depend on
            // one, or that read one, cannot be shown and are not failures.
            Outcome::Failed(TransportError::InvalidResponse | TransportError::Rejected)
                if cli.dry_run =>
            {
                "the answer of the service is not read in a dry run, so requests that depend on it are not shown".to_owned()
            }
            Outcome::Failed(error) => {
                failed = true;
                error.to_string()
            }
            _ => {
                failed = true;
                "nothing to send (the URL has no target)".to_owned()
            }
        };
        let _ = writeln!(io::stderr(), "tocsin: {}: {reason}", receipt.service);
    }
    Ok(if failed || report.receipts().is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// A file to attach, named after the last part of its path.
fn read_attachment(path: &Path) -> Result<Attachment, String> {
    // The path is the user's own, so it is fine to show, but not what is inside.
    let shown = path.display();
    let size = fs::metadata(path)
        .map_err(|_| format!("cannot read {shown}"))?
        .len();
    if size > MAX_ATTACHMENT_BYTES {
        return Err(format!("{shown} is larger than 50 MiB"));
    }
    let data = fs::read(path).map_err(|_| format!("cannot read {shown}"))?;
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    Ok(Attachment::new(name, data))
}

/// The message from a pipe or file; an interactive terminal is a usage error.
fn read_stdin() -> Result<String, String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Err("no message (use --body or pipe it in)".to_owned());
    }
    let mut text = String::new();
    stdin
        .lock()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| "the message on standard input is not valid UTF-8".to_owned())?;
    Ok(text)
}
