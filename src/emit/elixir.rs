//! Elixir emitter.
//!
//! Second Phase 2 scaffold. Elixir is the target that most aggressively
//! stress-tests IR target-neutrality because its paradigm is
//! fundamentally different from every other target on the list:
//!
//! - No classes — models are modules with a `defstruct` payload.
//! - No method dispatch — what Ruby calls `article.title` becomes
//!   module-function-on-record: `Article.title(article)` (or struct
//!   field access, `article.title`, for direct attributes).
//! - No mutation — ivar rebinds become variable rebinds (no `self.foo =`
//!   state threading at scaffold depth; real Elixir code returns the
//!   updated struct).
//! - No inheritance — the `parent` field on Model/Controller is noted
//!   but not emitted. Rails' `ApplicationRecord` / `ApplicationController`
//!   become `use Railcar.Record` / `use Railcar.Controller`-style
//!   conventions in a real runtime; the scaffold doesn't commit yet.
//! - Pattern matching as control flow — `if expr.save, do: …, else: …`
//!   is idiomatic as a `case` on `{:ok, _} / {:error, _}`. Scaffold
//!   emits `if/else` for now; Phase 3 runtime work converts.
//!
//! Non-goals:
//! - `@spec` type annotations (Elixir is dynamically typed; `Ty` info
//!   is useful for the Rust/Go/TS targets, not here).
//! - Phoenix / Plug integration.
//! - Live View / template emission.
//! - Controllers that return `{:cont, conn}` tuples (real Plug shape).

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ident::Symbol;
use crate::dialect::RouteSpec;
use crate::expr::{Expr, ExprNode, LValue, Literal};

mod controller;
mod fixture;
mod model;
mod spec;
mod view;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/elixir/runtime.ex");
const DB_SOURCE: &str = include_str!("../../runtime/elixir/db.ex");
/// Elixir HTTP runtime — Phase 4d pass-2 shape. Copied verbatim
/// into generated projects as `lib/roundhouse/http.ex` when any
/// controller emits. Exposes ActionResponse/ActionContext structs
/// + Router.match table; the emitter's action templates return
/// ActionResponse directly (no class-based dispatch).
const HTTP_SOURCE: &str = include_str!("../../runtime/elixir/http.ex");
/// Pass-2 test-support runtime. TestClient + TestResponse with
/// Rails-shaped assertions. Ships as
/// `lib/roundhouse/test_support.ex`.
const TEST_SUPPORT_SOURCE: &str =
    include_str!("../../runtime/elixir/test_support.ex");
/// View helpers — link_to, button_to, FormBuilder, etc. Ships as
/// `lib/roundhouse/view_helpers.ex` when views emit.
const VIEW_HELPERS_SOURCE: &str =
    include_str!("../../runtime/elixir/view_helpers.ex");
/// Plug.Cowboy-based HTTP server. Ships as
/// `lib/roundhouse/server.ex`.
const SERVER_SOURCE: &str = include_str!("../../runtime/elixir/server.ex");
/// /cable stub. Ships as `lib/roundhouse/cable.ex`.
const CABLE_SOURCE: &str = include_str!("../../runtime/elixir/cable.ex");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(emit_mix_exs());
    if !app.models.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse.ex"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/db.ex"),
            content: DB_SOURCE.to_string(),
        });
        files.push(emit_schema_sql_ex(app));
    }
    for model in &app.models {
        files.push(model::emit_model_file(model, app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime (ActionResponse/ActionContext + Router) —
        // copied verbatim.
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/http.ex"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/test_support.ex"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/view_helpers.ex"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/server.ex"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/cable.ex"),
            content: CABLE_SOURCE.to_string(),
        });
        let known_models: Vec<Symbol> =
            app.models.iter().map(|m| m.name.0.clone()).collect();
        for controller in &app.controllers {
            files.push(controller::emit_controller_file_pass2(controller, &known_models, app));
        }
        files.push(emit_ex_route_helpers(app));
        files.push(emit_ex_importmap(app));
        files.push(emit_ex_main(app));
        files.push(view::emit_ex_views(app));
    }
    if !app.routes.entries.is_empty() {
        files.push(controller::emit_router_file(app));
        files.push(emit_ex_routes_register(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(fixture::emit_ex_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(fixture::emit_ex_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("test/test_helper.exs"),
            content: "ExUnit.start()\n".to_string(),
        });
        for tm in &app.test_modules {
            files.push(spec::emit_ex_test(tm, app));
        }
    }
    files
}

/// Minimal mix.exs. `elixirc_paths` uses a wildcard filter that
/// excludes controllers and the router — their bodies reference
/// runtime that doesn't exist yet (redirect_to, Post.all, etc.), so
/// including them blocks `mix compile`. When Phase 3 wires the
/// runtime, the filter relaxes.
fn emit_mix_exs() -> EmittedFile {
    let content = "\
defmodule App.MixProject do
  use Mix.Project

  def project do
    [
      app: :app,
      version: \"0.1.0\",
      elixir: \"~> 1.18\",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  def application do
    [extra_applications: [:logger]]
  end

  defp deps do
    [
      {:exqlite, \"~> 0.30\"},
      {:plug_cowboy, \"~> 2.7\"},
      {:jason, \"~> 1.4\"},
      {:websock_adapter, \"~> 0.5\"}
    ]
  end

  # Phase 4c: controllers now lower through Roundhouse.Http stubs and
  # compile. Test env additionally includes test/support/ so fixtures
  # are compiled alongside the app.
  defp elixirc_paths(:test) do
    Path.wildcard(\"lib/**/*.ex\") ++ Path.wildcard(\"test/support/**/*.ex\")
  end

  defp elixirc_paths(_) do
    Path.wildcard(\"lib/**/*.ex\")
  end
end
";
    EmittedFile {
        path: PathBuf::from("mix.exs"),
        content: content.to_string(),
    }
}

/// `lib/roundhouse/schema_sql.ex` — Elixir module exposing the
/// target-neutral DDL produced by `lower::lower_schema` as a
/// `create_tables/0` function (Elixir module attributes can't hold
/// arbitrary strings at compile time in all versions; a function is
/// uniform).
fn emit_schema_sql_ex(app: &App) -> EmittedFile {
    let ddl = crate::lower::lower_schema(&app.schema);
    // Escape the DDL for a single-quoted Elixir string: backslashes,
    // quotes, newlines. Heredoc would be cleaner but needs matching
    // indentation, which collides with the DDL's column-0 layout.
    let escaped = ddl
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Roundhouse.SchemaSQL do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def create_tables do").unwrap();
    writeln!(s, "    \"{escaped}\"").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/roundhouse/schema_sql.ex"),
        content: s,
    }
}

// Pass-2 route helpers -------------------------------------------------

/// Emit `lib/roundhouse/importmap.ex` — a `Pins` module with a
/// `@pins` attribute that the layout's
/// `javascript_importmap_tags` helper consumes.
fn emit_ex_importmap(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Roundhouse.Importmap do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s, "  @pins [").unwrap();
    if let Some(importmap) = &app.importmap {
        for pin in &importmap.pins {
            writeln!(s, "    {{{:?}, {:?}}},", pin.name, pin.path).unwrap();
        }
    }
    writeln!(s, "  ]").unwrap();
    writeln!(s, "  def pins, do: @pins").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/roundhouse/importmap.ex"),
        content: s,
    }
}

/// Emit `lib/app_main.ex` — exposes a `App.Main.run/0` entry that
/// the compare driver invokes via `mix run -e`. Opens the DB via
/// `server.start/2` with the emitted layout when present. Defining
/// this as a module (rather than running top-level via a script)
/// keeps `mix compile` able to inspect it.
fn emit_ex_main(app: &App) -> EmittedFile {
    let has_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application");
    let layout_opt = if has_layout {
        "layout: fn -> App.Views.render_layouts_application(nil) end, "
    } else {
        ""
    };
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule App.Main do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s, "  alias Roundhouse.Server").unwrap();
    writeln!(s, "  alias Roundhouse.SchemaSQL").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def run do").unwrap();
    writeln!(s, "    App.Routes.register()").unwrap();
    writeln!(s, "    register_partials()").unwrap();
    writeln!(s, "    Server.start(SchemaSQL.create_tables(), {layout_opt}port: resolve_port())").unwrap();
    writeln!(s, "  end").unwrap();
    // Register partial renderers for every model whose plural
    // resource has a `_<singular>.html.erb` partial. Cable's
    // broadcast_*_to resolves the partial by model class name.
    writeln!(s).unwrap();
    writeln!(s, "  defp register_partials do").unwrap();
    writeln!(s, "    Roundhouse.Cable.ensure_started()").unwrap();
    let known_model_names: std::collections::BTreeSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .collect();
    let mut registered: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for v in &app.views {
        let name = v.name.as_str();
        if let Some((dir, base)) = name.rsplit_once('/') {
            if let Some(singular) = base.strip_prefix('_') {
                let class = crate::naming::camelize(singular);
                if !known_model_names.contains(&class) {
                    continue;
                }
                if registered.insert(class.clone()) {
                    let fn_name = format!("render_{}_{singular}", dir);
                    writeln!(
                        s,
                        "    Roundhouse.Cable.register_partial({class:?}, fn id ->"
                    )
                    .unwrap();
                    writeln!(s, "      case {class}.find(id) do").unwrap();
                    writeln!(s, "        nil -> \"\"").unwrap();
                    writeln!(s, "        record -> App.Views.{fn_name}(record)").unwrap();
                    writeln!(s, "      end").unwrap();
                    writeln!(s, "    end)").unwrap();
                }
            }
        }
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  defp resolve_port do").unwrap();
    writeln!(s, "    case System.get_env(\"PORT\") do").unwrap();
    writeln!(s, "      nil -> 3000").unwrap();
    writeln!(s, "      s -> String.to_integer(s)").unwrap();
    writeln!(s, "    end").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/app_main.ex"),
        content: s,
    }
}

fn emit_ex_route_helpers(app: &App) -> EmittedFile {
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Roundhouse.RouteHelpers do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s).unwrap();
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for route in &flat {
        if !seen.insert(route.as_name.clone()) {
            continue;
        }
        let fn_name = format!("{}_path", route.as_name);
        let params: Vec<String> = route
            .path_params
            .iter()
            .map(|p| format!("{p} \\\\ 0"))
            .collect();
        let sig = if params.is_empty() {
            String::new()
        } else {
            format!("({})", params.join(", "))
        };
        let body = if route.path_params.is_empty() {
            format!("\"{}\"", route.path)
        } else {
            let mut interp = String::new();
            let mut chars = route.path.chars().peekable();
            while let Some(c) = chars.next() {
                if c == ':' {
                    let mut ident = String::new();
                    while let Some(&nc) = chars.peek() {
                        if nc.is_alphanumeric() || nc == '_' {
                            ident.push(nc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    interp.push_str("#{");
                    interp.push_str(&ident);
                    interp.push('}');
                } else {
                    interp.push(c);
                }
            }
            format!("\"{}\"", interp)
        };
        writeln!(s, "  def {fn_name}{sig}, do: {body}").unwrap();
    }
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/roundhouse/route_helpers.ex"),
        content: s,
    }
}

// Pass-2 route registration --------------------------------------------

fn emit_ex_routes_register(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule App.Routes do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s, "  alias Roundhouse.Http.Router").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def register do").unwrap();
    writeln!(s, "    Router.reset()").unwrap();
    for entry in &app.routes.entries {
        emit_ex_route_spec(&mut s, entry);
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/app/routes.ex"),
        content: s,
    }
}

fn emit_ex_route_spec(out: &mut String, spec: &RouteSpec) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, .. } => {
            let verb = match method {
                crate::dialect::HttpMethod::Get => "get",
                crate::dialect::HttpMethod::Post => "post",
                crate::dialect::HttpMethod::Put => "put",
                crate::dialect::HttpMethod::Patch => "patch",
                crate::dialect::HttpMethod::Delete => "delete",
                _ => "get",
            };
            writeln!(
                out,
                "    Router.{verb}({:?}, {}, :{})",
                path,
                controller.0,
                action.as_str(),
            )
            .unwrap();
        }
        RouteSpec::Root { target } => {
            let (controller, action) = target
                .split_once('#')
                .map(|(c, a)| (controller::controller_class_name(c), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            writeln!(out, "    Router.root({controller}, :{action})").unwrap();
        }
        RouteSpec::Resources { name, only, except: _, nested } => {
            let controller = controller::controller_class_name(name.as_str());
            let mut opts: Vec<String> = Vec::new();
            if !only.is_empty() {
                let parts: Vec<String> =
                    only.iter().map(|s| format!(":{}", s.as_str())).collect();
                opts.push(format!("only: [{}]", parts.join(", ")));
            }
            if !nested.is_empty() {
                let mut nested_parts: Vec<String> = Vec::new();
                for child in nested {
                    if let Some(part) = ex_nested_spec_entry(child) {
                        nested_parts.push(part);
                    }
                }
                if !nested_parts.is_empty() {
                    opts.push(format!("nested: [{}]", nested_parts.join(", ")));
                }
            }
            if opts.is_empty() {
                writeln!(
                    out,
                    "    Router.resources({:?}, {})",
                    name.as_str(),
                    controller
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "    Router.resources({:?}, {}, [{}])",
                    name.as_str(),
                    controller,
                    opts.join(", ")
                )
                .unwrap();
            }
        }
    }
}

fn ex_nested_spec_entry(spec: &RouteSpec) -> Option<String> {
    let RouteSpec::Resources { name, only, except: _, nested: _ } = spec else {
        return None;
    };
    let controller = controller::controller_class_name(name.as_str());
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("name: {:?}", name.as_str()));
    parts.push(format!("controller: {}", controller));
    if !only.is_empty() {
        let items: Vec<String> =
            only.iter().map(|s| format!(":{}", s.as_str())).collect();
        parts.push(format!("only: [{}]", items.join(", ")));
    }
    Some(format!("[{}]", parts.join(", ")))
}

// Bodies ---------------------------------------------------------------

/// Emit a method / action body as Elixir statements. Ruby ivar writes
/// become local rebinds (`@post = …` → `post = …`); ivar reads become
/// struct field access through the receiver arg. If `receiver_arg` is
/// `None` (e.g. a controller action), ivar reads become bare locals.
pub(super) fn emit_block(body: &Expr, receiver_arg: Option<&str>) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt(e, receiver_arg));
            }
            lines.join("\n")
        }
        _ => emit_stmt(body, receiver_arg),
    }
}

fn emit_stmt(e: &Expr, receiver_arg: Option<&str>) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value, receiver_arg))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            // Elixir has no instance state. At scaffold depth, treat
            // `@foo = expr` as a local rebind `foo = expr`; real
            // controller code that mutates @post across multiple
            // statements needs a `with` pipeline, which Phase 3 adds.
            format!("{} = {}", name, emit_expr(value, receiver_arg))
        }
        _ => emit_expr(e, receiver_arg),
    }
}

pub(super) fn emit_expr(e: &Expr, receiver_arg: Option<&str>) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => {
            // Inside an instance method, `@foo` is a field on the
            // record arg: `post.foo`. Outside (e.g., in a controller
            // action), we've rebound to a local, so emit the bare name.
            match receiver_arg {
                Some(recv) => format!("{recv}.{name}"),
                None => name.to_string(),
            }
        }
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args, receiver_arg)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value, receiver_arg),
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|e| emit_expr(e, receiver_arg))
            .collect::<Vec<_>>()
            .join("; "),
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond, receiver_arg);
            let then_s = emit_block(then_branch, receiver_arg);
            let else_s = emit_block(else_branch, receiver_arg);
            // Elixir's `if / else / end` form. `case` would be more
            // idiomatic for `{:ok, _} / {:error, _}` shapes but that's
            // a Phase-3 semantic transform.
            format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            )
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!(
                "{} {op_s} {}",
                emit_expr(left, receiver_arg),
                emit_expr(right, receiver_arg),
            )
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_expr(e, receiver_arg)).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    // A symbol key in Ruby (`foo: 1`) becomes an atom key
                    // in an Elixir map: `%{foo: 1}` (shorthand) or
                    // `%{:foo => 1}`. Emit the shorthand when the key is
                    // a bareword-safe symbol; rocket form otherwise.
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_expr(v, receiver_arg))
                    } else {
                        format!(
                            "{} => {}",
                            emit_expr(k, receiver_arg),
                            emit_expr(v, receiver_arg),
                        )
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Elixir interpolation syntax matches Ruby exactly:
            // `"text #{expr} more"`. Emit verbatim.
            use crate::expr::InterpPart;
            let mut out = String::from("\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_expr(expr, receiver_arg));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ExprNode::Yield { args } => {
            let parts: Vec<String> = args.iter().map(|e| emit_expr(e, receiver_arg)).collect();
            // Elixir doesn't have `yield`; use `send(self, …)` as a
            // placeholder that parses. Real runtime work would pattern
            // this into a block-passing convention.
            format!("send(self(), {{:yield, {}}})", parts.join(", "))
        }
        other => format!("# TODO: emit {:?}", std::mem::discriminant(other)),
    }
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    receiver_arg: Option<&str>,
) -> String {
    let args_s: Vec<String> = args.iter().map(|e| emit_expr(e, receiver_arg)).collect();

    if method == "[]" && recv.is_some() {
        // `params[:id]` → `params[:id]` (Elixir maps with atom keys
        // support the same access syntax). For string keys or integer
        // indexing this would lower to `Map.fetch!` / `Enum.at`.
        return format!("{}[{}]", emit_expr(recv.unwrap(), receiver_arg), args_s.join(", "));
    }

    match recv {
        None => {
            // Bareword call. In Elixir this is a function in the
            // enclosing module or an imported function — the scaffold
            // emits as-is.
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r, receiver_arg);
            // Ruby String methods map onto Elixir's `String` module
            // functions (module-function-call form, not method). `.strip`
            // → `String.trim(recv)`, upcase/downcase similar.
            if args.is_empty() && matches!(r.ty, Some(crate::ty::Ty::Str)) {
                if let Some(wrapped) = map_ex_str_method(method, &recv_s) {
                    return wrapped;
                }
            }
            // `recv.method(args)` reads fine for both module function
            // calls (e.g. `Post.find(id)`) and struct-field-style
            // getters (`post.title` with no args).
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

/// Map Ruby String methods onto Elixir's `String` module functions
/// (module-function-call form — Elixir strings don't have `.method`
/// dispatch). Returns `Some(emit_text)` for a handled method; unhandled
/// methods fall through to the default `recv.method` emit.
fn map_ex_str_method(method: &str, recv_text: &str) -> Option<String> {
    match method {
        "strip" => Some(format!("String.trim({recv_text})")),
        "upcase" => Some(format!("String.upcase({recv_text})")),
        "downcase" => Some(format!("String.downcase({recv_text})")),
        "length" | "size" => Some(format!("String.length({recv_text})")),
        "empty?" => Some(format!("{recv_text} == \"\"")),
        _ => None,
    }
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Ruby symbols map cleanly to Elixir atoms.
        Literal::Sym { value } => format!(":{}", value.as_str()),
    }
}

fn indent(text: &str, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    text.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

