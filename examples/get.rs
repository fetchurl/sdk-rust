//! Example CLI client for fetchurl, similar to the Go `fetchurl get` command.
//!
//! Usage:
//!   cargo run --example get -- sha256 HASH --url URL1 --url URL2 -o output.tar.gz
//!
//! Set FETCHURL_SERVER to use cache servers:
//!   FETCHURL_SERVER='"http://cache:8080/api/fetchurl"' cargo run --example get -- sha256 HASH --url URL

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use clap::Parser;
use fetchurl_sdk as fetchurl;

/// Connect timeout for each attempt (matches integration-test order of magnitude).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-read timeout so a stalled body does not hang the CLI forever.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[command(
    name = "fetchurl-get",
    about = "Fetch a file using content-addressable storage"
)]
struct Cli {
    /// Hash algorithm (sha1, sha256, sha512)
    algo: String,

    /// Expected hash in hex
    hash: String,

    /// Source URLs (can be specified multiple times)
    #[arg(long = "url")]
    urls: Vec<String>,

    /// Output file path (defaults to stdout)
    #[arg(short, long)]
    output: Option<String>,
}

/// Sibling temp path so a failed or partial download never replaces the target.
fn partial_path(final_path: &str) -> PathBuf {
    let path = Path::new(final_path);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output");
    parent.join(format!(".{name}.fetchurl-partial.{}", std::process::id()))
}

fn main() {
    let cli = Cli::parse();

    let mut session = match fetchurl::FetchSession::new(&cli.algo, &cli.hash, &cli.urls) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    // File destinations write to a same-dir temp file and rename only after
    // hash verification succeeds, so readers never see a partial target.
    let tmp_path = cli.output.as_ref().map(|p| partial_path(p));
    let mut out: Box<dyn Write> = match (&cli.output, &tmp_path) {
        (Some(path), Some(tmp)) => match File::create(tmp) {
            Ok(f) => Box::new(f),
            Err(e) => {
                eprintln!("error: cannot create temporary file for {path}: {e}");
                process::exit(1);
            }
        },
        _ => Box::new(io::stdout()),
    };

    // Bound each attempt so a hung peer cannot wedge the process indefinitely.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();

    while let Some(attempt) = session.next_attempt() {
        eprintln!("trying: {}", attempt.url());

        let mut req = agent.get(attempt.url());
        for (key, value) in attempt.headers() {
            req = req.set(key, value);
        }

        let response = match req.call() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  failed: {e}");
                continue;
            }
        };

        let mut reader = response.into_reader();
        let mut verifier = session.verifier(&mut *out);

        if let Err(e) = io::copy(&mut reader, &mut verifier) {
            eprintln!("  download error: {e}");
            if verifier.bytes_written() > 0 {
                session.report_partial();
                break;
            }
            continue;
        }

        let written = verifier.bytes_written();
        match verifier.finish() {
            Ok(_) => {
                session.report_success();
                break;
            }
            Err(e) => {
                eprintln!("  verification failed: {e}");
                if written > 0 {
                    session.report_partial();
                    break;
                }
            }
        }
    }

    // Close the writer before rename/unlink.
    drop(out);

    if !session.succeeded() {
        eprintln!("error: failed to fetch from any source");
        if let Some(tmp) = &tmp_path {
            let _ = fs::remove_file(tmp);
        }
        process::exit(1);
    }

    if let (Some(path), Some(tmp)) = (&cli.output, &tmp_path) {
        if let Err(e) = fs::rename(tmp, path) {
            eprintln!("error: cannot finalize {path}: {e}");
            let _ = fs::remove_file(tmp);
            process::exit(1);
        }
    }
}
