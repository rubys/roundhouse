//! MCP server for LLM agents — Rung 2 of roundhouse#57.
//!
//! Exposes the whole-app type analysis as Model Context Protocol tools, so
//! a coding agent working on a Rails app gets the static-type feedback
//! loop that statically-typed languages give for free and Rails never
//! has: "what's the type here", "can this be nil", "what won't type-check",
//! and — uniquely — "what won't survive ejection to Go/Rust/…". All of it
//! with no app boot, sub-second, on broken code, side-effect-free.
//!
//! Transport is hand-rolled JSON-RPC 2.0 over stdio (MCP's newline-
//! delimited framing), built on the `serde_json` already in the tree — no
//! tokio, no `rmcp`. That matches the project's leanness and the Rung 1
//! decision: a stdio-per-process server has no use for an async runtime.
//!
//! Unlike the LSP (which tracks open editor buffers), an agent edits files
//! on disk with its own tools and then asks. So there is no document sync
//! and no overlay — every tool call re-ingests + re-analyses the app from
//! disk (cheap enough to do per call), seeing exactly what the agent just
//! wrote. The app root comes from `argv[1]`, else `$ROUNDHOUSE_APP_ROOT`,
//! else the process CWD.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::analyze::{diagnose_with_coverage, Analyzer};
use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ide;
use crate::ingest::{ingest_app, survey, IngestError};
use crate::project::{self, BuildTarget};

type McpResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The MCP revision we advertise when the client doesn't pin one. We echo
/// the client's requested version when present, for forward-compatibility.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Entry point for the `roundhouse-mcp` binary: serve over stdio until EOF.
pub fn run() -> McpResult<()> {
    let server = Server { root: workspace_root() };
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break; // EOF: client closed the pipe.
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("roundhouse-mcp: ignoring unparseable message: {e}");
                continue;
            }
        };
        if let Some(response) = server.handle(&msg) {
            writeln!(out, "{}", serde_json::to_string(&response)?)?;
            out.flush()?;
        }
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    if let Some(arg) = std::env::args().nth(1) {
        return PathBuf::from(arg);
    }
    if let Ok(env) = std::env::var("ROUNDHOUSE_APP_ROOT") {
        return PathBuf::from(env);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

struct Server {
    root: PathBuf,
}

impl Server {
    /// Dispatch one JSON-RPC message. Returns the response value for a
    /// request, or `None` for a notification (no `id` → no reply).
    fn handle(&self, msg: &Value) -> Option<Value> {
        let method = msg.get("method")?.as_str()?;
        let id = msg.get("id").cloned();
        match method {
            "initialize" => {
                let version = msg
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_PROTOCOL_VERSION)
                    .to_string();
                Some(ok(id?, self.initialize_result(&version)))
            }
            // Lifecycle notifications carry no id and need no reply.
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(ok(id?, json!({}))),
            "tools/list" => Some(ok(id?, tools_list())),
            "tools/call" => Some(self.tools_call(id?, msg.get("params"))),
            other => id.map(|id| err(id, -32601, format!("unknown method: {other}"))),
        }
    }

    fn initialize_result(&self, version: &str) -> Value {
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "roundhouse", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    fn tools_call(&self, id: Value, params: Option<&Value>) -> Value {
        let Some(params) = params else {
            return err(id, -32602, "missing params".to_string());
        };
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

        let outcome = match name {
            "type_at" => self.tool_type_at(&args),
            "can_be_nil" => self.tool_can_be_nil(&args),
            "references" => self.tool_references(&args),
            "diagnostics" => self.tool_diagnostics(&args),
            "traceroute" => self.tool_traceroute(&args),
            "wont_lower" => self.tool_wont_lower(&args),
            other => Err(format!("unknown tool: {other}")),
        };

        // MCP convention: a tool failure is a *result* with `isError`, not
        // a protocol error — so the model reads the message and adapts.
        match outcome {
            Ok(text) => ok(id, tool_text(text, false)),
            Err(text) => ok(id, tool_text(text, true)),
        }
    }

    /// Ingest + analyse the app fresh from disk — agents edit files between
    /// calls, so each query reflects the current on-disk state.
    ///
    /// Ingest runs in *survey mode*: a single unsupported construct (which
    /// every real app has — `alias_method`, `class << self`, …) records a
    /// gap and substitutes a placeholder instead of aborting, so the rest
    /// of the app stays queryable. Without this, one exotic node anywhere
    /// turns every `type_at`/`diagnostics` call into "ingest failed",
    /// making the server useless on any app larger than the demo fixture.
    /// Returns the recovered gaps so `diagnostics` can report the coverage
    /// hole rather than implying a clean bill of health.
    fn analyze(&self) -> Result<(App, Vec<Diagnostic>, Vec<IngestError>, Analyzer), String> {
        survey::activate();
        let (result, parse_diags) =
            crate::ingest::prism::scope(|| ingest_app(&self.root));
        let gaps = survey::drain();
        let mut app = result.map_err(|e| format!("ingest failed: {e}"))?;
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        Ok((app, parse_diags, gaps, analyzer))
    }

    fn tool_type_at(&self, args: &Value) -> Result<String, String> {
        let (app, _, _, _) = self.analyze()?;
        let (path, pos) = position_args(args)?;
        match ide::type_at_position(&app, &path, pos) {
            Some(info) => Ok(format!(
                "{}{} — {} node",
                info.display,
                if info.nilable { " (may be nil)" } else { "" },
                info.node_kind,
            )),
            None => Ok(format!("No typed expression at {path}.")),
        }
    }

    fn tool_can_be_nil(&self, args: &Value) -> Result<String, String> {
        let (app, _, _, _) = self.analyze()?;
        let (path, pos) = position_args(args)?;
        match ide::type_at_position(&app, &path, pos) {
            // Three-valued on purpose: an untyped/unresolved position is
            // an honest "can't tell", not a "no" — answering "cannot be
            // nil" off an `untyped` would be an overclaim an agent might
            // act on.
            Some(info) => Ok(match ide::nil_verdict(info.ty.as_ref()) {
                Some(true) => format!("Yes — type is `{}`, which admits nil.", info.display),
                Some(false) => format!("No — type is `{}`, which cannot be nil.", info.display),
                None => format!(
                    "Unknown — type is `{}`; roundhouse cannot tell whether this can be nil.",
                    info.display
                ),
            }),
            None => Ok(format!("No typed expression at {path}.")),
        }
    }

    fn tool_references(&self, args: &Value) -> Result<String, String> {
        let (app, _, _, _) = self.analyze()?;
        let (path, pos) = position_args(args)?;
        let file = ide::file_id(&app, &path).ok_or_else(|| format!("unknown file: {path}"))?;
        let src = ide::source(&app, file).ok_or("no source for file")?;
        let offset = ide::position_to_offset(&src.text, pos);

        let refs = ide::references(&app, file, offset);
        if refs.is_empty() {
            return Ok("No references — the position isn't on a local or instance variable.".to_string());
        }
        let lines: Vec<String> = refs
            .iter()
            .map(|r| {
                let s = ide::source(&app, r.span.file).expect("reference span has a source");
                let p = ide::offset_to_position(&s.text, r.span.start);
                let kind = if r.write { "write" } else { "read" };
                // Type-uncertain method matches (receiver type unresolved) are
                // labeled so an agent can treat them as lower-confidence.
                let conf = if r.certain { "" } else { ", uncertain" };
                format!("{}:{}:{} ({kind}{conf})", s.path, p.line + 1, p.character + 1)
            })
            .collect();
        let uncertain = refs.iter().filter(|r| !r.certain).count();
        let summary = if uncertain > 0 {
            format!("{} reference(s) ({uncertain} type-uncertain):", refs.len())
        } else {
            format!("{} reference(s):", refs.len())
        };
        Ok(format!("{summary}\n{}", lines.join("\n")))
    }

    fn tool_diagnostics(&self, args: &Value) -> Result<String, String> {
        let path_filter = args.get("path").and_then(|v| v.as_str());
        let (app, parse_diags, gaps, _) = self.analyze()?;
        let (mut diags, preload_cov) = diagnose_with_coverage(&app);
        // Diagnostics shadowing a recorded ingest gap become `note[...]`
        // lines naming the gap — an agent reading this output must be able
        // to tell "your code has a problem" from "roundhouse didn't
        // analyze the construct responsible".
        crate::analyze::attribution::attribute_ingest_gaps(&mut diags, &app, &gaps);
        diags.extend(parse_diags);

        let rendered: Vec<String> = diags
            .iter()
            .filter(|d| match path_filter {
                Some(p) => ide::source(&app, d.span.file)
                    .is_some_and(|s| s.path.ends_with(p) || p.ends_with(s.path.as_str())),
                None => true,
            })
            .map(|d| d.render(&app.sources))
            .collect();

        // Ingest gaps recovered under survey mode: constructs/templates the
        // analyzer skipped (so the result above is best-effort, not a clean
        // bill of health). These have no resolvable span — render file +
        // message. `survey::drain` flattens every gap to `Unsupported`.
        let gap_lines: Vec<String> = gaps
            .iter()
            .filter_map(|g| match g {
                IngestError::Unsupported { file, message } => Some((file, message)),
                _ => None,
            })
            .filter(|(file, _)| match path_filter {
                Some(p) => file.ends_with(p) || p.ends_with(file.as_str()),
                None => true,
            })
            .map(|(file, message)| format!("{file}: ingest gap: {message}"))
            .collect();

        let mut sections = Vec::new();
        if !rendered.is_empty() {
            sections.push(format!("{} diagnostic(s):\n{}", rendered.len(), rendered.join("\n")));
        }
        if !gap_lines.is_empty() {
            sections.push(format!(
                "{} ingest gap(s) — not analyzed, result above is best-effort:\n{}",
                gap_lines.len(),
                gap_lines.join("\n")
            ));
        }

        if sections.is_empty() {
            sections.push("No diagnostics — the app type-checks clean.".to_string());
        }

        // The missing_preload denominator (#64): a clean N+1 report is
        // only actionable with its coverage stated — "0 findings" must
        // be distinguishable from "couldn't check". Always app-wide;
        // the path filter above narrows the findings list, not the
        // claim.
        sections.push(format!(
            "missing_preload coverage (app-wide): checked {} query chain(s), \
             {} finding(s), {} chain(s) unverifiable (opaque to the static \
             chain harvest — no claim made)",
            preload_cov.known_chains, preload_cov.findings, preload_cov.opaque_chains,
        ));

        Ok(sections.join("\n\n"))
    }


    /// The request chain + coverage footer as structured JSON — the
    /// machine twin of the /ide/ trace panel (#63). One
    /// `ide::traceroute` + `ide::trace_gap_report` call rendered with
    /// empty fields omitted so an agent reads exactly what's there.
    fn tool_traceroute(&self, args: &Value) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("missing `query` — pass \"Controller#action\" or \"[VERB ]/path\"")?;
        let (app, _, gaps, analyzer) = self.analyze()?;
        let Some(trace) = ide::traceroute(&app, query) else {
            return Ok(format!(
                "No trace for `{query}` — not a known route or Controller#action."
            ));
        };
        let report = ide::trace_gap_report(&app, &trace, &gaps, Some(&analyzer));
        serde_json::to_string_pretty(&ide::trace_json(&trace, &report)).map_err(|e| e.to_string())
    }

    fn tool_wont_lower(&self, args: &Value) -> Result<String, String> {
        let target_str = args.get("target").and_then(|v| v.as_str()).ok_or("missing `target`")?;
        let target = BuildTarget::from_str(target_str)
            .filter(|t| BuildTarget::TRANSPILE.contains(t))
            .ok_or_else(|| {
                format!("unknown transpile target `{target_str}`; valid: {}", transpile_target_names())
            })?;
        let (mut app, _, _, analyzer) = self.analyze()?;
        // The wont-lower ledger reports gaps in what emitters actually
        // consume, so mirror the transpile driver's post-analyze shared
        // lowerings (this tool's App is its own copy; the source-shaped
        // query tools each analyze afresh).
        let _ = crate::lower::apply_post_analyze_lowerings(&mut app, analyzer.class_registry());

        // Run lower+emit for the target inside the emit-diagnostic scope so
        // every unsupported-construct gap is collected (issue #28's sink).
        let (_files, diags) =
            crate::emit::diagnostics::scope(|| project::target_files(&app, &self.root, target));
        let gaps: Vec<String> = diags
            .iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::Unsupported { .. }))
            .map(|d| d.render(&app.sources))
            .collect();

        if gaps.is_empty() {
            Ok(format!("`{target_str}` lowers cleanly — no unsupported constructs."))
        } else {
            Ok(format!(
                "{} construct(s) won't lower to `{target_str}`:\n{}",
                gaps.len(),
                gaps.join("\n")
            ))
        }
    }
}

/// `{ path, line, column }` (1-based line/column) → an `ide::Position`
/// (0-based; `character` is UTF-16, which equals the column for the ASCII
/// source that dominates Ruby).
fn position_args(args: &Value) -> Result<(String, ide::Position), String> {
    let path = args.get("path").and_then(|v| v.as_str()).ok_or("missing `path`")?.to_string();
    let line = args.get("line").and_then(|v| v.as_u64()).ok_or("missing `line`")? as u32;
    let column = args.get("column").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let pos = ide::Position { line: line.saturating_sub(1), character: column.saturating_sub(1) };
    Ok((path, pos))
}

fn transpile_target_names() -> String {
    BuildTarget::TRANSPILE.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}


fn tools_list() -> Value {
    let position_schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Source file path (app-relative or absolute)." },
            "line": { "type": "integer", "description": "1-based line number." },
            "column": { "type": "integer", "description": "1-based column. Defaults to 1." }
        },
        "required": ["path", "line"]
    });
    json!({
        "tools": [
            {
                "name": "type_at",
                "description": "Inferred type of the expression at a source position — no app boot, works on broken code. Reports the type, whether it can be nil, and the node kind.",
                "inputSchema": position_schema,
            },
            {
                "name": "can_be_nil",
                "description": "Whether the value at a source position can be nil (its type is nil or a union with a nil arm). Static nil-safety, no runtime.",
                "inputSchema": position_schema,
            },
            {
                "name": "references",
                "description": "Every read and write of the local or instance variable at a position. Locals resolve by exact binding (body-scoped); instance variables by name (class-scoped).",
                "inputSchema": position_schema,
            },
            {
                "name": "diagnostics",
                "description": "Type/analysis problems across the app (unresolved ivars, failed method dispatch, incompatible operators, syntax errors, static N+1 missing-preload warnings). Ends with the missing_preload coverage triple (checked / findings / unverifiable) so a clean N+1 report states its denominator. Optionally filter to one file.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Optional: limit to this file." }
                    }
                },
            },
            {
                "name": "traceroute",
                "description": "The full static request flow for a route or Controller#action, as JSON: ordered hops (route match; every before/around/after filter with its defining class or concern, only:/except:/if: gating, skip application, typed ivar assigns, DB effects, file:line; the action; the view with its partials; the layout), static N+1 findings annotated on the hop whose body or template contains the un-preloaded association read (`n_plus_one`), plus a coverage report — how many hops resolved and what blocks the rest, split between untyped gem/framework boundaries (with an inferred candidate RBS signature to accept when available) and roundhouse's own ingest gaps. Statically derived; no app boot.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "\"Controller#action\" (StatusesController#show) or \"[VERB ]/path\" (GET /articles/:id)." }
                    },
                    "required": ["query"]
                },
            },
            {
                "name": "wont_lower",
                "description": "Which constructs in the app won't compile to a given eject target (rust, go, typescript, …) — the 'will this survive ejection?' check no other Rails tool offers.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Eject target: rust, go, typescript, kotlin, swift, crystal, elixir, python, csharp." }
                    },
                    "required": ["target"]
                },
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server { root: PathBuf::from("fixtures/real-blog") }
    }

    fn call(server: &Server, name: &str, args: Value) -> Value {
        let msg = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        });
        server.handle(&msg).expect("tools/call returns a response")
    }

    fn text_of(response: &Value) -> String {
        response["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
    }

    #[test]
    fn initialize_reports_server_info_and_echoes_version() {
        let msg = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {} }
        });
        let resp = server().handle(&msg).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "roundhouse");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }


    #[test]
    fn traceroute_returns_the_structured_chain_with_coverage() {
        let resp = call(
            &server(),
            "traceroute",
            json!({ "query": "ArticlesController#edit" }),
        );
        let text = text_of(&resp);
        let v: Value = serde_json::from_str(&text).expect("tool returns JSON");
        assert_eq!(v["route"], "GET /articles/:id/edit → ArticlesController#edit");
        let hops = v["hops"].as_array().expect("hops array");
        assert_eq!(hops[0]["kind"], "route");
        let filter = hops
            .iter()
            .find(|h| h["kind"] == "filter" && h["name"] == "set_article")
            .expect("set_article hop");
        assert_eq!(filter["applies"], true);
        assert_eq!(filter["resolved"], true);
        assert_eq!(filter["assigns"]["@article"], "Article");
        assert!(hops.iter().any(|h| h["kind"] == "view" && h["name"] == "articles/edit"));
        assert!(hops.iter().any(|h| h["kind"] == "layout"));
        // real-blog resolves clean: the coverage triple is the positive claim.
        assert_eq!(v["coverage"]["complete"], true);
        assert_eq!(v["gaps"].as_array().map(|g| g.len()), Some(0));
    }

    #[test]
    fn traceroute_misses_politely() {
        let resp = call(&server(), "traceroute", json!({ "query": "NopeController#zap" }));
        assert!(text_of(&resp).starts_with("No trace for"));
    }

    #[test]
    fn tools_list_advertises_every_tool() {
        let msg = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let resp = server().handle(&msg).unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for tool in ["type_at", "can_be_nil", "references", "diagnostics", "traceroute", "wont_lower"] {
            assert!(names.contains(&tool), "missing tool {tool}");
        }
    }

    #[test]
    fn references_lists_ivar_uses() {
        let content =
            std::fs::read_to_string("fixtures/real-blog/app/controllers/articles_controller.rb")
                .unwrap();
        let byte = content.find("@article =").unwrap() + 1;
        let before = &content[..byte];
        let line = before.matches('\n').count() as u64 + 1;
        let column = (byte - before.rfind('\n').map(|p| p + 1).unwrap_or(0)) as u64 + 1;

        let resp = call(
            &server(),
            "references",
            json!({ "path": "app/controllers/articles_controller.rb", "line": line, "column": column }),
        );
        let text = text_of(&resp);
        assert!(text.contains("reference(s)"), "got: {text}");
        assert!(text.contains("(write)"), "should mark the assignment a write: {text}");
    }

    #[test]
    fn notification_without_id_gets_no_response() {
        let msg = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(server().handle(&msg).is_none());
    }

    #[test]
    fn type_at_resolves_a_string_in_class_body_dsl() {
        // The `broadcasts_to ->(_a){ "articles" }` lambda in article.rb —
        // exercises both the MCP path and the class-body DSL coverage.
        let content =
            std::fs::read_to_string("fixtures/real-blog/app/models/article.rb").unwrap();
        let byte = content.find("\"articles\"").unwrap() + 2;
        let before = &content[..byte];
        let line = before.matches('\n').count() as u64 + 1;
        let column = (byte - before.rfind('\n').map(|p| p + 1).unwrap_or(0)) as u64 + 1;

        let resp = call(
            &server(),
            "type_at",
            json!({ "path": "app/models/article.rb", "line": line, "column": column }),
        );
        assert!(text_of(&resp).contains("String"), "got: {}", text_of(&resp));
    }

    #[test]
    fn diagnostics_real_blog_has_no_errors() {
        // real-blog carries no hard type ERRORS. It does surface
        // coverage-class WARNINGS (unresolved framework helpers,
        // gradual-untyped) — visible modeling debt we ratchet to zero as
        // the framework gets typed, not a regression. "No errors" is the
        // durable invariant; the old "clean" assertion only held while
        // those silently-unresolved positions were invisible.
        let resp = call(&server(), "diagnostics", json!({}));
        let text = text_of(&resp);
        assert!(!text.contains("error["), "real-blog should report no errors, got: {text}");
    }

    #[test]
    fn diagnostics_reports_the_missing_preload_coverage_triple() {
        // #64: the denominator is part of the claim — the coverage
        // line must be present even when there are zero findings, so
        // "0 findings" is distinguishable from "couldn't check".
        let text = text_of(&call(&server(), "diagnostics", json!({})));
        assert!(
            text.contains("missing_preload coverage (app-wide): checked "),
            "coverage triple missing, got: {text}"
        );
        assert!(
            text.contains("unverifiable"),
            "triple states the unverifiable bucket, got: {text}"
        );
    }

    #[test]
    fn diagnostics_surfaces_uningested_view_templates() {
        // Regression for the silent-view-drop gap: real-blog ships
        // `mailer.text.erb` / `manifest.json.erb` (non-`.html.erb`
        // templates the analyzer doesn't type). They must show up as
        // ingest gaps rather than vanishing. This also guards the survey-
        // mode wiring in `analyze()`: drop `survey::activate()` and the
        // gap collector stays empty, so this line disappears.
        let text = text_of(&call(&server(), "diagnostics", json!({})));
        assert!(
            text.contains("view template not ingested"),
            "expected un-ingested view templates to be surfaced as gaps, got: {text}"
        );
    }

    #[test]
    fn wont_lower_lowers_datetime_cleanly_and_bad_target_is_an_error() {
        // real-blog has Date/DateTime columns, which type as the first-
        // class `Ty::Time`. The shared Stage-2 datetime foundation stores
        // temporal columns as ISO-8601 TEXT and exposes them through a
        // synthesized reader that parses to a native datetime — so `Ty::Time`
        // never reaches a not-supported type position. With Python now
        // wired (its reader emits a `datetime` `@property` over a `str`
        // backing, rather than a `RoundhouseUnsupportedTime` field), every
        // transpile target lowers real-blog cleanly. (When a target grows a
        // NEW un-lowerable construct, re-point one of these to witness the
        // gap; a valid target with gaps is still isError=false — the gaps
        // are the *answer*, not a tool failure.)
        for target in ["rust", "python", "go", "typescript"] {
            let r = call(&server(), "wont_lower", json!({ "target": target }));
            assert_eq!(r["result"]["isError"], false);
            assert!(
                text_of(&r).contains("lowers cleanly"),
                "expected `{target}` to lower real-blog cleanly (datetime seam), got: {}",
                text_of(&r)
            );
        }

        // An unknown/unsupported target is a genuine tool error.
        let bad = call(&server(), "wont_lower", json!({ "target": "fortran" }));
        assert_eq!(bad["result"]["isError"], true);
    }
}
