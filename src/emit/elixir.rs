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
    Action, Controller, MethodDef, Model, RouteSpec, Test, TestModule,
};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::naming::snake_case;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/elixir/runtime.ex");
const DB_SOURCE: &str = include_str!("../../runtime/elixir/db.ex");

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
        files.push(emit_model_file(model, app));
    }
    for controller in &app.controllers {
        files.push(emit_controller_file(controller));
    }
    if !app.routes.entries.is_empty() {
        files.push(emit_router_file(app));
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
      {:exqlite, \"~> 0.30\"}
    ]
  end

  # Compile only models in Phase 1. Controllers + router are emitted
  # as files but reference runtime that doesn't exist yet; including
  # them here would block compile. Test env additionally includes
  # test/support/ so fixtures are compiled alongside the app.
  defp elixirc_paths(:test) do
    (Path.wildcard(\"lib/**/*.ex\") ++ Path.wildcard(\"test/support/**/*.ex\"))
    |> Enum.reject(&excluded?/1)
  end

  defp elixirc_paths(_) do
    Path.wildcard(\"lib/**/*.ex\")
    |> Enum.reject(&excluded?/1)
  end

  defp excluded?(p) do
    String.ends_with?(p, \"_controller.ex\") or String.ends_with?(p, \"router.ex\")
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

// Models ---------------------------------------------------------------

fn emit_model_file(model: &Model, app: &App) -> EmittedFile {
    let module = model.name.0.as_str();
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {module} do").unwrap();

    // Struct declaration with typed defaults so NOT NULL columns
    // get concrete values (empty strings for text, 0 for ints)
    // rather than nil. SQLite rejects NULL → NOT NULL at INSERT
    // time, and the fixture harness calls save before every
    // non-id field is explicitly set.
    let fields: Vec<String> = model
        .attributes
        .fields
        .iter()
        .map(|(k, ty)| format!("{}: {}", k.as_str(), ex_default_for(ty)))
        .collect();
    if !fields.is_empty() {
        writeln!(s, "  defstruct [{}]", fields.join(", ")).unwrap();
    } else {
        writeln!(s, "  defstruct []").unwrap();
    }

    let attrs: Vec<Symbol> = model.attributes.fields.keys().cloned().collect();
    for method in model.methods() {
        writeln!(s).unwrap();
        emit_model_method_with_attrs(&mut s, module, method, &attrs);
    }

    let lowered = crate::lower::lower_validations(model);
    if !lowered.is_empty() {
        writeln!(s).unwrap();
        emit_validate_method_ex(&mut s, &lowered);
    }
    // Skip persistence for abstract base classes (no columns beyond
    // `id`) — ApplicationRecord etc.
    let has_table = model
        .attributes
        .fields
        .keys()
        .any(|k| k.as_str() != "id");
    if has_table {
        writeln!(s).unwrap();
        emit_persistence_methods_ex(&mut s, module, model, !lowered.is_empty(), app);
    }

    writeln!(s, "end").unwrap();
    let fname = format!("lib/{}.ex", snake_case(module));
    EmittedFile { path: PathBuf::from(fname), content: s }
}

/// Render save/destroy/count/find for a module against the test DB.
/// Naming: `save/1` and `destroy/1` take the record; `count/0` and
/// `find/1` are module functions. Matches Elixir's functional shape
/// (no implicit receiver) and what our spec emit expects.
fn emit_persistence_methods_ex(
    out: &mut String,
    module: &str,
    model: &Model,
    has_validate: bool,
    app: &App,
) {
    let lp = crate::lower::lower_persistence(model, app);
    let recv = module_receiver_name(module);

    let insert_sql = positional_placeholders_ex(&lp.insert_sql);
    let update_sql = positional_placeholders_ex(&lp.update_sql);
    let delete_sql = positional_placeholders_ex(&lp.delete_sql);
    let select_by_id_sql = positional_placeholders_ex(&lp.select_by_id_sql);

    let non_id_args: Vec<String> = lp
        .non_id_columns
        .iter()
        .map(|s| format!("{recv}.{}", s.as_str()))
        .collect();

    // ----- save/1 -----
    writeln!(out, "  def save({recv}) do").unwrap();
    if has_validate {
        writeln!(out, "    cond do").unwrap();
        writeln!(out, "      validate({recv}) != [] ->").unwrap();
        writeln!(out, "        false").unwrap();
        // belongs_to existence checks, chained into the cond.
        for check in &lp.belongs_to_checks {
            let fk = check.foreign_key.as_str();
            let target = check.target_class.0.as_str();
            writeln!(
                out,
                "      {recv}.{fk} == nil or {recv}.{fk} == 0 or {target}.find({recv}.{fk}) == nil ->",
            )
            .unwrap();
            writeln!(out, "        false").unwrap();
        }
        writeln!(out, "      true ->").unwrap();
        writeln!(out, "        do_save({recv})").unwrap();
        writeln!(out, "    end").unwrap();
        writeln!(out, "  end").unwrap();
    } else {
        writeln!(out, "    do_save({recv})").unwrap();
        writeln!(out, "  end").unwrap();
    }

    writeln!(out).unwrap();
    writeln!(out, "  defp do_save({recv}) do").unwrap();
    writeln!(out, "    if {recv}.id == nil or {recv}.id == 0 do").unwrap();
    writeln!(
        out,
        "      _id = Roundhouse.Db.execute({insert_sql:?}, [{}])",
        non_id_args.join(", "),
    )
    .unwrap();
    writeln!(out, "    else").unwrap();
    writeln!(
        out,
        "      _id = Roundhouse.Db.execute({update_sql:?}, [{}, {recv}.id])",
        non_id_args.join(", "),
    )
    .unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "    true").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- destroy/1 -----
    writeln!(out).unwrap();
    writeln!(out, "  def destroy({recv}) do").unwrap();
    for dc in &lp.dependent_children {
        let child_class = dc.child_class.0.as_str();
        let child_select = positional_placeholders_ex(&dc.select_by_parent_sql);
        writeln!(
            out,
            "    rows = Roundhouse.Db.query_all({child_select:?}, [{recv}.id])",
        )
        .unwrap();
        writeln!(out, "    Enum.each(rows, fn row ->").unwrap();
        writeln!(out, "      [{}] = row", dc
            .child_columns
            .iter()
            .map(|c| c.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", "))
            .unwrap();
        let fields: Vec<String> = dc
            .child_columns
            .iter()
            .map(|c| format!("{0}: {0}", c.as_str()))
            .collect();
        writeln!(
            out,
            "      child = %{child_class}{{{}}}",
            fields.join(", ")
        )
        .unwrap();
        writeln!(out, "      {child_class}.destroy(child)").unwrap();
        writeln!(out, "    end)").unwrap();
    }
    writeln!(
        out,
        "    _ = Roundhouse.Db.execute({delete_sql:?}, [{recv}.id])",
    )
    .unwrap();
    writeln!(out, "    :ok").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- count/0 -----
    writeln!(out).unwrap();
    writeln!(out, "  def count do").unwrap();
    writeln!(
        out,
        "    Roundhouse.Db.scalar({:?}, [])",
        lp.count_sql,
    )
    .unwrap();
    writeln!(out, "  end").unwrap();

    // ----- find/1 -----
    writeln!(out).unwrap();
    writeln!(out, "  def find(id) do").unwrap();
    writeln!(
        out,
        "    case Roundhouse.Db.query_one({select_by_id_sql:?}, [id]) do",
    )
    .unwrap();
    let field_list: Vec<String> = lp
        .columns
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    writeln!(out, "      [{}] ->", field_list.join(", ")).unwrap();
    let struct_fields: Vec<String> = lp
        .columns
        .iter()
        .map(|c| format!("{0}: {0}", c.as_str()))
        .collect();
    writeln!(
        out,
        "        %{module}{{{}}}",
        struct_fields.join(", ")
    )
    .unwrap();
    writeln!(out, "      nil ->").unwrap();
    writeln!(out, "        nil").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();
}

/// Elixir defstruct default for a field typed by the schema. Mirrors
/// TS/Crystal: `""` for strings/text/time, `0` for ints, `0.0` for
/// floats, `false` for bools, `nil` otherwise. The id column still
/// starts at nil — save distinguishes "unsaved" from "saved" by
/// checking nil-or-0 so both work.
fn ex_default_for(ty: &crate::ty::Ty) -> &'static str {
    use crate::ty::Ty;
    match ty {
        Ty::Int => "nil",
        Ty::Float => "0.0",
        Ty::Bool => "false",
        Ty::Str | Ty::Sym => "\"\"",
        Ty::Class { id, .. } if id.0.as_str() == "Time" => "\"\"",
        _ => "nil",
    }
}

/// SQLite `?N` → exqlite/sqlite positional `?`. Same workaround as
/// Crystal/Go/TS — driver-level quirk, absorbed at emit time.
fn positional_placeholders_ex(sql: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '?' {
            out.push('?');
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn emit_validate_method_ex(
    out: &mut String,
    validations: &[crate::lower::LoweredValidation],
) {
    writeln!(out, "  def validate(record) do").unwrap();
    writeln!(out, "    errors = []").unwrap();
    for lv in validations {
        for check in &lv.checks {
            emit_check_inline_ex(out, lv.attribute.as_str(), check);
        }
    }
    writeln!(out, "    errors").unwrap();
    writeln!(out, "  end").unwrap();
}

fn emit_check_inline_ex(out: &mut String, attr: &str, check: &crate::lower::Check) {
    use crate::lower::Check;
    let msg = check.default_message();
    let push = |cond: &str| -> String {
        format!(
            "    errors =\n      if {cond} do\n        [Roundhouse.ValidationError.new({attr:?}, {msg:?}) | errors]\n      else\n        errors\n      end"
        )
    };
    let block = match check {
        Check::Presence => push(&format!("record.{attr} == \"\" or record.{attr} == nil")),
        Check::Absence => push(&format!("record.{attr} != \"\" and record.{attr} != nil")),
        Check::MinLength { n } => {
            push(&format!("record.{attr} == nil or String.length(record.{attr}) < {n}"))
        }
        Check::MaxLength { n } => {
            push(&format!("record.{attr} != nil and String.length(record.{attr}) > {n}"))
        }
        Check::GreaterThan { threshold } => {
            push(&format!("record.{attr} <= {threshold}"))
        }
        Check::LessThan { threshold } => push(&format!("record.{attr} >= {threshold}")),
        Check::OnlyInteger => {
            format!("    # OnlyInteger on {attr:?} — no-op, Elixir has no implicit coercion")
        }
        Check::Inclusion { values } => {
            let parts: Vec<String> = values.iter().map(inclusion_value_to_ex).collect();
            push(&format!("record.{attr} not in [{}]", parts.join(", ")))
        }
        Check::Format { pattern } => {
            format!("    # TODO: Format check on {attr:?} requires Regex ({pattern:?})")
        }
        Check::Uniqueness { .. } => {
            format!("    # TODO: Uniqueness on {attr:?} requires DB access at runtime")
        }
        Check::Custom { method } => {
            format!("    errors = {method}(record, errors)", method = method.as_str())
        }
    };
    writeln!(out, "{block}").unwrap();
}

fn inclusion_value_to_ex(v: &crate::lower::InclusionValue) -> String {
    use crate::lower::InclusionValue;
    match v {
        InclusionValue::Str { value } => format!("{value:?}"),
        InclusionValue::Int { value } => value.to_string(),
        InclusionValue::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        InclusionValue::Bool { value } => value.to_string(),
    }
}

fn emit_model_method_with_attrs(
    out: &mut String,
    module: &str,
    m: &MethodDef,
    attrs: &[Symbol],
) {
    let name = m.name.as_str();
    // Instance methods take the record as first arg (`post` for
    // module Post). Class methods take only their declared params.
    let is_instance = matches!(m.receiver, crate::dialect::MethodReceiver::Instance);
    let first_arg = if is_instance { Some(module_receiver_name(module)) } else { None };

    let mut params: Vec<String> = Vec::new();
    if let Some(arg) = &first_arg {
        params.push(arg.clone());
    }
    for p in &m.params {
        params.push(p.to_string());
    }
    let param_list = if params.is_empty() {
        String::new()
    } else {
        format!("({})", params.join(", "))
    };

    writeln!(out, "  def {name}{param_list} do").unwrap();
    // Pre-emit rewrite: bare-name Sends matching a model attribute
    // become Ivar reads, which `emit_expr` already renders as
    // `post.<field>` when receiver_arg is set. Same shape as the
    // TS/Go pre-emit passes.
    let body_expr = if first_arg.is_some() {
        rewrite_bare_attrs_to_ivars_ex(&m.body, attrs)
    } else {
        m.body.clone()
    };
    let body = emit_block(&body_expr, first_arg.as_deref());
    for line in body.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn rewrite_bare_attrs_to_ivars_ex(e: &Expr, attrs: &[Symbol]) -> Expr {
    use crate::expr::{Arm, InterpPart, Pattern};
    let rewrite = |child: &Expr| rewrite_bare_attrs_to_ivars_ex(child, attrs);
    let new_node = match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && attrs.iter().any(|s| s == method) =>
        {
            ExprNode::Ivar { name: method.clone() }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(&rewrite),
            method: method.clone(),
            args: args.iter().map(&rewrite).collect(),
            block: block.as_ref().map(&rewrite),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(&rewrite).collect(),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(&rewrite).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries.iter().map(|(k, v)| (rewrite(k), rewrite(v))).collect(),
            braced: *braced,
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite(cond),
            then_branch: rewrite(then_branch),
            else_branch: rewrite(else_branch),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: rewrite(scrutinee),
            arms: arms
                .iter()
                .map(|arm| Arm {
                    pattern: arm.pattern.clone(),
                    guard: arm.guard.as_ref().map(&rewrite),
                    body: rewrite(&arm.body),
                })
                .collect(),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite(left),
            right: rewrite(right),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: rewrite(expr) },
                })
                .collect(),
        },
        ExprNode::Let { id, name, value, body } => ExprNode::Let {
            id: *id,
            name: name.clone(),
            value: rewrite(value),
            body: rewrite(body),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite(body),
            block_style: *block_style,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: rewrite(fun),
            args: args.iter().map(&rewrite).collect(),
            block: block.as_ref().map(&rewrite),
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
                LValue::Ivar { name } => LValue::Ivar { name: name.clone() },
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: rewrite(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: rewrite(recv),
                    index: rewrite(index),
                },
            };
            ExprNode::Assign {
                target: new_target,
                value: rewrite(value),
            }
        }
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(&rewrite).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise { value: rewrite(value) },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: rewrite(expr),
            fallback: rewrite(fallback),
        },
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. } => (*e.node).clone(),
    };
    let _ = Pattern::Wildcard;
    Expr {
        span: e.span,
        node: Box::new(new_node),
        ty: e.ty.clone(),
        leading_blank_line: e.leading_blank_line,
    }
}

/// Convention: the record arg is the snake_case form of the module name.
/// `Post` → `post`, `ApplicationRecord` → `application_record`.
fn module_receiver_name(module: &str) -> String {
    snake_case(module)
}

// Controllers ----------------------------------------------------------

fn emit_controller_file(c: &Controller) -> EmittedFile {
    let module = c.name.0.as_str();
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {module} do").unwrap();
    for (i, action) in c.actions().enumerate() {
        if i > 0 {
            writeln!(s).unwrap();
        }
        emit_action(&mut s, action);
    }
    writeln!(s, "end").unwrap();
    let fname = format!("lib/{}.ex", snake_case(module));
    EmittedFile { path: PathBuf::from(fname), content: s }
}

fn emit_action(out: &mut String, a: &Action) {
    let name = a.name.as_str();
    // Every action receives `params` — matches Plug-ish convention
    // without committing to a specific runtime shape.
    writeln!(out, "  def {name}(params) do").unwrap();
    let body = emit_block(&a.body, None);
    let body_text = if body.is_empty() { ":ok".to_string() } else { body };
    for line in body_text.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

// Router ---------------------------------------------------------------

fn emit_router_file(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Router do").unwrap();
    writeln!(s, "  @routes [").unwrap();
    let mut flat: Vec<(String, String, String, String)> = Vec::new();
    for entry in &app.routes.entries {
        collect_flat_routes(entry, &mut flat, None);
    }
    for (i, (method, path, controller, action)) in flat.iter().enumerate() {
        let sep = if i + 1 == flat.len() { "" } else { "," };
        writeln!(
            s,
            "    %{{method: :{}, path: {:?}, controller: {}, action: :{}}}{sep}",
            method.to_lowercase(),
            path,
            controller,
            action,
        )
        .unwrap();
    }
    writeln!(s, "  ]").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def routes, do: @routes").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("lib/router.ex"), content: s }
}

fn collect_flat_routes(
    spec: &RouteSpec,
    out: &mut Vec<(String, String, String, String)>,
    scope_prefix: Option<(&str, &str)>,
) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, .. } => {
            let verb = match method {
                crate::dialect::HttpMethod::Get => "GET",
                crate::dialect::HttpMethod::Post => "POST",
                crate::dialect::HttpMethod::Put => "PUT",
                crate::dialect::HttpMethod::Patch => "PATCH",
                crate::dialect::HttpMethod::Delete => "DELETE",
                crate::dialect::HttpMethod::Head => "HEAD",
                crate::dialect::HttpMethod::Options => "OPTIONS",
                crate::dialect::HttpMethod::Any => "ANY",
            };
            let full_path = match scope_prefix {
                Some((parent, _)) => format!("/{parent}/:{parent}_id{path}"),
                None => path.clone(),
            };
            out.push((
                verb.to_string(),
                full_path,
                controller.0.to_string(),
                action.to_string(),
            ));
        }
        RouteSpec::Root { target } => {
            if let Some((c, a)) = target.split_once('#') {
                out.push((
                    "GET".into(),
                    "/".into(),
                    controller_class_name(c),
                    a.to_string(),
                ));
            }
        }
        RouteSpec::Resources { name, only, except, nested } => {
            let resource_path = format!("/{name}");
            let controller = controller_class_name(name.as_str());
            for (action, verb, suffix) in standard_resource_actions() {
                let action = *action;
                let verb = *verb;
                let suffix = *suffix;
                if !only.is_empty() && !only.iter().any(|s| s.as_str() == action) {
                    continue;
                }
                if except.iter().any(|s| s.as_str() == action) {
                    continue;
                }
                let path = format!("{resource_path}{suffix}");
                let full_path = match scope_prefix {
                    Some((parent, _)) => format!("/{parent}/:{parent}_id{path}"),
                    None => path,
                };
                out.push((verb.into(), full_path, controller.clone(), action.into()));
            }
            let singular =
                crate::naming::singularize_camelize(name.as_str()).to_lowercase();
            for child in nested {
                collect_flat_routes(child, out, Some((&singular, name.as_str())));
            }
        }
    }
}

fn standard_resource_actions() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("index", "GET", ""),
        ("new", "GET", "/new"),
        ("create", "POST", ""),
        ("show", "GET", "/:id"),
        ("edit", "GET", "/:id/edit"),
        ("update", "PATCH", "/:id"),
        ("destroy", "DELETE", "/:id"),
    ]
}

fn controller_class_name(short: &str) -> String {
    let mut s = crate::naming::camelize(short);
    s.push_str("Controller");
    s
}

// Bodies ---------------------------------------------------------------

/// Emit a method / action body as Elixir statements. Ruby ivar writes
/// become local rebinds (`@post = …` → `post = …`); ivar reads become
/// struct field access through the receiver arg. If `receiver_arg` is
/// `None` (e.g. a controller action), ivar reads become bare locals.
fn emit_block(body: &Expr, receiver_arg: Option<&str>) -> String {
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

fn emit_literal(lit: &Literal) -> String {
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

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {} do", tm.name.0).unwrap();
    writeln!(s, "  use ExUnit.Case").unwrap();
    // Each test starts on a fresh :memory: SQLite DB with all
    // fixtures loaded — Rails' transactional-fixture isolation
    // adapted to Elixir's per-process test semantics.
    if !app.fixtures.is_empty() {
        writeln!(s).unwrap();
        writeln!(s, "  setup do").unwrap();
        writeln!(s, "    Fixtures.setup()").unwrap();
        writeln!(s, "    :ok").unwrap();
        writeln!(s, "  end").unwrap();
    }

    for test in &tm.tests {
        writeln!(s).unwrap();
        if test_needs_runtime_unsupported_ex(test) {
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
                    "_before = {count_expr}\n{body_s}\n_after = {count_expr}\nassert _after - _before == {delta}"
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
