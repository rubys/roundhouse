//! `roundhouse` — compile a Rails source application to a target
//! language. Compiler-shaped CLI: positional input, `-o` output,
//! flags for everything else.
//!
//! Two modes:
//!
//!   `roundhouse --target LANG [INPUT] [-o OUT]`
//!       Single-target transpile. Default INPUT=`.`, default
//!       OUT=`./out/<lang>/`. Writes the emitted file set to OUT;
//!       the set includes a quick-start README (injected by
//!       `project::target_files` unless the target ships its own).
//!
//!   `roundhouse --site [INPUT] [-o OUT]`
//!       Build the full GitHub Pages site: per-target archives
//!       (`<lang>.json`, `.tgz`, `.zip`) plus static landing-page
//!       assets. Default INPUT=`fixtures/real-blog`, default
//!       OUT=`./_site/`.
//!
//! `--target` and `--site` are mutually exclusive; exactly one must
//! be specified. See `--help` for the full flag list.
//!
//! It is also the multi-call binary the release tarball ships: a first
//! argument of `check`, `lsp` or `mcp` hands the rest of the line to
//! that tool (`roundhouse::cli`), so one file on PATH is the compiler,
//! the checker, the Language Server and the MCP server. The per-tool
//! bins (`roundhouse-check`, `roundhouse-lsp`, `roundhouse-mcp`) remain
//! as aliases for the cargo dev loop.

use std::path::PathBuf;
use std::process::ExitCode;

use roundhouse::analyze::{diagnose, Severity};
use roundhouse::ingest::ingest_app;
use roundhouse::project::{self, BuildTarget};

fn usage() -> &'static str {
    "\
Usage: roundhouse --target LANG [INPUT] [-o OUT]
       roundhouse --site [INPUT] [-o OUT]
       roundhouse --target fractal [--file PATH]... [--whole-app] [--report PATH] [INPUT] [-o OUT]
       roundhouse fractal emit [OPTIONS] [APP] [-o OUT]
       roundhouse fractal apply [OVERLAY] [APP]
       roundhouse fractal verify --report PATH [OPTIONS] [OVERLAY] [APP]
       roundhouse --archives LANG,LANG [INPUT] [-o OUT]
       roundhouse check [--continue] [--inventory PATH] [--hotspots] [APP]
       roundhouse lsp
       roundhouse mcp [APP]
       roundhouse --help | --version

Transpile a Rails source application to a target language, or build
the multi-target Pages site.

Subcommands (each takes --help):
  check                Analyze APP (default .) and print the diagnostics;
                       exit 1 on errors. --continue keeps going past
                       unsupported constructs, prints a punch list and a
                       ranked hotspot report. --inventory PATH writes JSON.
  lsp                  Serve the Language Server over stdio.
  mcp                  Serve the MCP tools over stdio for APP (default .).
  fractal              Emit a file overlay, apply it to an existing app,
                       or verify it by booting the app before and after.

Options:
  -t, --target LANG    Transpile target. One of:
                         crystal, csharp, elixir, go, kotlin, python, rust,
                         swift, typescript, typescript-worker, ruby, jruby,
                         spinel
                       Default INPUT=.  Default OUT=./out/<lang>/
                       `fractal` emits only changed files under their app
                       paths. Apply them with `fractal apply`; it replaces
                       only those base files and does not copy the app.
      --file PATH      With --target fractal: a file to compile, relative
                       to INPUT (`app/models/order.rb`). Repeatable.
                       No --file emits no app files. Only these
                       files, and what the registered passes say they
                       need, are ingested.
      --whole-app      With --target fractal: ingest and analyze the
                       whole app instead, as a pass that needs it does.
      --report PATH    With --target fractal: write the build's decisions
                       (compiled files, methods, rewrites, declines) as JSON.
      --site           Build all targets + landing-page assets.
                       Default INPUT=fixtures/real-blog  Default OUT=./_site/
      --archives LIST  Build only comma-separated targets' browse archives.
                       Same defaults as --site; no website or browser demos.
  -o, --output PATH    Output directory.
      --allow-unsupported
                       Don't fail on unsupported-construct gaps: emit a
                       stub at each site, downgrade the diagnostics to
                       warnings, and write the output anyway. Use to see
                       the full inventory of gaps in one run.
      --survey         Don't abort INGEST at the first unsupported
                       construct: record it as a gap, substitute a nil
                       placeholder, and keep going. Prints a
                       deduplicated punch list, and downgrades the
                       diagnostics those gaps caused to notes. Combine
                       with --allow-unsupported to transpile an app that
                       is not fully covered yet.
  -h, --help           Show this help and exit.
  -V, --version        Show version and exit.

Examples:
  roundhouse --target rust                            # → ./out/rust/
  roundhouse --target typescript -o build/ my-app/    # explicit input + output
  roundhouse --site                                   # → ./_site/
  roundhouse -t ruby --survey --allow-unsupported ~/git/mastodon
                                                      # partly-covered app
  roundhouse check --continue ~/git/mastodon          # diagnostics + hotspots
  roundhouse check --continue --inventory /tmp/i.json ~/git/jsp  # JSON worklist
  roundhouse mcp ~/git/campfire                       # an agent's MCP server
"
}

fn main() -> ExitCode {
    roundhouse::stack::run(cli)
}

fn cli() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Multi-call dispatch: the subcommand is only ever the first word,
    // so an INPUT directory that happens to be named `check` still
    // works as `roundhouse --target rust check`.
    match args.first().map(String::as_str) {
        Some("check") => return roundhouse::cli::check(&args[1..], "."),
        Some("lsp") => return roundhouse::cli::lsp(&args[1..]),
        Some("mcp") => return roundhouse::cli::mcp(&args[1..]),
        Some("fractal") => return roundhouse::cli::fractal::run(&args[1..]),
        _ => {}
    }
    match parse_args(args) {
        Ok(Action::Help) => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(Action::Version) => {
            println!("roundhouse {}", roundhouse::version::describe());
            ExitCode::SUCCESS
        }
        Ok(Action::Fractal { files, input, out, survey, report, whole_app }) => {
            match roundhouse::cli::fractal::emit(&files, &input, &out, survey, report.as_deref(), whole_app) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("roundhouse: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Action::Transpile { target, input, out, allow_unsupported, survey }) => {
            match run_transpile(target, &input, &out, allow_unsupported, survey) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("roundhouse: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Action::Site { input, out }) => match project::build_site(&input, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("roundhouse: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Action::Archives { targets, input, out }) => match project::build_archives(&input, &out, &targets) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("roundhouse: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("roundhouse: {e}");
            eprintln!();
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

enum Action {
    Help,
    Version,
    Transpile {
        target: BuildTarget,
        input: PathBuf,
        out: PathBuf,
        allow_unsupported: bool,
        survey: bool,
    },
    Site {
        input: PathBuf,
        out: PathBuf,
    },
    Fractal {
        files: Vec<String>,
        input: PathBuf,
        out: PathBuf,
        survey: bool,
        report: Option<PathBuf>,
        whole_app: bool,
    },
    Archives {
        targets: Vec<BuildTarget>,
        input: PathBuf,
        out: PathBuf,
    },
}

fn parse_args(args: Vec<String>) -> Result<Action, String> {
    let mut target: Option<BuildTarget> = None;
    let mut site = false;
    let mut archives = None;
    let mut out: Option<PathBuf> = None;
    let mut allow_unsupported = false;
    let mut survey = false;
    let mut fractal = false;
    let mut files: Vec<String> = Vec::new();
    let mut report: Option<PathBuf> = None;
    let mut whole_app = false;
    let mut positional: Vec<String> = Vec::new();

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-V" | "--version" => return Ok(Action::Version),
            "--site" => site = true,
            "--archives" => {
                let value = iter.next().ok_or("--archives requires a target list")?;
                let targets = value.split(',').map(|name| {
                    BuildTarget::from_str(name)
                        .filter(|t| BuildTarget::ALL.contains(t) || BuildTarget::SITE_ONLY.contains(t))
                        .ok_or_else(|| format!("unknown archive target '{name}'"))
                }).collect::<Result<Vec<_>, _>>()?;
                archives = Some(targets);
            }
            "--allow-unsupported" => allow_unsupported = true,
            "--survey" => survey = true,
            "-t" | "--target" => {
                let v = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                if v == "fractal" {
                    fractal = true;
                } else {
                    target = Some(parse_target(&v)?);
                }
            }
            "--file" => files.push(iter.next().ok_or("--file requires a path")?),
            "--whole-app" => whole_app = true,
            "--report" => report = Some(PathBuf::from(iter.next().ok_or("--report requires a path")?)),
            "-o" | "--output" => {
                let v = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                out = Some(PathBuf::from(v));
            }
            "--target=fractal" => fractal = true,
            s if s.starts_with("--file=") => files.push(s["--file=".len()..].to_string()),
            s if s.starts_with("--report=") => report = Some(PathBuf::from(&s["--report=".len()..])),
            s if s.starts_with("--target=") => {
                target = Some(parse_target(&s["--target=".len()..])?);
            }
            s if s.starts_with("--output=") => {
                out = Some(PathBuf::from(&s["--output=".len()..]));
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown flag: {s}"));
            }
            _ => positional.push(arg),
        }
    }

    if positional.len() > 1 {
        return Err(format!(
            "expected at most one positional INPUT, got {}: {}",
            positional.len(),
            positional.join(" ")
        ));
    }

    if fractal {
        if target.is_some() || site || archives.is_some() || allow_unsupported {
            return Err("--target fractal cannot be combined with another --target, --site, --archives or --allow-unsupported".into());
        }
        return Ok(Action::Fractal {
            files,
            input: positional.pop().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")),
            out: out.unwrap_or_else(|| PathBuf::from("out/fractal")),
            survey,
            report,
            whole_app,
        });
    }
    if !files.is_empty() || report.is_some() || whole_app {
        return Err("--file, --whole-app and --report are only meaningful with --target fractal".into());
    }
    if let Some(targets) = archives {
        if target.is_some() || site || allow_unsupported || survey {
            return Err("--archives cannot be combined with --target, --site, --survey or --allow-unsupported".into());
        }
        return Ok(Action::Archives {
            targets,
            input: positional.pop().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("fixtures/real-blog")),
            out: out.unwrap_or_else(|| PathBuf::from("_site")),
        });
    }
    match (target, site) {
        (Some(_), true) => Err("--target and --site are mutually exclusive".into()),
        (None, false) => Err("one of --target LANG or --site is required".into()),
        (Some(target), false) => {
            let input = positional
                .pop()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let out =
                out.unwrap_or_else(|| PathBuf::from("out").join(target.as_str()));
            Ok(Action::Transpile { target, input, out, allow_unsupported, survey })
        }
        (None, true) => {
            let input = positional
                .pop()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("fixtures/real-blog"));
            let out = out.unwrap_or_else(|| PathBuf::from("_site"));
            Ok(Action::Site { input, out })
        }
    }
}

fn parse_target(s: &str) -> Result<BuildTarget, String> {
    match BuildTarget::from_str(s) {
        Some(BuildTarget::Blog) => Err(format!(
            "--target blog is not a transpile target (use --site to include the source archive)"
        )),
        Some(t) => Ok(t),
        None => {
            let valid = BuildTarget::TRANSPILE
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!("unknown target '{s}'. Valid: {valid}"))
        }
    }
}

fn run_transpile(
    target: BuildTarget,
    input: &std::path::Path,
    out: &std::path::Path,
    allow_unsupported: bool,
    survey: bool,
) -> Result<(), String> {
    if !input.exists() {
        return Err(format!("input {} does not exist", input.display()));
    }
    // Survey mode: record unsupported constructs as gaps and substitute
    // a nil placeholder instead of aborting at the first one. Without
    // it the ingester is fail-fast, which is right for a supported app
    // and useless for an unsupported one — mastodon dies on a single
    // `alias` in `account.rb` after 0.02s, having transpiled nothing.
    // The /ide/ page has always done this (its wasm entry calls
    // `survey::activate()`), so before this flag the BROWSER could
    // transpile an app the command line could not.
    if survey {
        roundhouse::ingest::survey::activate();
    }
    // Ingest inside a parse-diagnostic scope so Prism syntax errors —
    // which the error-recovering parser otherwise drops on the floor —
    // are collected and routed through the same report as emit gaps.
    let (app_result, parse_diags) =
        roundhouse::ingest::prism::scope(|| ingest_app(input));
    let mut survey_gaps =
        if survey { roundhouse::ingest::survey::drain() } else { Vec::new() };
    let mut app = match app_result {
        Ok(app) => app,
        Err(e) => {
            // Ingest failed outright. Surface any syntax errors first: a
            // malformed file is usually the root cause of the construct
            // ingest then choked on. The sources table isn't populated on
            // the error path, so these render message-only.
            for d in &parse_diags {
                eprintln!("roundhouse: {}", d.render(&[]));
            }
            // Survey mode can still fail — on I/O, or on a construct
            // outside `ingest_expr`'s interception. Print what it did
            // collect rather than dropping it.
            if !survey_gaps.is_empty() {
                eprintln!();
                eprint!("{}", roundhouse::ingest::survey::render_report(&survey_gaps));
            } else if !survey {
                eprintln!(
                    "roundhouse: hint: re-run with --survey to record unsupported \
                     constructs as a punch list and keep going"
                );
            }
            return Err(format!("ingest {}: {e}", input.display()));
        }
    };
    // An ActiveRecord class outside app/models (a migration helper's own
    // query class, a gem's model) is reported at ingest for the targets
    // that lower models, which don't model it (#1278). Futamura runs on
    // Rails and the Roda conversion on Sequel, which load it themselves;
    // under --survey it is a recorded gap, not a failed run.
    // Kept apart from `survey_gaps` until the punch list prints: the
    // class did ingest, so its file is not a skipped one for attribution.
    let mut unmodelled_gaps: Vec<roundhouse::ingest::IngestError> = Vec::new();
    let mut parse_diags = parse_diags;
    let unmodelled_record = |d: &roundhouse::diagnostic::Diagnostic| {
        matches!(&d.kind, roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == roundhouse::ingest::UNMODELLED_RECORD)
    };
    if matches!(target, BuildTarget::Futamura | BuildTarget::Roda) {
        parse_diags.retain(|d| !unmodelled_record(d));
    } else if survey {
        let (recorded, kept): (Vec<_>, Vec<_>) = parse_diags.into_iter().partition(|d| unmodelled_record(d));
        parse_diags = kept;
        for d in recorded {
            let file = roundhouse::ide::source(&app, d.span.file).map(|s| s.path.clone()).unwrap_or_default();
            unmodelled_gaps.push(roundhouse::ingest::IngestError::Unsupported { file, message: d.message.clone() });
        }
    }
    // Analyze + the post-analyze shared lowerings — type-directed IR
    // rewrites every target consumes (blank-predicate grounding,
    // `Time.current`). The returned residue diagnostics (sites a pass
    // had to leave dynamic) join the analyze warnings below: same
    // collapse-to-count print policy, same --allow-unsupported
    // full-inventory behavior.
    //
    // Exception: the Roda conversion target is source-to-source from
    // the INGEST-shape IR — lowering would rewrite the controller
    // bodies into runtime vocabulary (SQL-folded queries, Views::
    // calls), the wrong altitude to re-idiomize into Sequel/Roda from.
    // See `emit::roda`. Futamura needs the types but none of the
    // lowerings: those rewrite into the runtime's vocabulary, and its
    // specializations call Rails (docs/pipeline/specialization.md).
    // Futamura's whole-route replacement needs the `ruby` target's
    // lowering of the same app, from the ingest shape; built before the
    // analysis below mutates `app`.
    let whole_route = if target == BuildTarget::Futamura {
        Some(project::futamura_whole_route(&app, input, &survey_gaps)?)
    } else {
        None
    };
    let lower_diags = if target == BuildTarget::Roda {
        Vec::new()
    } else if target == BuildTarget::Futamura {
        roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
        Vec::new()
    } else {
        roundhouse::session::analyze_and_lower_for(&mut app, target)
    };

    // Analyze-time diagnostics — the same type errors roundhouse-check
    // reports (dispatch failures, unresolved ivars, incompatible ops).
    // Emitters only stub *annotated* sites (body-typer `expr.diagnostic`);
    // walker-found errors would otherwise pass through into target code
    // that fails later in tsc/cargo/runtime with a worse message, so
    // they print and gate here alongside the emit-gap inventory.
    // Roda still reports recovered structural route errors. Its type
    // analysis never ran, so analyzer-only diagnostics would be noise.
    // Futamura reports nothing: whatever it cannot specialize is
    // residue that Rails runs, not an unsupported construct.
    let mut analyze_diags = match target {
        BuildTarget::Roda => app.routes.diagnostics.clone(),
        BuildTarget::Futamura => Vec::new(),
        _ => diagnose(&app),
    };
    analyze_diags.extend(lower_diags);

    // CRuby can carry an external include only when the lockfile uniquely
    // identifies its gem and the defining source has a matching literal
    // top-level require. Keep check's source diagnostic and all strict-target
    // refusals unchanged.
    if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby) {
        let (external_namespaces, supported_includes) = if target == BuildTarget::Ruby {
            (roundhouse::gems::locked_external_gem_namespaces(&app), roundhouse::gems::statically_resolved_external_includes(&app))
        } else {
            (roundhouse::gems::native_extension_gem_namespaces(&app), Default::default())
        };
        let mut external_roots = roundhouse::gems::ExternalRootIndex::new(&external_namespaces);
        analyze_diags.retain(|diagnostic| match &diagnostic.kind {
            roundhouse::diagnostic::DiagnosticKind::Unsupported {
                construct, detail, ..
            } if construct.as_str() == "include" => !supported_includes.contains_key(detail),
            roundhouse::diagnostic::DiagnosticKind::Unsupported {
                construct, detail, ..
            } if construct.as_str() == "constant" => !external_roots.names_any(detail),
            _ => true,
        });
    }

    // A residue whose own text says the construct is "unsupported at
    // strict-target emit" has to fail the emit it predicts that for.
    // The ledger runs once, target-agnostically, inside the shared
    // post-analyze pipeline, so it files the same Warning whichever
    // target is being built — right for the ruby-family targets, where
    // the chain executes on the runtime `ActiveRecord::Relation`, and a
    // silent write of uncompilable output for the ones that have no
    // Relation at all (issue #76). The target is known HERE, so the
    // severity is decided here.
    roundhouse::lower::relation_residue::elevate_dynamic_relation_for_target(
        &mut analyze_diags,
        target,
    );
    roundhouse::lower::params_residue::elevate_nested_params_for_target(&mut analyze_diags, target);
    // queue_adapter :sidekiq is honored on the Ruby family only; the other
    // targets say once that the app's jobs run in process there.
    analyze_diags.extend(roundhouse::lower::job_payload::sidekiq_adapter_target_warning(&app, target));

    // A diagnostic whose root cause is a recorded ingest gap is OUR
    // coverage problem, not the app's: the nil placeholder survey mode
    // substituted is what the analyzer then failed to resolve. Downgrade
    // those to notes with the cause attached, so the error count means
    // "findings" rather than "shadows of the gaps listed below".
    roundhouse::analyze::attribution::attribute_ingest_gaps(
        &mut analyze_diags,
        &app,
        &survey_gaps,
    );
    if survey {
        roundhouse::analyze::attribution::attribute_analysis_gaps(
            &mut analyze_diags,
            &app,
            &mut survey_gaps,
        );
    }

    // Emit inside a diagnostic scope so unsupported-construct gaps in
    // any lowerer/emitter are collected rather than lost (issue #28).
    // Each gap still degrades to a stub in the emitted output, so a
    // single run surfaces the whole inventory instead of dying on the
    // first one.
    let emit = || {
        roundhouse::emit::diagnostics::scope(|| match &whole_route {
            Some(wr) => project::futamura_target_files(&app, input, Some(wr)),
            None => project::target_files(&app, input, target),
        })
    };
    let audit = std::env::var_os("ROUNDHOUSE_AUDIT").is_some();
    // Not recorded on every run: the record clones each rendered class's IR, which only the audit reads.
    let ((files_result, mut diags), rendered) =
        if audit { roundhouse::emit::ruby::record_rendered(emit) } else { (emit(), Vec::new()) };
    if audit
        && let Ok(files) = &files_result
    {
        let started = std::time::Instant::now();
        let findings = roundhouse::emit::audit::audit(&rendered, files);
        for d in &findings {
            eprintln!("{}", d.render(&app.sources));
        }
        eprintln!("roundhouse: audit: {} send(s) resolve to no emitted method ({:?})", findings.len(), started.elapsed());
    }

    // Same attribution pass as `analyze_diags` above, run again here:
    // model-to-library lowering (where an unresolved `alias` is caught,
    // `lower::model_to_library::resolve_model_aliases`) runs a SECOND
    // time inside `target_files` itself, to build the actual emitted
    // method bodies — `analyze_and_lower`'s own lowering pass above
    // isn't wrapped in `emit::diagnostics::scope`, so a push from
    // there is silently dropped, and this is the first (and only)
    // place such a diagnostic is ever collected. Without this second
    // call, `--survey` (no `--allow-unsupported`) still hard-failed on
    // an unresolved alias — the Error severity from `diags` alone
    // gates the exit code below — and it never reached the gap
    // ledger, which undercounted the app's real gap total.
    if survey {
        roundhouse::analyze::attribution::attribute_analysis_gaps(
            &mut diags,
            &app,
            &mut survey_gaps,
        );
    }

    // Prism syntax errors recorded during ingest join the emit-gap
    // inventory: same `Diagnostic` shape, same print/policy/gate
    // treatment. They lead the list — earliest phase, and usually the
    // root cause of any downstream emit noise on the recovered AST. The
    // materialized `files` are deferred to after the gate (below) so a
    // gate failure reports every diagnostic before the run aborts.
    diags.splice(0..0, parse_diags);

    // Policy: errors (unsupported constructs and type errors alike) fail
    // the transpile cleanly by default. With --allow-unsupported they
    // downgrade to warnings and the output is written anyway, so the
    // user can inspect the full inventory in one pass.
    if allow_unsupported {
        for d in diags.iter_mut().chain(analyze_diags.iter_mut()) {
            if d.severity == Severity::Error {
                d.severity = Severity::Warning;
            }
        }
    }
    // The emit sink is always small — print it all. Analyze warnings
    // (gradual_untyped above all) can run to hundreds on a large app,
    // so by default they collapse to a count; --allow-unsupported asks
    // for the full inventory and gets every line.
    // The initializer ledger is a warning per unconsumed boot statement,
    // which can run to hundreds on a large app: like the analyze warnings
    // below it collapses to a count unless `--survey` (which prints the
    // ledger section) or `--allow-unsupported` asks for the inventory.
    let list_ledger = survey || allow_unsupported;
    let is_ledger = |d: &roundhouse::diagnostic::Diagnostic| {
        matches!(d.kind, roundhouse::diagnostic::DiagnosticKind::UnconsumedInitializer { .. })
    };
    let mut ledger_warnings = 0usize;
    for d in &diags {
        if is_ledger(d) {
            ledger_warnings += 1;
            if !list_ledger {
                continue;
            }
        }
        eprintln!("roundhouse: {}", d.render(&app.sources));
    }
    if ledger_warnings > 0 && !list_ledger {
        eprintln!(
            "roundhouse: {ledger_warnings} unconsumed initializer statement(s) (warnings) — \
             rerun with --survey to list them"
        );
    }
    let mut suppressed_warnings = 0usize;
    for d in &analyze_diags {
        if d.severity == Severity::Error || allow_unsupported {
            eprintln!("roundhouse: {}", d.render(&app.sources));
        } else {
            suppressed_warnings += 1;
        }
    }
    if suppressed_warnings > 0 {
        eprintln!(
            "roundhouse: {suppressed_warnings} analyze warning(s) not shown — \
             rerun with --allow-unsupported to list them"
        );
    }

    // The initializer ledger section, beside the punch list but NOT part of
    // its "ingest gap(s)" count: a ledger line is a statement nothing reads,
    // not a construct ingest failed to parse, and counting it as a gap would
    // let `attribute_ingest_gaps` downgrade real errors in the same file.
    if survey && ledger_warnings > 0 {
        eprintln!();
        eprint!("{}", roundhouse::ingest::initializer_ledger::render_summary(&diags));
    }

    // The punch list, before the gate: on an app that fails the gate
    // this is the most useful thing in the run, and printing it after
    // the early `return Err` would mean never printing it at all.
    survey_gaps.append(&mut unmodelled_gaps);
    if !survey_gaps.is_empty() {
        // `attribute_analysis_gaps` runs twice (once over `analyze_diags`,
        // once over the emit-phase `diags` — see the call sites above) and
        // an attributed note can land in EITHER vector, or only in `diags`
        // when the construct is caught solely during emit (an unresolved
        // model alias, today's only such case — `resolve_model_aliases`'s
        // push only ever reaches `diags`' `emit::diagnostics::scope`, never
        // `analyze_diags`). Counting `analyze_diags` alone could report 0
        // attributed notes for a run that plainly has some. Count both,
        // deduped by (span, message) so a construct caught in both passes
        // for the same source line isn't counted twice.
        let mut seen = std::collections::HashSet::new();
        let notes = analyze_diags
            .iter()
            .chain(diags.iter())
            .filter(|d| d.severity == Severity::Info)
            .filter(|d| seen.insert((d.span, d.message.clone())))
            .count();
        eprintln!();
        eprint!("{}", roundhouse::ingest::survey::render_report(&survey_gaps));
        eprintln!(
            "roundhouse: {} ingest gap(s) recorded; {notes} diagnostic(s) attributed to them",
            survey_gaps.len()
        );
    }

    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let type_errors = analyze_diags.iter().filter(|d| d.severity == Severity::Error).count();
    if errors + type_errors > 0 {
        // A project-boundary refusal (`target_files` returned `Err`, e.g.
        // a Date column on a target without a date-only runtime) fails
        // with or without the flag, so suggesting it would be false.
        // Name the refusal instead (issue #303).
        if let Err(e) = &files_result {
            return Err(format!(
                "{errors} unsupported/syntax error(s), {type_errors} type error(s) — {e}"
            ));
        }
        return Err(format!(
            "{errors} unsupported/syntax error(s), {type_errors} type error(s) — rerun \
             with --allow-unsupported to write the output anyway"
        ));
    }

    // Gate passed (or every error was downgraded): materialize the emit
    // output, propagating any hard emit failure now.
    let files = files_result?;
    project::write_to_dir(&files, out)?;
    // The app's own binary files — images, fonts, binary test fixtures.
    // They never became `EmittedFile`s (that type's content is a
    // `String`), so before this they were dropped without a word.
    let assets = project::write_binary_assets(&app.binary_assets, &files, out)?;
    if target == BuildTarget::Futamura {
        project::copy_exec_bits(input, &files, out)?;
    }
    eprintln!(
        "roundhouse: emitted {} files to {} ({})",
        files.len() + assets,
        out.display(),
        target.as_str()
    );
    Ok(())
}
