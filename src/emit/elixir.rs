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
use crate::dialect::{
    Controller, RouteSpec, Test, TestModule,
};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::lower::CtrlWalker as _;
use crate::naming::snake_case;

mod model;
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
            files.push(emit_controller_file_pass2(controller, &known_models, app));
        }
        files.push(emit_ex_route_helpers(app));
        files.push(emit_ex_importmap(app));
        files.push(emit_ex_main(app));
        files.push(view::emit_ex_views(app));
    }
    if !app.routes.entries.is_empty() {
        files.push(emit_router_file(app));
        files.push(emit_ex_routes_register(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(emit_ex_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(emit_ex_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("test/test_helper.exs"),
            content: "ExUnit.start()\n".to_string(),
        });
        for tm in &app.test_modules {
            files.push(emit_ex_test(tm, app));
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

// Router ---------------------------------------------------------------

fn emit_router_file(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Router do").unwrap();
    writeln!(s, "  @routes [").unwrap();
    let flat = crate::lower::flatten_routes(app);
    for (i, route) in flat.iter().enumerate() {
        let sep = if i + 1 == flat.len() { "" } else { "," };
        writeln!(
            s,
            "    %{{method: :{}, path: {:?}, controller: {}, action: :{}}}{sep}",
            ex_http_verb_lower(&route.method),
            route.path,
            route.controller.0,
            route.action,
        )
        .unwrap();
    }
    writeln!(s, "  ]").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def routes, do: @routes").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("lib/router.ex"), content: s }
}

/// Map `dialect::HttpMethod` to the Elixir atom form (`:get`,
/// `:post`, …) used inside the `@routes` module-attribute list.
fn ex_http_verb_lower(m: &crate::dialect::HttpMethod) -> &'static str {
    use crate::dialect::HttpMethod;
    match m {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        _ => "get",
    }
}

fn controller_class_name(short: &str) -> String {
    let mut s = crate::naming::camelize(short);
    s.push_str("Controller");
    s
}

// Pass-2 controllers ---------------------------------------------------
//
// The pass-1 emitter (`emit_controller_file`) returns :ok from every
// action to make `mix compile` pass. Pass-2 replaces that with
// template actions — each action returns an ActionResponse struct
// that Router.match-driven TestClient dispatch can assert on. The
// action bodies are synthesized per Rails CRUD shape (index, show,
// new, edit, create, update, destroy); arbitrary custom actions
// fall back to a 501 stub. Mirrors the Python/TS/Crystal pass-2
// controllers in shape.

fn emit_controller_file_pass2(
    c: &Controller,
    known_models: &[Symbol],
    app: &App,
) -> EmittedFile {
    let module = c.name.0.as_str();
    let resource = crate::lower::resource_from_controller_name(module);
    let model_class = crate::naming::singularize_camelize(&resource);
    let has_model = known_models.iter().any(|m| m.as_str() == model_class);
    let parent = crate::lower::find_nested_parent(app, module);
    let permitted = crate::lower::permitted_fields_for(c, &resource)
        .unwrap_or_else(|| crate::lower::default_permitted_fields(app, &model_class));

    let (public_actions, _private) = crate::lower::split_public_private(c);

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {module} do").unwrap();
    if !public_actions.is_empty() {
        writeln!(s, "  alias Roundhouse.Http.ActionResponse").unwrap();
        writeln!(s, "  import Roundhouse.RouteHelpers").unwrap();
        writeln!(s).unwrap();
    }

    for action in &public_actions {
        let la = crate::lower::lower_action(
            action.name.as_str(),
            &resource,
            &model_class,
            has_model,
            parent.as_ref(),
            &permitted,
        );
        emit_ex_action(&mut s, &la, &action.body, known_models, c);
        writeln!(s).unwrap();
    }
    writeln!(s, "end").unwrap();

    let fname = format!("lib/{}.ex", snake_case(module));
    EmittedFile {
        path: PathBuf::from(fname),
        content: s,
    }
}

fn ex_view_fn(model_class: &str, suffix: &str) -> String {
    let plural = crate::naming::pluralize_snake(model_class);
    format!("render_{plural}_{}", suffix.to_lowercase())
}

/// Render one LoweredAction as an Elixir `def`. Body emission flows
/// through the walker; the ActionKind dispatch is gone (precedent
/// from TS/Rust/Python/Crystal/Go). Elixir's key distinctive
/// idioms: immutable structs, `%{r | field: v}` update syntax,
/// `Module.function(struct)` dispatch (so `record.save` becomes
/// `Model.save(record)`).
fn emit_ex_action(
    out: &mut String,
    la: &crate::lower::LoweredAction,
    body: &Expr,
    known_models: &[Symbol],
    controller: &Controller,
) {
    let name = la.name.as_str();

    if !la.has_model {
        writeln!(out, "  def {name}(_context) do").unwrap();
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
        writeln!(out, "  end").unwrap();
        return;
    }

    use crate::lower::CtrlWalker;
    let normalized = crate::lower::normalize_action_body(controller, la.name.as_str(), body);
    let mut emitter = ExEmitter {
        ctx: crate::lower::WalkCtx {
            known_models,
            model_class: la.model_class.as_str(),
            resource: la.resource.as_str(),
            parent: la.parent.as_ref(),
            permitted: &la.permitted,
            adapter: &crate::adapter::SqliteAdapter,
        },
        state: crate::lower::WalkState::new(),
    };
    // Indent=2 because Elixir def-bodies live inside a defmodule.
    let mut body_src = String::new();
    emitter.walk_stmt(&normalized, &mut body_src, 2, true);

    let arg = if emitter.state.uses_context || body_src.contains("context") {
        "context"
    } else {
        "_context"
    };
    writeln!(out, "  def {name}({arg}) do").unwrap();
    out.push_str(&body_src);
    writeln!(out, "  end").unwrap();
}

struct ExEmitter<'a> {
    ctx: crate::lower::WalkCtx<'a>,
    state: crate::lower::WalkState,
}

impl<'a> crate::lower::CtrlWalker<'a> for ExEmitter<'a> {
    fn ctx(&self) -> &crate::lower::WalkCtx<'a> { &self.ctx }
    fn state_mut(&mut self) -> &mut crate::lower::WalkState { &mut self.state }
    fn indent_unit(&self) -> &'static str { "  " }

    fn write_assign(&mut self, name: &str, value: &Expr, indent: &str, out: &mut String) {
        let rhs = self.render_expr(value);
        writeln!(out, "{indent}{name} = {rhs}").unwrap();
    }

    fn write_create_expansion(
        &mut self,
        var_name: &str,
        class: &str,
        indent: &str,
        out: &mut String,
    ) {
        writeln!(out, "{indent}{var_name} = %{class}{{}}").unwrap();
        if let Some(parent) = self.ctx.parent {
            writeln!(
                out,
                "{indent}{var_name} = %{{{var_name} | {0}_id: String.to_integer(to_string(context.params[\"{0}_id\"]))}}",
                parent.singular,
            )
            .unwrap();
            self.state.uses_context = true;
        }
        if !self.ctx.permitted.is_empty() {
            let permitted: Vec<String> = self.ctx.permitted.iter().cloned().collect();
            let resource = self.ctx.resource.to_string();
            let assigns: Vec<String> = permitted.iter()
                .map(|f| format!("{f}: Map.get(context.params, \"{resource}[{f}]\", \"\")"))
                .collect();
            writeln!(
                out,
                "{indent}{var_name} = %{{{var_name} | {}}}",
                assigns.join(", "),
            )
            .unwrap();
            self.state.uses_context = true;
        }
    }

    fn write_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    ) {
        // Pre-render cond so the save-detection runs first.
        let save_recv = detect_save_cond(cond);
        let cond_s = self.render_expr(cond);
        writeln!(out, "{indent}if {cond_s} do").unwrap();
        // Post-save id rebind — Elixir's save/1 doesn't mutate, so
        // newly-inserted records lack their id. Gates on save-cond +
        // last_local_is_new + top-level resource (see doc above the
        // last_local_is_new field).
        if let Some(recv_s) = save_recv {
            if self.state.last_local_is_new && self.ctx.parent.is_none() {
                let inner_indent = "  ".repeat(depth + 1);
                writeln!(
                    out,
                    "{inner_indent}{recv_s} = %{{{recv_s} | id: Roundhouse.Db.last_insert_rowid()}}",
                )
                .unwrap();
            }
        }
        self.walk_stmt(then_branch, out, depth + 1, is_tail);
        if !crate::lower::is_empty_body(else_branch) {
            writeln!(out, "{indent}else").unwrap();
            self.walk_stmt(else_branch, out, depth + 1, is_tail);
        }
        writeln!(out, "{indent}end").unwrap();
    }

    fn write_update_if(
        &mut self,
        recv: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    ) {
        let recv_s = self.render_expr(recv);
        let permitted: Vec<String> = self.ctx.permitted.iter().cloned().collect();
        let resource = self.ctx.resource.to_string();
        for field in &permitted {
            writeln!(
                out,
                "{indent}{recv_s} = if Map.has_key?(context.params, \"{resource}[{field}]\"), do: %{{{recv_s} | {field}: context.params[\"{resource}[{field}]\"]}}, else: {recv_s}",
            )
            .unwrap();
            self.state.uses_context = true;
        }
        writeln!(out, "{indent}if {}.save({recv_s}) do", self.ctx.model_class).unwrap();
        // No post-save rebind here — Update flows load the record
        // via ModelFind, so the id is already set.
        self.walk_stmt(then_branch, out, depth + 1, is_tail);
        if !crate::lower::is_empty_body(else_branch) {
            writeln!(out, "{indent}else").unwrap();
            self.walk_stmt(else_branch, out, depth + 1, is_tail);
        }
        writeln!(out, "{indent}end").unwrap();
    }

    fn write_response_stmt(&mut self, r: &str, _is_tail: bool, indent: &str, out: &mut String) {
        writeln!(out, "{indent}{r}").unwrap();
    }

    fn write_expr_stmt(&mut self, s: &str, indent: &str, out: &mut String) {
        writeln!(out, "{indent}{s}").unwrap();
    }

    fn render_expr(&mut self, expr: &Expr) -> String {
        if let ExprNode::Send { recv, method, args, block, .. } = &*expr.node {
            if let Some(stmt) = self.render_send_stmt(
                recv.as_ref(), method.as_str(), args, block.as_ref(), "",
            ) {
                return match stmt {
                    crate::lower::Stmt::Response(r) => r,
                    crate::lower::Stmt::Expr(s) => s,
                };
            }
            let args_s: Vec<String> = args.iter().map(|a| self.render_expr(a)).collect();
            return match recv {
                None if args.is_empty() => method.to_string(),
                None => format!("{method}({})", args_s.join(", ")),
                Some(r) => {
                    // Elixir can't call `recv.method(args)` the way OO
                    // languages do. For known-model method calls on a
                    // local (e.g. `post.save` or `post.destroy`),
                    // rewrite to `Module.method(local, args...)`.
                    let recv_s = self.render_expr(r);
                    if is_bare_ex_ident(&recv_s) {
                        let all_args: Vec<String> = std::iter::once(recv_s)
                            .chain(args_s.iter().cloned())
                            .collect();
                        format!("{}.{method}({})", self.ctx.model_class, all_args.join(", "))
                    } else if args.is_empty() {
                        format!("{recv_s}.{method}")
                    } else {
                        format!("{recv_s}.{method}({})", args_s.join(", "))
                    }
                }
            };
        }
        if let ExprNode::Ivar { name } = &*expr.node {
            return name.to_string();
        }
        emit_expr(expr, None)
    }

    fn render_send_stmt(
        &mut self,
        recv: Option<&Expr>,
        method: &str,
        args: &[Expr],
        block: Option<&Expr>,
        // Elixir concurrency lives in BEAM processes; no async syntax,
        // prefix unused.
        _suspending_prefix: &str,
    ) -> Option<crate::lower::Stmt> {
        use crate::lower::{SendKind, Stmt};
        let kind = crate::lower::classify_controller_send(
            recv, method, args, block, self.ctx.known_models,
        )?;
        Some(match kind {
            SendKind::ParamsAccess => {
                self.state.uses_context = true;
                Stmt::Expr("context.params".to_string())
            }
            SendKind::ParamsIndex { key } => {
                self.state.uses_context = true;
                match &*key.node {
                    ExprNode::Lit { value: Literal::Sym { value: k } } => Stmt::Expr(
                        format!("String.to_integer(to_string(context.params[\"{}\"]))", k.as_str()),
                    ),
                    _ => {
                        let ks = self.render_expr(key);
                        Stmt::Expr(format!("context.params[{ks}]"))
                    }
                }
            }
            SendKind::ParamsExpect { args: pe_args } => {
                self.state.uses_context = true;
                match pe_args.first().map(|e| &*e.node) {
                    Some(ExprNode::Lit { value: Literal::Sym { value: k } }) => Stmt::Expr(
                        format!("String.to_integer(to_string(context.params[\"{}\"]))", k.as_str()),
                    ),
                    _ => Stmt::Expr("context.params # TODO: params.expect hash".to_string()),
                }
            }
            SendKind::ModelNew { class } => Stmt::Expr(format!("%{}{{}}", class.as_str())),
            SendKind::ModelFind { class, id } => {
                let id_s = self.render_expr(id);
                Stmt::Expr(format!("{0}.find({id_s}) || %{0}{{}}", class.as_str()))
            }
            SendKind::QueryChain { target: Some(target), method: cm, args: ca, recv: cr } => {
                let mut out = format!("{}.all()", target.as_str());
                let mods = crate::lower::collect_chain_modifiers(cm, ca, cr.as_deref());
                for m in mods {
                    out = view::apply_ex_chain_modifier(out, m);
                }
                Stmt::Expr(out)
            }
            SendKind::QueryChain { target: None, .. } => Stmt::Expr("[]".to_string()),
            SendKind::AssocLookup { target, outer_method } => match outer_method {
                "find" => {
                    let id_s = args.first().map(|a| self.render_expr(a))
                        .unwrap_or_else(|| "0".to_string());
                    Stmt::Expr(format!("{0}.find({id_s}) || %{0}{{}}", target.as_str()))
                }
                _ => Stmt::Expr(format!("%{}{{}}", target.as_str())),
            },
            SendKind::BangStrip { recv, stripped_method, args: bs_args } => {
                let recv_s = self.render_expr(recv);
                let mut call_args = vec![recv_s];
                for a in bs_args {
                    call_args.push(self.render_expr(a));
                }
                Stmt::Expr(format!(
                    "{}.{stripped_method}({})",
                    self.ctx.model_class,
                    call_args.join(", "),
                ))
            }
            SendKind::InstanceUpdate => Stmt::Expr("false".to_string()),
            SendKind::PathOrUrlHelper => {
                let helper = method.strip_suffix("_path").or_else(|| method.strip_suffix("_url"))
                    .unwrap_or(method);
                Stmt::Expr(format!("{helper}_path()"))
            }
            SendKind::Render { args: r_args } => Stmt::Response(self.render_ex_render(r_args)),
            SendKind::RedirectTo { args: r_args } => Stmt::Response(self.render_ex_redirect(r_args)),
            SendKind::Head { args: h_args } => {
                let status = h_args.first().and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Sym { value: s } } =>
                        Some(crate::lower::status_sym_to_code(s.as_str())),
                    ExprNode::Lit { value: Literal::Int { value: n } } => Some(*n as u16),
                    _ => None,
                }).unwrap_or(200);
                Stmt::Response(format!("%ActionResponse{{status: {status}}}"))
            }
            SendKind::RespondToBlock { .. }
            | SendKind::FormatHtml { .. }
            | SendKind::FormatJson => Stmt::Expr(
                "%ActionResponse{} # unreachable: respond_to not normalized".to_string(),
            ),
        })
    }
}

impl<'a> ExEmitter<'a> {
    fn render_ex_render(&mut self, args: &[Expr]) -> String {
        if let Some(first) = args.first() {
            if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*first.node {
                let view_fn = ex_view_fn(self.ctx.model_class, sym.as_str());
                let arg = self.state.last_local.clone().unwrap_or_else(|| "nil".to_string());
                let body_part = format!("body: App.Views.{view_fn}({arg})");
                return match crate::lower::extract_status_from_kwargs(&args[1..]) {
                    Some(status) => format!("%ActionResponse{{status: {status}, {body_part}}}"),
                    None => format!("%ActionResponse{{{body_part}}}"),
                };
            }
            let body_s = self.render_expr(first);
            return format!("%ActionResponse{{body: {body_s}}}");
        }
        "%ActionResponse{}".to_string()
    }

    fn render_ex_redirect(&mut self, args: &[Expr]) -> String {
        let Some(first) = args.first() else {
            return "%ActionResponse{status: 303}".to_string();
        };
        let loc = self.render_expr(first);
        let status = crate::lower::extract_status_from_kwargs(&args[1..]).unwrap_or(303);
        if is_bare_ex_ident(&loc) {
            let id_access = format!("{loc}.id");
            return format!(
                "%ActionResponse{{status: {status}, location: {loc}_path({id_access})}}",
            );
        }
        format!("%ActionResponse{{status: {status}, location: {loc}}}")
    }
}

/// If `cond` is the shape `<recv>.save` (a Send where we'd rewrite
/// to `Model.save(recv)`), return the receiver's emitted string —
/// callers use it to synthesize the post-save id rebind Elixir
/// needs. Returns None for any other cond shape.
fn detect_save_cond(cond: &Expr) -> Option<String> {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "save" || !args.is_empty() {
        return None;
    }
    // Must be on a local (ivar rewritten to Var, or still Var).
    match &*r.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.to_string()),
        _ => None,
    }
}

fn is_bare_ex_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return false; }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first == b'_') { return false; }
    bytes.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
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
                .map(|(c, a)| (controller_class_name(c), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            writeln!(out, "    Router.root({controller}, :{action})").unwrap();
        }
        RouteSpec::Resources { name, only, except: _, nested } => {
            let controller = controller_class_name(name.as_str());
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
    let controller = controller_class_name(name.as_str());
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

fn emit_expr(e: &Expr, receiver_arg: Option<&str>) -> String {
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

// Fixtures + tests ---------------------------------------------------

fn emit_ex_fixture(lowered: &crate::lower::LoweredFixture) -> EmittedFile {
    let fixture_name = lowered.name.as_str();
    let class_name = lowered.class.0.as_str();
    let ns = crate::naming::camelize(fixture_name);

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Fixtures.{ns} do").unwrap();

    // _load_all — invoked from Fixtures.setup in the test setup
    // callback. Each record runs through Class.save/1 (validations +
    // INSERT) then Roundhouse.Db.last_insert_rowid captures the
    // autoincrement id — exposed via the shared fixture-id map.
    writeln!(s).unwrap();
    writeln!(s, "  def _load_all do").unwrap();
    for record in &lowered.records {
        let label = record.label.as_str();
        let mut field_lines: Vec<String> = Vec::new();
        for field in &record.fields {
            let col = field.column.as_str();
            let val = match &field.value {
                crate::lower::LoweredFixtureValue::Literal { ty, raw } => {
                    ex_literal_for(raw, ty)
                }
                crate::lower::LoweredFixtureValue::FkLookup {
                    target_fixture,
                    target_label,
                } => format!(
                    "Fixtures.fixture_id({:?}, {:?})",
                    target_fixture.as_str(),
                    target_label.as_str(),
                ),
            };
            field_lines.push(format!("      {col}: {val}"));
        }
        writeln!(s, "    record = %{class_name}{{").unwrap();
        for (idx, line) in field_lines.iter().enumerate() {
            if idx < field_lines.len() - 1 {
                writeln!(s, "{line},").unwrap();
            } else {
                writeln!(s, "{line}").unwrap();
            }
        }
        writeln!(s, "    }}").unwrap();
        writeln!(
            s,
            "    unless {class_name}.save(record), do: raise \"fixture {fixture_name}/{label} failed to save\""
        )
        .unwrap();
        writeln!(
            s,
            "    Fixtures.register({fixture_name:?}, {label:?}, Roundhouse.Db.last_insert_rowid())"
        )
        .unwrap();
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();

    // Named getters — look up id and Class.find.
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s).unwrap();
        writeln!(s, "  def {label}() do").unwrap();
        writeln!(
            s,
            "    id = Fixtures.fixture_id({fixture_name:?}, {label:?})"
        )
        .unwrap();
        writeln!(s, "    {class_name}.find(id)").unwrap();
        writeln!(s, "  end").unwrap();
    }

    writeln!(s, "end").unwrap();

    EmittedFile {
        path: PathBuf::from(format!("test/support/fixtures/{fixture_name}.ex")),
        content: s,
    }
}

/// `test/support/fixtures.ex` — shared Fixtures module with setup,
/// fixture_id lookup, and the register helper the per-class
/// _load_all functions call.
fn emit_ex_fixtures_helper(lowered: &crate::lower::LoweredFixtureSet) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Fixtures do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  @doc \"Per-test setup. Called from each test module's ExUnit setup.\"").unwrap();
    writeln!(s, "  def setup do").unwrap();
    writeln!(
        s,
        "    Roundhouse.Db.setup_test_db(Roundhouse.SchemaSQL.create_tables())"
    )
    .unwrap();
    writeln!(s, "    Process.put(:roundhouse_fixture_ids, %{{}})").unwrap();
    for f in &lowered.fixtures {
        let ns = crate::naming::camelize(f.name.as_str());
        writeln!(s, "    Fixtures.{ns}._load_all()").unwrap();
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def register(fixture, label, id) do").unwrap();
    writeln!(s, "    ids = Process.get(:roundhouse_fixture_ids, %{{}})").unwrap();
    writeln!(
        s,
        "    Process.put(:roundhouse_fixture_ids, Map.put(ids, {{fixture, label}}, id))"
    )
    .unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def fixture_id(fixture, label) do").unwrap();
    writeln!(
        s,
        "    case Map.get(Process.get(:roundhouse_fixture_ids, %{{}}), {{fixture, label}}) do"
    )
    .unwrap();
    writeln!(s, "      nil -> raise \"fixture #{{fixture}}/#{{label}} not loaded\"").unwrap();
    writeln!(s, "      id -> id").unwrap();
    writeln!(s, "    end").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("test/support/fixtures.ex"),
        content: s,
    }
}

fn ex_literal_for(value: &str, ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                value.to_string()
            } else {
                format!("0 # TODO: coerce {value:?}")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                value.to_string()
            } else {
                format!("0.0 # TODO: coerce {value:?}")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "true".into(),
            "false" | "0" => "false".into(),
            _ => format!("false # TODO: coerce {value:?}"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}

fn emit_ex_test(tm: &TestModule, app: &App) -> EmittedFile {
    let fixture_names: Vec<Symbol> =
        app.fixtures.iter().map(|f| f.name.clone()).collect();
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let mut attrs_set: std::collections::BTreeSet<Symbol> =
        std::collections::BTreeSet::new();
    for m in &app.models {
        for attr in m.attributes.fields.keys() {
            attrs_set.insert(attr.clone());
        }
    }
    let model_attrs: Vec<Symbol> = attrs_set.into_iter().collect();

    let ctx = ExTestCtx {
        app,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
    };

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {} do", tm.name.0).unwrap();
    writeln!(s, "  use ExUnit.Case").unwrap();
    if is_controller_test {
        writeln!(s, "  alias Roundhouse.TestClient").unwrap();
        writeln!(s, "  alias Roundhouse.TestResponse").unwrap();
        writeln!(s, "  import Roundhouse.RouteHelpers").unwrap();
    }
    // Each test starts on a fresh :memory: SQLite DB with all
    // fixtures loaded — Rails' transactional-fixture isolation
    // adapted to Elixir's per-process test semantics.
    if !app.fixtures.is_empty() {
        writeln!(s).unwrap();
        writeln!(s, "  setup do").unwrap();
        if is_controller_test {
            writeln!(s, "    App.Routes.register()").unwrap();
        }
        writeln!(s, "    Fixtures.setup()").unwrap();
        writeln!(s, "    :ok").unwrap();
        writeln!(s, "  end").unwrap();
    }

    for test in &tm.tests {
        writeln!(s).unwrap();
        if is_controller_test {
            writeln!(s, "  test {:?} do", test.name).unwrap();
            let body = emit_ex_controller_test_body(test, app, ctx);
            if body.is_empty() {
                writeln!(s, "    :ok").unwrap();
            } else {
                for line in body.lines() {
                    writeln!(s, "    {line}").unwrap();
                }
            }
            writeln!(s, "  end").unwrap();
        } else if test_needs_runtime_unsupported_ex(test) {
            writeln!(s, "  @tag :skip").unwrap();
            writeln!(s, "  test {:?} do", test.name).unwrap();
            writeln!(s, "    # Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "  end").unwrap();
        } else {
            writeln!(s, "  test {:?} do", test.name).unwrap();
            let body = emit_ex_test_body(&test.body, ctx);
            for line in body.lines() {
                writeln!(s, "    {line}").unwrap();
            }
            writeln!(s, "  end").unwrap();
        }
    }

    writeln!(s, "end").unwrap();

    let filename = snake_case(tm.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("test/{filename}.exs")),
        content: s,
    }
}

#[derive(Clone, Copy)]
struct ExTestCtx<'a> {
    app: &'a App,
    fixture_names: &'a [Symbol],
    known_models: &'a [Symbol],
    model_attrs: &'a [Symbol],
}

fn emit_ex_test_body(body: &Expr, ctx: ExTestCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|e| emit_ex_test_stmt(e, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_ex_test_stmt(body, ctx),
    }
}

fn emit_ex_test_stmt(e: &Expr, ctx: ExTestCtx) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_ex_test_expr(value, ctx))
        }
        _ => emit_ex_test_expr(e, ctx),
    }
}

fn emit_ex_test_expr(e: &Expr, ctx: ExTestCtx) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_ex_test_send(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!(
                "{} {op_s} {}",
                emit_ex_test_expr(left, ctx),
                emit_ex_test_expr(right, ctx)
            )
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_ex_test_expr(v, ctx))
                    } else {
                        format!(
                            "{} => {}",
                            emit_ex_test_expr(k, ctx),
                            emit_ex_test_expr(v, ctx),
                        )
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        _ => format!("# TODO: Elixir test emit for {:?}", std::mem::discriminant(&*e.node)),
    }
}

fn emit_ex_test_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: ExTestCtx,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_ex_test_expr(a, ctx)).collect();

    // Fixture accessor: articles(:one) → Fixtures.Articles.one()
    if recv.is_none()
        && args.len() == 1
        && ctx.fixture_names.iter().any(|s| s.as_str() == method)
    {
        if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*args[0].node {
            let ns = crate::naming::camelize(method);
            return format!("Fixtures.{ns}.{}()", sym.as_str());
        }
    }

    // assert_difference("Class.count", delta) do ... end →
    // `_before = ...; <body>; _after = ...; assert _after - _before == delta`.
    if recv.is_none() && method == "assert_difference" {
        if let Some(body) = block {
            if let Some(count_expr) = args
                .first()
                .and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => {
                        rewrite_ruby_dot_call_ex(value)
                    }
                    _ => None,
                })
            {
                let delta = args_s.get(1).cloned().unwrap_or_else(|| "1".into());
                let body_s = emit_block_body_ex(body, ctx);
                return format!(
                    "count_before = {count_expr}\n{body_s}\ncount_after = {count_expr}\nassert count_after - count_before == {delta}"
                );
            }
        }
    }

    // owner.<assoc>.create(hash) / .build(hash) — HasMany rewrite
    // wrapped in an anonymous function so the record (with its
    // assigned id) becomes available as the expression's value.
    if (method == "create" || method == "build") && args.len() == 1 {
        if let Some(outer_recv) = recv {
            if let ExprNode::Send {
                recv: Some(assoc_recv),
                method: assoc_method,
                args: inner_args,
                ..
            } = &*outer_recv.node
            {
                if inner_args.is_empty() {
                    if let Some(s) = try_emit_assoc_create_ex(
                        assoc_recv,
                        assoc_method.as_str(),
                        args,
                        method,
                        ctx,
                    ) {
                        return s;
                    }
                }
            }
        }
    }

    // Assertion macros → ExUnit's assert/refute.
    if recv.is_none() {
        match (method, args_s.len()) {
            ("assert_equal", 2) => {
                return format!(
                    "assert {actual} == {expected}",
                    expected = args_s[0],
                    actual = args_s[1]
                );
            }
            ("assert_not_equal", 2) => {
                return format!(
                    "refute {actual} == {expected}",
                    expected = args_s[0],
                    actual = args_s[1]
                );
            }
            ("assert_not", 1) => return format!("refute {}", args_s[0]),
            ("assert", 1) => return format!("assert {}", args_s[0]),
            ("assert_nil", 1) => return format!("assert is_nil({})", args_s[0]),
            ("assert_not_nil", 1) => return format!("refute is_nil({})", args_s[0]),
            _ => {}
        }
    }

    // `Class.new(hash)` → `%Class{ k: v, ... }` struct literal.
    if let Some(r) = recv {
        if method == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class_name) = path.last() {
                    if ctx.known_models.iter().any(|s| s == class_name) {
                        if let ExprNode::Hash { entries, .. } = &*args[0].node {
                            let pairs: Vec<String> = entries
                                .iter()
                                .filter_map(|(k, v)| {
                                    if let ExprNode::Lit {
                                        value: Literal::Sym { value: f },
                                    } = &*k.node
                                    {
                                        Some(format!(
                                            "{}: {}",
                                            f.as_str(),
                                            emit_ex_test_expr(v, ctx)
                                        ))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            return format!("%{} {{{}}}", class_name, pairs.join(", "));
                        }
                    }
                }
            }
        }
    }

    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_ex_test_expr(r, ctx);
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            if is_class_call {
                // Module.function(args) — Elixir's module-function-call form.
                if args_s.is_empty() {
                    format!("{recv_s}.{method}()")
                } else {
                    format!("{recv_s}.{method}({})", args_s.join(", "))
                }
            } else {
                let is_attr = args_s.is_empty()
                    && ctx.model_attrs.iter().any(|s| s.as_str() == method);
                if is_attr {
                    format!("{recv_s}.{method}")
                } else if matches!(method, "save" | "destroy") && args_s.is_empty() {
                    // Instance-method-like call on a model record:
                    // `article.save` / `article.destroy` in Ruby →
                    // `Module.save(article)` / `Module.destroy(article)`.
                    format!("{}(\n      {recv_s}\n    )", ex_module_fn_for(r, method))
                } else if args_s.is_empty() {
                    format!("{recv_s}.{method}")
                } else {
                    format!("{recv_s}.{method}({})", args_s.join(", "))
                }
            }
        }
    }
}

/// Parse a Ruby-style `"Class.method"` expression into Elixir
/// `Class.method()`. Capitalized LHS → module function call;
/// lowercase LHS → instance field access (same as in the other
/// targets).
fn rewrite_ruby_dot_call_ex(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    let (lhs, rhs) = trimmed.split_once('.')?;
    let is_ident = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if !is_ident(lhs) || !is_ident(rhs) {
        return None;
    }
    let is_class = lhs.chars().next().is_some_and(|c| c.is_uppercase());
    if is_class {
        Some(format!("{lhs}.{rhs}()"))
    } else {
        Some(format!("{lhs}.{rhs}"))
    }
}

/// Render a Ruby block body as Elixir statements, peeling one
/// Lambda layer. Ruby `do ... end` lowers to `ExprNode::Lambda`.
fn emit_block_body_ex(e: &Expr, ctx: ExTestCtx) -> String {
    let inner = match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    };
    match &*inner.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|s| emit_ex_test_stmt(s, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_ex_test_stmt(inner, ctx),
    }
}

fn try_emit_assoc_create_ex(
    owner: &Expr,
    assoc_name: &str,
    args: &[Expr],
    outer_method: &str,
    ctx: ExTestCtx,
) -> Option<String> {
    let resolved = crate::lower::resolve_has_many(
        &Symbol::from(assoc_name),
        owner.ty.as_ref(),
        ctx.app,
    )?;
    let target_class = resolved.target_class.0.as_str();
    let foreign_key = resolved.foreign_key.as_str();

    let owner_s = emit_ex_test_expr(owner, ctx);
    let hash_entries = match &args.first()?.node.as_ref() {
        ExprNode::Hash { entries, .. } => entries.clone(),
        _ => return None,
    };

    let mut pairs: Vec<String> = vec![format!("{foreign_key}: {owner_s}.id")];
    for (k, v) in &hash_entries {
        if let ExprNode::Lit { value: Literal::Sym { value: field_name } } = &*k.node {
            pairs.push(format!(
                "{}: {}",
                field_name.as_str(),
                emit_ex_test_expr(v, ctx),
            ));
        }
    }
    let struct_lit = format!("%{target_class}{{{}}}", pairs.join(", "));
    // Elixir has no direct "block expression" — an anonymous function
    // call is the standard way to evaluate a sequence and yield the
    // last value. `.create` saves and returns the record with its id
    // assigned; `.build` just constructs.
    if outer_method == "create" {
        Some(format!(
            "(fn ->\n      record = {struct_lit}\n      {target_class}.save(record)\n      %{{record | id: Roundhouse.Db.last_insert_rowid()}}\n    end).()"
        ))
    } else {
        Some(struct_lit)
    }
}

/// Guess the module-function path for `recv.method`. If recv is a
/// struct, the struct's module holds the function — so `article.save`
/// emits as `Article.save(article)`. Without type info we scrape the
/// variable name and infer.
fn ex_module_fn_for(recv: &Expr, method: &str) -> String {
    if let ExprNode::Var { name, .. } = &*recv.node {
        let n = name.as_str();
        // Variable names for fixtures follow the snake-cased class name.
        let camelized = crate::naming::camelize(n);
        return format!("{camelized}.{method}");
    }
    format!("??.{method}") // fallback; should be rare
}

fn test_needs_runtime_unsupported_ex(_test: &Test) -> bool {
    // Phase 3 rounded out Elixir's real-blog coverage. Keep as a
    // future-guard; no current pattern forces a skip.
    false
}

// Pass-2 controller-test emit ------------------------------------------
//
// Walks the test AST via the shared classifier + renders each
// statement through an Elixir-specific assertion render table.

fn emit_ex_controller_test_body(test: &Test, app: &App, ctx: ExTestCtx) -> String {
    let mut out = String::new();
    // Prime ivars read without assignment: `@article` → `article = Fixtures.Articles.one()`.
    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(ivar.as_str());
        if ctx.fixture_names.iter().any(|s| s.as_str() == plural) {
            let ns = crate::naming::camelize(&plural);
            out.push_str(&format!("{} = Fixtures.{}.one()\n", ivar.as_str(), ns));
        }
    }

    for stmt in crate::lower::test_body_stmts(&test.body) {
        let rendered = emit_ex_ctrl_test_stmt(stmt, app, ctx);
        out.push_str(&rendered);
        out.push('\n');
    }
    out
}

fn emit_ex_ctrl_test_stmt(stmt: &Expr, app: &App, ctx: ExTestCtx) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ex_ctrl_test_send(method.as_str(), args, block.as_ref(), app, ctx)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ex_ctrl_test_expr(r, app, ctx),
                };
                let module = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => {
                        crate::naming::camelize(name.as_str())
                    }
                    _ => "Unknown".to_string(),
                };
                return format!("{recv_s} = {module}.reload({recv_s})");
            }
            let recv_s = emit_ex_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                format!("{recv_s}.{}", method.as_str())
            } else {
                format!("{recv_s}.{}({})", method.as_str(), args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
        | ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{name} = {}", emit_ex_ctrl_test_expr(value, app, ctx))
        }
        _ => emit_ex_ctrl_test_expr(stmt, app, ctx),
    }
}

fn emit_ex_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            format!("resp = TestClient.get({u})")
        }
        Some(ControllerTestSend::HttpWrite { method: m, url, params }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            let body = params
                .map(|h| flatten_ex_params_to_form(h, None, app, ctx))
                .unwrap_or_else(|| "%{}".to_string());
            format!("resp = TestClient.{m}({u}, {body})")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            format!("resp = TestClient.delete({u})")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "TestResponse.assert_ok(resp)".to_string(),
            "unprocessable_entity" => "TestResponse.assert_unprocessable(resp)".to_string(),
            other => format!("TestResponse.assert_status(resp, 200) # TODO: {other:?}"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            format!("TestResponse.assert_redirected_to(resp, {u})")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_ex_assert_select_classified(selector, kind, app, ctx)
        }
        Some(ControllerTestSend::AssertDifference {
            method: _,
            count_expr,
            delta,
            block,
        }) => emit_ex_assert_difference_classified(count_expr, delta, block, app, ctx),
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_ex_ctrl_test_expr(expected, app, ctx);
            let a = emit_ex_ctrl_test_expr(actual, app, ctx);
            format!("assert {a} == {e}")
        }
        None => {
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_ex_url_expr(expr: &Expr, app: &App, ctx: ExTestCtx) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_ex_ctrl_test_expr(expr, app, ctx);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last().id", class.as_str()),
            UrlArg::Raw(e) => emit_ex_ctrl_test_expr(e, app, ctx),
        })
        .collect();
    if args_s.is_empty() {
        format!("{helper_name}()")
    } else {
        format!("{helper_name}({})", args_s.join(", "))
    }
}

fn emit_ex_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } =
        &*selector_expr.node
    else {
        return format!(
            "TestResponse.assert_select(resp, {}) # TODO: dynamic selector",
            emit_ex_ctrl_test_expr(selector_expr, app, ctx),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_ex_ctrl_test_expr(expr, app, ctx);
            format!("TestResponse.assert_select_text(resp, {selector:?}, {text})")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_ex_ctrl_test_expr(expr, app, ctx);
            format!("TestResponse.assert_select_min(resp, {selector:?}, {n})")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("TestResponse.assert_select(resp, {selector:?})\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in crate::lower::test_body_stmts(inner_body) {
                out.push_str(&emit_ex_ctrl_test_stmt(stmt, app, ctx));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("TestResponse.assert_select(resp, {selector:?})")
        }
    }
}

fn emit_ex_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    // `Article.count` → `Article.count()` in Elixir.
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}.{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("count_before = {count_expr}\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in crate::lower::test_body_stmts(inner_body) {
            out.push_str(&emit_ex_ctrl_test_stmt(stmt, app, ctx));
            out.push('\n');
        }
    }
    out.push_str(&format!("count_after = {count_expr}\n"));
    out.push_str(&format!("assert count_after - count_before == {expected_delta}"));
    out
}

fn emit_ex_ctrl_test_expr(expr: &Expr, app: &App, ctx: ExTestCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last()");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count()");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ex_ctrl_test_expr(r, app, ctx),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_ex_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_ex_url_expr(expr, app, ctx);
            }
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit {
                        value: Literal::Sym { value },
                    } = &*k.node
                    {
                        format!("{value}: {}", emit_ex_ctrl_test_expr(v, app, ctx))
                    } else {
                        format!(
                            "{} => {}",
                            emit_ex_ctrl_test_expr(k, app, ctx),
                            emit_ex_ctrl_test_expr(v, app, ctx),
                        )
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        _ => "nil # TODO expr".to_string(),
    }
}

fn flatten_ex_params_to_form(
    expr: &Expr,
    scope: Option<&str>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_ex_ctrl_test_expr(value, app, ctx);
            format!("{key:?} => to_string({val})")
        })
        .collect();
    format!("%{{{}}}", pairs.join(", "))
}

#[allow(dead_code)]

fn test_body_uses_unsupported_ex(e: &Expr) -> bool {
    use crate::expr::InterpPart;
    let self_hit = match &*e.node {
        ExprNode::Send { recv, method, .. } => {
            let m = method.as_str();
            matches!(
                m,
                "assert_difference"
                    | "destroy"
                    | "destroy!"
                    | "build"
                    | "create"
                    | "create!"
            ) || (m == "count"
                && recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. })))
        }
        _ => false,
    };
    if self_hit {
        return true;
    }
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if test_body_uses_unsupported_ex(r) {
                    return true;
                }
            }
            for a in args {
                if test_body_uses_unsupported_ex(a) {
                    return true;
                }
            }
            if let Some(b) = block {
                if test_body_uses_unsupported_ex(b) {
                    return true;
                }
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                if test_body_uses_unsupported_ex(e) {
                    return true;
                }
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                if test_body_uses_unsupported_ex(k) || test_body_uses_unsupported_ex(v) {
                    return true;
                }
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    if test_body_uses_unsupported_ex(expr) {
                        return true;
                    }
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            if test_body_uses_unsupported_ex(left) || test_body_uses_unsupported_ex(right) {
                return true;
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            if test_body_uses_unsupported_ex(cond)
                || test_body_uses_unsupported_ex(then_branch)
                || test_body_uses_unsupported_ex(else_branch)
            {
                return true;
            }
        }
        ExprNode::Let { value, body, .. } => {
            if test_body_uses_unsupported_ex(value) || test_body_uses_unsupported_ex(body) {
                return true;
            }
        }
        ExprNode::Lambda { body, .. } => {
            if test_body_uses_unsupported_ex(body) {
                return true;
            }
        }
        ExprNode::Assign { value, .. } => {
            if test_body_uses_unsupported_ex(value) {
                return true;
            }
        }
        _ => {}
    }
    false
}
