//! `roundhouse-compare` — cross-runtime HTML rendering comparator.
//!
//! Two servers, one URL list, one diff. The reference server is
//! Rails (the canonical renderer for the ingested fixture); the
//! target server is any roundhouse-emitted runtime (TS-node, rust-
//! axum, …). For each URL, we issue a GET against both, parse the
//! responses into canonicalized DOM trees, and walk the trees in
//! lockstep looking for the first structural divergence.
//!
//! The design target (per user): "same DOM when inspected by JS or
//! CSS". That means:
//!   - tag tree must match exactly (same elements, same children)
//!   - text nodes must match byte-for-byte (whitespace included)
//!   - attribute order is insignificant (canonicalized to sorted)
//!   - HTML comments are insignificant (dropped during canon)
//!   - specific known-variable values (CSRF tokens, asset
//!     fingerprints, session ids) are replaced with placeholders
//!     before comparison, per the ignore-rules config.
//!
//! The tool fails loudly on any other divergence. A new ERB
//! pattern that renders differently between Rails and the target
//! is a bug — either in the emitter's lowering, or in the view
//! helper's output shape.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

mod config;
mod dom;
mod diff;
mod fetch;
mod report;

use config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "roundhouse-compare",
    about = "Cross-runtime HTML rendering comparator",
    version
)]
struct Cli {
    /// Reference server base URL (canonical, e.g. Rails).
    #[arg(long, default_value = "http://localhost:4000")]
    reference: String,

    /// Target server base URL (roundhouse-emitted, e.g. TS-node).
    #[arg(long, default_value = "http://localhost:3000")]
    target: String,

    /// URL paths to compare. Repeatable.
    #[arg(long = "path", num_args = 1.., required = true)]
    paths: Vec<String>,

    /// Config file with ignore rules. Defaults to built-in rules.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Print the full diff on failure (default: just first divergence).
    #[arg(long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = match &cli.config {
        Some(path) => {
            let src = fs::read_to_string(path)
                .with_context(|| format!("read config {path:?}"))?;
            serde_yaml_ng::from_str::<Config>(&src)
                .with_context(|| format!("parse config {path:?}"))?
        }
        None => Config::default(),
    };

    let client = reqwest::blocking::Client::builder()
        .user_agent("roundhouse-compare/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build http client")?;

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut failures: Vec<report::Failure> = Vec::new();

    for path in &cli.paths {
        match compare_one(&client, &cli.reference, &cli.target, path, &config) {
            Ok(None) => {
                println!("  {path} ... \x1b[32mmatch\x1b[0m");
                pass += 1;
            }
            Ok(Some(f)) => {
                println!("  {path} ... \x1b[31mdiffer\x1b[0m");
                failures.push(f);
                fail += 1;
            }
            Err(e) => {
                println!("  {path} ... \x1b[33merror: {e:#}\x1b[0m");
                fail += 1;
            }
        }
    }

    println!();
    println!("{pass}/{total} paths match", total = pass + fail);

    if !failures.is_empty() {
        println!();
        for f in &failures {
            report::print_failure(f, cli.verbose);
        }
        std::process::exit(1);
    }
    Ok(())
}

fn compare_one(
    client: &reqwest::blocking::Client,
    reference: &str,
    target: &str,
    path: &str,
    config: &Config,
) -> Result<Option<report::Failure>> {
    let ref_url = join_url(reference, path);
    let tgt_url = join_url(target, path);

    let ref_resp = fetch::get(client, &ref_url)
        .with_context(|| format!("fetch reference {ref_url}"))?;
    let tgt_resp = fetch::get(client, &tgt_url)
        .with_context(|| format!("fetch target {tgt_url}"))?;

    // Status divergence is its own failure — a 200 vs 404 on the
    // same path is a routing or resource-state mismatch we want to
    // report independently of DOM diff.
    if ref_resp.status != tgt_resp.status {
        return Ok(Some(report::Failure {
            path: path.to_string(),
            kind: report::FailureKind::Status {
                reference: ref_resp.status,
                target: tgt_resp.status,
            },
            reference_body: ref_resp.body,
            target_body: tgt_resp.body,
        }));
    }

    let ref_dom = dom::parse_and_canonicalize(&ref_resp.body, config)
        .context("parse reference HTML")?;
    let tgt_dom = dom::parse_and_canonicalize(&tgt_resp.body, config)
        .context("parse target HTML")?;

    match diff::compare(&ref_dom, &tgt_dom) {
        diff::Outcome::Equal => Ok(None),
        diff::Outcome::Different(div) => Ok(Some(report::Failure {
            path: path.to_string(),
            kind: report::FailureKind::Dom(div),
            reference_body: ref_resp.body,
            target_body: tgt_resp.body,
        })),
    }
}

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{base}{path}")
}

// Re-export for easier test-time construction of the canonical
// attribute map shape.
pub type AttrMap = BTreeMap<String, String>;
