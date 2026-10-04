//! `tocsin`: send one notification to many services from Apprise-style URLs.

use std::{
    io::{self, IsTerminal, Read, Write},
    process::ExitCode,
};

use clap::{Parser, ValueEnum};
use tocsin::{
    Format, Kind, Notification, Notifier, Outcome, PreparedRequest, Response, Transport,
    TransportError, UreqTransport,
};

/// The most text read from standard input.
const MAX_STDIN_BYTES: u64 = 1 << 20;

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

    let body = match cli.body {
        Some(body) => body,
        None => read_stdin()?,
    };
    if body.trim().is_empty() {
        return Err("the message is empty".to_owned());
    }
    let mut notification = Notification::new(body)
        .kind(cli.notification_type.into())
        .format(cli.input_format.into());
    if let Some(title) = cli.title {
        notification = notification.title(title);
    }

    let report = if cli.dry_run {
        notifier.send(&notification, &mut DryRun)
    } else {
        notifier.send(&notification, &mut UreqTransport)
    };
    for receipt in report.failures() {
        let reason = match receipt.outcome {
            Outcome::Failed(error) => error.to_string(),
            _ => "nothing to send (the URL has no target)".to_owned(),
        };
        let _ = writeln!(io::stderr(), "tocsin: {}: {reason}", receipt.service);
    }
    Ok(if report.is_success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
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
