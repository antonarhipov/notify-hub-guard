use clap::{Args, Parser, Subcommand};
use notify_hub_guard::Result;
use notify_hub_guard::audit::{self, Report};
use notify_hub_guard::history::{self, HistoryClient};
use notify_hub_guard::model::Submission;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "guard",
    version,
    about = "Read-only NotifyHub content audits: unresolved templates and SMS segment estimates"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Audit all retained submissions, or an offline detail fixture.
    Audit {
        #[command(flatten)]
        source: Source,
        /// Write a JSON report atomically. Defaults to stdout.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Explain the findings for one submission (without printing its content).
    Explain {
        #[arg(long)]
        submission: String,
        #[command(flatten)]
        source: Source,
    },
}

#[derive(Args)]
struct Source {
    /// Java base URL. Defaults to http://localhost:8080.
    #[arg(long, conflicts_with = "input")]
    url: Option<String>,
    /// Offline JSON array of Java submission details; makes no HTTP requests.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Refuse larger histories rather than silently truncating them.
    #[arg(long, default_value = "5000", value_parser = clap::value_parser!(u32).range(1..))]
    max_records: u32,
}

impl Source {
    fn load(&self, id: Option<&str>) -> Result<(Vec<Submission>, String)> {
        if let Some(id) = id
            && !history::valid_submission_id(id)
        {
            return Err("Submission ID must be a UUID".into());
        }
        if let Some(path) = &self.input {
            let mut records = history::load_file(path, self.max_records as usize)?;
            if let Some(id) = id {
                records.retain(|s| s.submission_id == id);
                if records.is_empty() {
                    return Err("Submission not found in input".into());
                }
            }
            return Ok((records, format!("file:{}", path.display())));
        }
        let url = self.url.as_deref().unwrap_or("http://localhost:8080");
        let client = HistoryClient::new(url, self.max_records as usize)?;
        let records = match id {
            Some(id) => vec![client.detail(id)?],
            None => client.load()?,
        };
        Ok((records, url.to_owned()))
    }
}

fn write_report(path: &Path, report: &Report) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".guard-{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| -> Result<()> {
        serde_json::to_writer_pretty(&mut file, report)?;
        writeln!(file)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Audit { source, output } => {
            if let (Some(input), Some(output)) = (&source.input, &output)
                && output.exists()
                && input.canonicalize()? == output.canonicalize()?
            {
                return Err("Output must differ from the input fixture".into());
            }
            let (records, source) = source.load(None)?;
            let report = audit::audit(&records, source);
            if let Some(path) = output {
                write_report(&path, &report)?;
                eprintln!(
                    "Audited {} submissions: {} findings; {} estimated SMS segments. Report: {}",
                    report.summary.submissions,
                    report.summary.findings,
                    report.summary.estimated_sms_segments,
                    path.display()
                );
            } else {
                let mut stdout = io::stdout().lock();
                serde_json::to_writer_pretty(&mut stdout, &report)?;
                writeln!(stdout)?;
            }
        }
        Command::Explain { submission, source } => {
            let (records, source) = source.load(Some(&submission))?;
            let report = audit::audit(&records, source);
            let entry = &report.submissions[0];
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "Submission {}", entry.submission_id)?;
            writeln!(
                stdout,
                "Channel: {} | Outcome: {}",
                entry.resolved_channel.as_deref().unwrap_or("unattributed"),
                entry.outcome
            )?;
            if let Some(sms) = &entry.sms {
                writeln!(
                    stdout,
                    "SMS: {:?}, {} units, {} estimated segment(s)",
                    sms.encoding, sms.units, sms.segments
                )?;
            }
            if !entry.body_available {
                writeln!(
                    stdout,
                    "Captured body unavailable; body checks were skipped."
                )?;
            }
            if entry.findings.is_empty() {
                writeln!(stdout, "No findings from the available content.")?;
            }
            for finding in &entry.findings {
                writeln!(
                    stdout,
                    "[{:?}] {:?} ({}): {}",
                    finding.severity, finding.rule, finding.field, finding.explanation
                )?;
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("guard: {error}");
            ExitCode::FAILURE
        }
    }
}
