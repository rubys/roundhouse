//! `roundhouse-ast` — structural inspection of the Roundhouse pipeline.
//!
//! Takes a Ruby snippet or an ERB / Ruby file, runs it through one or more
//! pipeline stages (Prism parse → ERB compile → ingest → Ruby emit), and
//! prints the intermediate form. When a new ingest gap or round-trip
//! divergence shows up, this is the tool that makes it visible without
//! writing a throwaway test.
//!
//! Typical uses:
//!
//! ```text
//! roundhouse-ast -e '[:a, :b]'                  # see IR for a snippet
//! roundhouse-ast --stage prism -e '@x.y do end' # see what Prism produced
//! roundhouse-ast --stage compile-erb view.erb   # see compiler's Ruby output
//! roundhouse-ast --stage emit-ruby -e '"a#{x}"' # round-trip one expression
//! roundhouse-ast --round-trip -e 'a + b'        # ingest→emit→ingest diff
//! roundhouse-ast --stages -e 'form.label :x'    # run every stage, dump each
//! ```
//!
//! Output is pretty-printed JSON for IR stages (the `Expr` type already
//! derives `Serialize`) and plain text for the source-level stages
//! (`compile-erb`, `emit-ruby`).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use roundhouse::emit::ruby as ruby_emit;
use roundhouse::erb;
use roundhouse::expr::Expr;
use roundhouse::ingest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Prism,
    CompileErb,
    Ingest,
    EmitRuby,
}

impl Stage {
    fn from_name(s: &str) -> Option<Self> {
        match s {
            "prism" => Some(Self::Prism),
            "compile-erb" | "compile_erb" => Some(Self::CompileErb),
            "ingest" => Some(Self::Ingest),
            "emit-ruby" | "emit_ruby" => Some(Self::EmitRuby),
            _ => None,
        }
    }

    fn as_label(self) -> &'static str {
        match self {
            Self::Prism => "prism",
            Self::CompileErb => "compile-erb",
            Self::Ingest => "ingest",
            Self::EmitRuby => "emit-ruby",
        }
    }
}

#[derive(Debug)]
enum Input {
    Inline(String),
    File(PathBuf),
}

struct Args {
    input: Input,
    /// Explicit `--erb`, or auto-detected from file extension.
    force_erb: bool,
    /// `--stage NAME`. Default: `ingest`.
    stage: Stage,
    /// `--stages` — dump every stage in pipeline order.
    all_stages: bool,
    /// `--round-trip` — ingest → emit Ruby → re-ingest; diff IRs.
    round_trip: bool,
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(args) => match run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Err(ArgError::Help) => {
            print_usage();
            ExitCode::SUCCESS
        }
        Err(ArgError::Bad(msg)) => {
            eprintln!("error: {msg}\n");
            print_usage();
            ExitCode::from(2)
        }
    }
}

enum ArgError {
    Help,
    Bad(String),
}

fn parse_args() -> Result<Args, ArgError> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut inline: Option<String> = None;
    let mut file: Option<PathBuf> = None;
    let mut force_erb = false;
    let mut stage = Stage::Ingest;
    let mut all_stages = false;
    let mut round_trip = false;

    let mut i = 0;
    while i < raw.len() {
        let a = &raw[i];
        match a.as_str() {
            "-h" | "--help" => return Err(ArgError::Help),
            "-e" => {
                i += 1;
                let v = raw.get(i).ok_or_else(|| {
                    ArgError::Bad("-e requires a value".into())
                })?;
                inline = Some(v.clone());
            }
            "--erb" => force_erb = true,
            "--stage" => {
                i += 1;
                let v = raw.get(i).ok_or_else(|| {
                    ArgError::Bad("--stage requires a value".into())
                })?;
                stage = Stage::from_name(v).ok_or_else(|| {
                    ArgError::Bad(format!(
                        "unknown stage: {v}. Valid: prism, compile-erb, ingest, emit-ruby"
                    ))
                })?;
            }
            "--stages" => all_stages = true,
            "--round-trip" => round_trip = true,
            s if s.starts_with('-') => {
                return Err(ArgError::Bad(format!("unknown flag: {s}")));
            }
            _ => {
                if file.is_some() {
                    return Err(ArgError::Bad(
                        "only one positional file argument allowed".into(),
                    ));
                }
                file = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }

    let input = match (inline, file) {
        (Some(s), None) => Input::Inline(s),
        (None, Some(p)) => Input::File(p),
        (Some(_), Some(_)) => {
            return Err(ArgError::Bad("use either -e or a file path, not both".into()));
        }
        (None, None) => {
            return Err(ArgError::Bad(
                "no input — pass `-e CODE` or a file path".into(),
            ));
        }
    };

    Ok(Args { input, force_erb, stage, all_stages, round_trip })
}

fn print_usage() {
    eprintln!(
        "roundhouse-ast — inspect the Roundhouse pipeline

USAGE:
    roundhouse-ast [OPTIONS] (-e CODE | FILE)

INPUT (choose one):
    -e CODE           Inline Ruby source
    FILE              Path to a `.rb` or `.erb` file
                      (`.erb` auto-applies the ERB compiler)

OPTIONS:
    --erb             Force ERB compilation on inline input
    --stage NAME      Which stage's output to print (default: ingest)
                      Valid: prism, compile-erb, ingest, emit-ruby
    --stages          Run every stage and print each in pipeline order
    --round-trip      ingest → emit Ruby → re-ingest; diff IR,
                      exit non-zero if divergent
    -h, --help        Print this help"
    );
}

fn run(args: &Args) -> Result<(), String> {
    let (raw_source, is_erb) = load_source(&args.input, args.force_erb)?;

    // ERB → compiled Ruby. Ruby input passes through.
    let compiled = if is_erb {
        erb::compile_erb(&raw_source)
    } else {
        raw_source.clone()
    };

    if args.round_trip {
        if is_erb {
            return Err(
                "--round-trip is only supported for plain Ruby snippets; ERB \
                 reconstruction was removed when the spinel emit pipeline \
                 dropped the parsed-AST emitter."
                    .into(),
            );
        }
        return run_round_trip_expr(&compiled);
    }

    let stages: Vec<Stage> = if args.all_stages {
        let mut s = Vec::new();
        s.push(Stage::Prism);
        if is_erb {
            s.push(Stage::CompileErb);
        }
        s.push(Stage::Ingest);
        s.push(Stage::EmitRuby);
        s
    } else {
        vec![args.stage]
    };

    for (idx, stage) in stages.iter().enumerate() {
        if args.all_stages {
            if idx > 0 {
                println!();
            }
            println!("=== {} ===", stage.as_label());
        }
        emit_stage(*stage, &compiled, is_erb)?;
    }

    Ok(())
}

fn load_source(input: &Input, force_erb: bool) -> Result<(String, bool), String> {
    match input {
        Input::Inline(s) => Ok((s.clone(), force_erb)),
        Input::File(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let is_erb = force_erb
                || path.extension().and_then(|e| e.to_str()) == Some("erb");
            Ok((text, is_erb))
        }
    }
}

fn emit_stage(
    stage: Stage,
    compiled_ruby: &str,
    is_erb: bool,
) -> Result<(), String> {
    match stage {
        Stage::Prism => {
            // Prism's Debug is our best available structural view.
            // `{:#?}` gives one-node-per-line output.
            let result = ruby_prism::parse(compiled_ruby.as_bytes());
            let root = result.node();
            println!("{:#?}", root);
            Ok(())
        }
        Stage::CompileErb => {
            if !is_erb {
                return Err(
                    "compile-erb stage requires ERB input (.erb file or --erb)".into(),
                );
            }
            print!("{compiled_ruby}");
            Ok(())
        }
        Stage::Ingest => {
            let expr = ingest_snippet(compiled_ruby)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&expr)
                    .map_err(|e| format!("serialize: {e}"))?
            );
            Ok(())
        }
        Stage::EmitRuby => {
            let expr = ingest_snippet(compiled_ruby)?;
            println!("{}", ruby_emit::emit_expr(&expr));
            Ok(())
        }
    }
}

/// Parse `source` as a Ruby program and ingest the resulting statements.
/// Multi-statement programs land as `ExprNode::Seq`; single-statement
/// programs collapse to the inner node.
fn ingest_snippet(source: &str) -> Result<Expr, String> {
    let result = ruby_prism::parse(source.as_bytes());
    let root = result.node();
    let program = root
        .as_program_node()
        .ok_or_else(|| "input is not a Ruby program".to_string())?;
    let stmts = program.statements().as_node();
    ingest::ingest_expr(&stmts, "<input>").map_err(|e| e.to_string())
}

/// Snippet round-trip: compile-or-ingest once, emit Ruby (via the
/// general `emit_expr`), re-ingest, compare IRs. Suitable for Ruby
/// input where the emit target is plain Ruby.
fn run_round_trip_expr(compiled_ruby: &str) -> Result<(), String> {
    let first = ingest_snippet(compiled_ruby)?;
    let emitted = ruby_emit::emit_expr(&first);
    let second = ingest_snippet(&emitted).map_err(|e| {
        format!("re-ingest of emitted Ruby failed: {e}\n--- emitted Ruby ---\n{emitted}")
    })?;
    compare_ir("ingest → emit-ruby → ingest", &first, &second, &emitted)
}

fn compare_ir(
    pipeline: &str,
    first: &Expr,
    second: &Expr,
    emitted: &str,
) -> Result<(), String> {
    if first == second {
        println!("ok: IR stable across {pipeline}");
        return Ok(());
    }
    let first_json = serde_json::to_string_pretty(first)
        .map_err(|e| format!("serialize first: {e}"))?;
    let second_json = serde_json::to_string_pretty(second)
        .map_err(|e| format!("serialize second: {e}"))?;
    let diff = unified_diff(&first_json, &second_json);
    let mut out = String::new();
    writeln!(out, "IR diverged across {pipeline}").ok();
    writeln!(out, "--- emitted ---\n{emitted}").ok();
    writeln!(out, "--- diff (first → second) ---\n{diff}").ok();
    Err(out)
}

/// Minimal line-diff: for each line that differs, print `- first` and
/// `+ second` with a small amount of context. The output isn't fancy —
/// just enough to point at where the divergence lives. Falls back to
/// full dump when the line counts differ.
fn unified_diff(a: &str, b: &str) -> String {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();
    if a_lines.len() != b_lines.len() {
        let mut s = String::new();
        writeln!(
            s,
            "(line count differs: {} → {}; showing full bodies)",
            a_lines.len(),
            b_lines.len()
        )
        .ok();
        writeln!(s, "--- first ---\n{a}\n--- second ---\n{b}").ok();
        return s;
    }
    let mut s = String::new();
    for (i, (l, r)) in a_lines.iter().zip(b_lines.iter()).enumerate() {
        if l != r {
            writeln!(s, "@@ line {} @@", i + 1).ok();
            writeln!(s, "- {l}").ok();
            writeln!(s, "+ {r}").ok();
        }
    }
    if s.is_empty() {
        s.push_str("(no line-level differences — structural divergence elsewhere)");
    }
    s
}
