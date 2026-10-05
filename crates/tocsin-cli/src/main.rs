//! `tocsin`: send one notification to many services from Apprise-style URLs.

mod hook;
mod mcp;

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
/// or a URL had nothing to send to, 2 for a usage error or an invalid URL. With
/// `--hook` it is never 2, because that status blocks the action in a Claude
/// Code hook.
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

    /// The kind of notification (info unless a hook gives another).
    #[arg(short = 'n', long, value_enum)]
    notification_type: Option<KindArg>,

    /// Build the message from the hook of a coding agent: the JSON that Claude
    /// Code writes to standard input, or the one that Codex gives as the last
    /// argument. `--body`, `--title` and `-n` still win.
    #[arg(long, value_enum, value_name = "AGENT")]
    hook: Option<hook::Agent>,

    /// With `--hook`, add the last message of the agent to the notification. It
    /// is left out by default, because it can hold code or secrets and a
    /// notification service is not private.
    #[arg(long, requires = "hook")]
    include_message: bool,

    /// Run as an MCP server on standard input and output, with a `notify` tool
    /// for an agent. The URLs are the only places it can send to: the agent
    /// chooses a title, a message and a kind, never a destination, and at most
    /// 10 notifications a minute go out.
    #[arg(
        long,
        conflicts_with_all = ["body", "title", "notification_type", "hook", "include_message", "attach", "dry_run"]
    )]
    mcp: bool,

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
    let cli = Cli::parse();
    // Status 2 blocks the action in a Claude Code hook, so a hook gets 1.
    let failure = if cli.hook.is_some() { 1 } else { 2 };
    match run(cli) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(io::stderr(), "tocsin: {message}");
            ExitCode::from(failure)
        }
    }
}

fn run(mut cli: Cli) -> Result<ExitCode, String> {
    // Codex puts its JSON after the arguments of the command, where it looks
    // like a URL.
    let payload = cli.hook.and_then(|_| {
        let at = cli
            .urls
            .iter()
            .position(|argument| argument.trim_start().starts_with('{'))?;
        Some(cli.urls.remove(at))
    });
    let hooked = match cli.hook {
        Some(agent) => {
            let payload = match payload {
                Some(payload) => payload,
                None => read_stdin()?,
            };
            Some(hook::describe(agent, &payload, cli.include_message)?)
        }
        None => None,
    };

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

    if cli.mcp {
        let mut transport = UreqTransport;
        return mcp::Server::new(&notifier, &mut transport)
            .run(io::stdin().lock(), io::stdout().lock())
            .map(|()| ExitCode::SUCCESS)
            .map_err(|error| format!("MCP: {error}"));
    }

    let attachments = cli
        .attach
        .iter()
        .map(|path| read_attachment(path))
        .collect::<Result<Vec<_>, _>>()?;
    let body = match (cli.body, &hooked) {
        (Some(body), _) => body,
        (None, Some(hooked)) => hooked.body.clone(),
        // A file can be sent on its own, so a terminal is not an error then.
        (None, None) if !attachments.is_empty() && io::stdin().is_terminal() => String::new(),
        (None, None) => read_stdin()?,
    };
    if body.trim().is_empty() && attachments.is_empty() {
        return Err("the message is empty".to_owned());
    }
    let kind = cli
        .notification_type
        .map(Kind::from)
        .or(hooked.as_ref().map(|hooked| hooked.kind))
        .unwrap_or_default();
    let mut notification = Notification::new(body)
        .kind(kind)
        .format(cli.input_format.into());
    if let Some(title) = cli.title.or(hooked.map(|hooked| hooked.title)) {
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
