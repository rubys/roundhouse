//! Crystal emitter.
//!
//! Last Phase 2 scaffold. Crystal is Ruby-flavored with mandatory
//! static types, which makes this the easiest scaffold of the six —
//! the emission shape is almost identical to Ruby's, plus type
//! annotations.
//!
//! Scaffold choices:
//! - Models as `class Name` with typed `property :field : Type`
//!   declarations (Crystal's getter/setter macro).
//! - Controllers as `class Name` with one method per action. No
//!   `< Kemal::Controller` base — Phase 3+ runtime work picks.
//! - Routes as a `ROUTES` constant with a NamedTuple array —
//!   Crystal's idiomatic static table shape.
//!
//! Notably not mirrored from railcar:
//! - Railcar's Crystal output uses a heavy macro DSL
//!   (`model("articles") do column(title, String) end`) to hook
//!   into its runtime. That's a Phase-4-depth choice; our scaffold
//!   stays runtime-agnostic.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::{
    Controller, MethodDef, Model, Test, TestModule,
};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::lower::CtrlWalker as _;
use crate::ty::Ty;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/crystal/runtime.cr");
const DB_SOURCE: &str = include_str!("../../runtime/crystal/db.cr");
/// Crystal HTTP runtime — ActionResponse/ActionContext + in-memory
/// Router match table. Mirrors `runtime/rust/http.rs` +
/// `runtime/typescript/juntos.ts`; emitted controllers register
/// handlers through Router.add + tests dispatch via Router.match.
const HTTP_SOURCE: &str = include_str!("../../runtime/crystal/http.cr");
/// Crystal test-support runtime — TestClient + TestResponse with
/// Rails-shaped assertions (assert_ok, assert_redirected_to,
/// assert_select, etc). Dispatches through Router.match, no real HTTP.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/crystal/test_support.cr");
/// Crystal view helpers — link_to/button_to/form_wrap/FormBuilder.
/// Minimal HTML-returning surface covering the scaffold blog's ERB
/// uses; substring-match assertions in controller specs pass with
/// this level of fidelity.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/crystal/view_helpers.cr");
/// Crystal HTTP::Server runtime — `Roundhouse::Server.start`
/// dispatches through Router.match, wraps HTML in the emitted
/// layout, and handles `_method` override. Copied as `src/server.cr`.
const SERVER_SOURCE: &str = include_str!("../../runtime/crystal/server.cr");
/// Crystal cable stub — `/cable` handler returning 426. Copied as
/// `src/cable.cr`.
const CABLE_SOURCE: &str = include_str!("../../runtime/crystal/cable.cr");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(emit_shard_yml());
    if !app.models.is_empty() {
        files.push(emit_models(app));
        // Runtime tags along whenever any model is emitted — validate()
        // calls ValidationError.new.
        files.push(EmittedFile {
            path: PathBuf::from("src/runtime.cr"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/db.cr"),
            content: DB_SOURCE.to_string(),
        });
        files.push(emit_schema_sql(app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime — copied verbatim, same posture as `runtime.cr`
        // / `db.cr`. Provides the `Roundhouse::Http` surface that
        // emitted controller actions call into.
        files.push(EmittedFile {
            path: PathBuf::from("src/http.cr"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.cr"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.cr"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/server.cr"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/cable.cr"),
            content: CABLE_SOURCE.to_string(),
        });
        files.push(emit_controllers(app));
        files.extend(emit_views_cr(app));
        files.push(emit_route_helpers_cr(app));
        files.push(emit_cr_importmap(app));
        files.push(emit_cr_main(app));
    }
    if !app.routes.entries.is_empty() {
        files.push(emit_routes(app));
    }
    // Fixtures as modules under spec/fixtures/. Emitted as individual
    // files plus a top-level spec/fixtures.cr helper.
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(emit_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(emit_crystal_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(emit_crystal_spec(tm, app));
        }
        files.push(emit_spec_helper(app));
    }
    files.push(emit_app_cr(app));
    files
}

/// Minimal `shard.yml` — Crystal's equivalent of `Cargo.toml`. Declares
/// one named `targets` entry so `crystal build` knows the entry point;
/// the entry file (`src/app.cr`) requires whatever modules we emitted.
fn emit_shard_yml() -> EmittedFile {
    let content = "\
name: app
version: 0.1.0

targets:
  app:
    main: src/main.cr

dependencies:
  sqlite3:
    github: crystal-lang/crystal-sqlite3
    version: ~> 0.21
";
    EmittedFile {
        path: PathBuf::from("shard.yml"),
        content: content.to_string(),
    }
}

/// `src/schema_sql.cr` — Crystal constant wrapping the target-neutral
/// DDL produced by `lower::lower_schema`. `Roundhouse::Db.setup_test_db`
/// reads it to initialize a fresh :memory: SQLite connection per spec.
fn emit_schema_sql(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "module Roundhouse").unwrap();
    writeln!(s, "  module SchemaSQL").unwrap();
    writeln!(s, "    CREATE_TABLES = <<-SQL").unwrap();
    let ddl = crate::lower::lower_schema(&app.schema);
    for line in ddl.lines() {
        writeln!(s, "      {line}").unwrap();
    }
    writeln!(s, "    SQL").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("src/schema_sql.cr"),
        content: s,
    }
}

/// `src/app.cr` — Crystal entry point. Requires the emitted modules we
/// expect to compile.
fn emit_app_cr(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    if !app.models.is_empty() {
        writeln!(s, "require \"./runtime\"").unwrap();
        writeln!(s, "require \"./db\"").unwrap();
        writeln!(s, "require \"./schema_sql\"").unwrap();
        writeln!(s, "require \"./models\"").unwrap();
    }
    if !app.controllers.is_empty() {
        writeln!(s, "require \"./http\"").unwrap();
        writeln!(s, "require \"./view_helpers\"").unwrap();
        writeln!(s, "require \"./route_helpers\"").unwrap();
        writeln!(s, "require \"./importmap\"").unwrap();
        writeln!(s, "require \"./views\"").unwrap();
        writeln!(s, "require \"./controllers\"").unwrap();
        writeln!(s, "require \"./routes\"").unwrap();
        writeln!(s, "require \"./test_support\"").unwrap();
        writeln!(s, "require \"./cable\"").unwrap();
        writeln!(s, "require \"./server\"").unwrap();
        // Entry point — run the server when crystal-built binary is
        // invoked. Test specs require `src/app.cr` for type defs
    }
    EmittedFile {
        path: PathBuf::from("src/app.cr"),
        content: s,
    }
}

/// Emit `src/importmap.cr` — a `PINS` constant of `(name, path)`
/// tuples ingested from `config/importmap.rb`.
fn emit_cr_importmap(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "module Importmap").unwrap();
    writeln!(s, "  PINS = [").unwrap();
    if let Some(importmap) = &app.importmap {
        for pin in &importmap.pins {
            writeln!(s, "    {{ {:?}, {:?} }},", pin.name, pin.path).unwrap();
        }
    }
    writeln!(s, "  ] of Tuple(String, String)").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("src/importmap.cr"),
        content: s,
    }
}

/// Emit `src/main.cr` — the server entry point. `shard.yml`'s
/// `main:` points here; test specs require `./app` directly for
/// type defs without triggering the `Server.start` call.
fn emit_cr_main(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"./app\"").unwrap();
    writeln!(s).unwrap();
    // Register partial renderers for any model whose plural
    // resource has a scaffold partial (`_<singular>.html.erb`).
    // Cable's broadcast_*_to resolves the partial by model class
    // name through this registry. Only register partials whose
    // singular matches a known model class — `_form.html.erb` is
    // a view-only partial, not a model.
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
                        "Roundhouse::Cable.register_partial({class:?}, ->(id : Int64) {{",
                    )
                    .unwrap();
                    writeln!(s, "  record = {class}.find(id)").unwrap();
                    writeln!(s, "  record ? Views.{fn_name}(record) : \"\"").unwrap();
                    writeln!(s, "}})").unwrap();
                }
            }
        }
    }
    writeln!(s).unwrap();
    let has_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application");
    let layout_arg = if has_layout {
        "layout: ->{ Views.render_layouts_application(\"\") },"
    } else {
        ""
    };
    writeln!(
        s,
        "Roundhouse::Server.start(schema_sql: Roundhouse::SchemaSQL::CREATE_TABLES, {layout_arg})"
    )
    .unwrap();
    EmittedFile {
        path: PathBuf::from("src/main.cr"),
        content: s,
    }
}

// Models ---------------------------------------------------------------

fn emit_models(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    for (i, model) in app.models.iter().enumerate() {
        if i > 0 {
            writeln!(s).unwrap();
        }
        emit_model(&mut s, model, app);
    }
    EmittedFile { path: PathBuf::from("src/models.cr"), content: s }
}

fn emit_model(out: &mut String, model: &Model, app: &App) {
    let name = model.name.0.as_str();
    writeln!(out, "class {name}").unwrap();
    for (field, ty) in &model.attributes.fields {
        writeln!(
            out,
            "  property {} : {} = {}",
            field,
            crystal_ty(ty),
            crystal_default(ty),
        )
        .unwrap();
    }
    // Validation-error state — views read `article.errors.any?` +
    // `error_messages_for(article.errors, …)`. Default empty;
    // populated by a failed `save` (below).
    if !model.attributes.fields.is_empty() {
        writeln!(
            out,
            "  property errors : Array(Roundhouse::ValidationError) = [] of Roundhouse::ValidationError",
        )
        .unwrap();
    }
    for method in model.methods() {
        writeln!(out).unwrap();
        emit_model_method(out, method);
    }
    let lowered = crate::lower::lower_validations(model);
    if !lowered.is_empty() {
        writeln!(out).unwrap();
        emit_validate_method(out, &lowered);
    }
    // Skip persistence for abstract base classes (no columns beyond
    // maybe an `id`) — ApplicationRecord et al. have nothing to save.
    let has_table = model
        .attributes
        .fields
        .keys()
        .any(|k| k.as_str() != "id");
    if has_table {
        writeln!(out).unwrap();
        let broadcasts = crate::lower::lower_broadcasts(model);
        emit_persistence_methods_cr(
            out,
            model,
            !lowered.is_empty(),
            app,
            !broadcasts.is_empty(),
        );
        if !broadcasts.is_empty() {
            let lp = crate::lower::lower_persistence(model, app);
            emit_cr_broadcaster_methods(
                out,
                lp.class.0.as_str(),
                lp.table.as_str(),
                &broadcasts,
            );
        }
    }
    writeln!(out, "end").unwrap();
}

// ── Broadcaster emission ────────────────────────────────────────

fn emit_cr_broadcaster_methods(
    out: &mut String,
    class: &str,
    table: &str,
    decls: &crate::lower::LoweredBroadcasts,
) {
    writeln!(out).unwrap();
    writeln!(out, "  def _broadcast_after_save : Nil").unwrap();
    if decls.save.is_empty() {
        writeln!(out, "    nil").unwrap();
    } else {
        for b in &decls.save {
            emit_cr_one_broadcast_call(out, class, table, b);
        }
    }
    writeln!(out, "  end").unwrap();

    writeln!(out).unwrap();
    writeln!(out, "  def _broadcast_after_delete : Nil").unwrap();
    if decls.destroy.is_empty() {
        writeln!(out, "    nil").unwrap();
    } else {
        for b in &decls.destroy {
            emit_cr_one_broadcast_call(out, class, table, b);
        }
    }
    writeln!(out, "  end").unwrap();
}

fn emit_cr_one_broadcast_call(
    out: &mut String,
    class: &str,
    table: &str,
    b: &crate::lower::LoweredBroadcast,
) {
    let channel = cr_render_broadcast_expr(&b.channel, b.self_param.as_ref());
    let target = b
        .target
        .as_ref()
        .map(|t| cr_render_broadcast_expr(t, b.self_param.as_ref()))
        .unwrap_or_else(|| "\"\"".to_string());
    if let Some(assoc) = &b.on_association {
        let var = assoc.name.as_str();
        let target_class = assoc.target_class.as_str();
        let target_table = assoc.target_table.as_str();
        let fk = assoc.foreign_key.as_str();
        writeln!(out, "    {var} = {target_class}.find({fk})").unwrap();
        writeln!(out, "    if {var}").unwrap();
        if b.action == crate::lower::BroadcastAction::Remove {
            writeln!(
                out,
                "      Roundhouse::Cable.broadcast_remove_to({target_table:?}, {var}.id, {channel}, {target})",
            )
            .unwrap();
        } else {
            let func = cr_action_fn(b.action);
            writeln!(
                out,
                "      Roundhouse::Cable.{func}({target_table:?}, {var}.id, {target_class:?}, {channel}, {target})",
            )
            .unwrap();
        }
        writeln!(out, "    end").unwrap();
        return;
    }
    if b.action == crate::lower::BroadcastAction::Remove {
        writeln!(
            out,
            "    Roundhouse::Cable.broadcast_remove_to({table:?}, id, {channel}, {target})",
        )
        .unwrap();
    } else {
        let func = cr_action_fn(b.action);
        writeln!(
            out,
            "    Roundhouse::Cable.{func}({table:?}, id, {class:?}, {channel}, {target})",
        )
        .unwrap();
    }
}

fn cr_action_fn(action: crate::lower::BroadcastAction) -> &'static str {
    match action {
        crate::lower::BroadcastAction::Prepend => "broadcast_prepend_to",
        crate::lower::BroadcastAction::Append => "broadcast_append_to",
        crate::lower::BroadcastAction::Replace => "broadcast_replace_to",
        crate::lower::BroadcastAction::Remove => "broadcast_remove_to",
    }
}

fn cr_render_broadcast_expr(expr: &Expr, self_param: Option<&Symbol>) -> String {
    let p = self_param.map(|s| s.as_str());
    match &*expr.node {
        ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
        ExprNode::Lit { value: Literal::Int { value } } => format!("{value}"),
        ExprNode::Var { name, .. } => {
            if let Some(pname) = p {
                let stripped = pname.strip_prefix('_').unwrap_or(pname);
                if name.as_str() == pname || name.as_str() == stripped {
                    return "self".to_string();
                }
            }
            name.as_str().to_string()
        }
        ExprNode::Send { recv: Some(r), method, .. } => {
            let recv_s = cr_render_broadcast_expr(r, self_param);
            format!("{recv_s}.{}", method.as_str())
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                '#' => out.push_str("\\#"),
                                _ => out.push(c),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&cr_render_broadcast_expr(expr, self_param));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        _ => "nil".to_string(),
    }
}

/// Render save/destroy/count/find for a model against the test
/// connection. SQL strings come from `LoweredPersistence`; per-target
/// concerns are placeholder dialect (`?N` → `?`) and the crystal-db
/// API shape (exec/scalar/query_one? with block hydration).
fn emit_persistence_methods_cr(
    out: &mut String,
    model: &Model,
    has_validate: bool,
    app: &App,
    has_broadcasts: bool,
) {
    let lp = crate::lower::lower_persistence(model, app);
    let class = lp.class.0.as_str();

    let insert_sql = positional_placeholders(&lp.insert_sql);
    let update_sql = positional_placeholders(&lp.update_sql);
    let delete_sql = positional_placeholders(&lp.delete_sql);
    let select_by_id_sql = positional_placeholders(&lp.select_by_id_sql);
    let count_sql = lp.count_sql.clone(); // no placeholders

    let non_id_args: Vec<String> = lp
        .non_id_columns
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();

    // ----- save -----
    writeln!(out, "  def save : Bool").unwrap();
    if has_validate {
        writeln!(out, "    errs = validate").unwrap();
        writeln!(out, "    unless errs.empty?").unwrap();
        writeln!(out, "      @errors = errs").unwrap();
        writeln!(out, "      return false").unwrap();
        writeln!(out, "    end").unwrap();
        writeln!(out, "    @errors = [] of Roundhouse::ValidationError").unwrap();
    }
    for check in &lp.belongs_to_checks {
        let fk = check.foreign_key.as_str();
        let target = check.target_class.0.as_str();
        writeln!(
            out,
            "    return false if {fk} == 0_i64 || {target}.find({fk}).nil?",
        )
        .unwrap();
    }
    writeln!(out, "    db = Roundhouse::Db.conn").unwrap();
    writeln!(out, "    if id == 0_i64").unwrap();
    writeln!(
        out,
        "      result = db.exec({insert_sql:?}, {})",
        non_id_args.join(", "),
    )
    .unwrap();
    writeln!(out, "      @id = result.last_insert_id").unwrap();
    writeln!(out, "    else").unwrap();
    writeln!(
        out,
        "      db.exec({update_sql:?}, {}, id)",
        non_id_args.join(", "),
    )
    .unwrap();
    writeln!(out, "    end").unwrap();
    if has_broadcasts {
        writeln!(out, "    _broadcast_after_save").unwrap();
    }
    writeln!(out, "    true").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- destroy -----
    writeln!(out).unwrap();
    writeln!(out, "  def destroy").unwrap();
    for dc in &lp.dependent_children {
        let child_class = dc.child_class.0.as_str();
        let child_select = positional_placeholders(&dc.select_by_parent_sql);
        writeln!(out, "    dependents = [] of {child_class}").unwrap();
        writeln!(
            out,
            "    Roundhouse::Db.conn.query_all({child_select:?}, id) do |rs|"
        )
        .unwrap();
        writeln!(out, "      record = {child_class}.new").unwrap();
        for col in &dc.child_columns {
            let col_name = col.as_str();
            let ty = cr_rs_reader_type(col_name);
            writeln!(out, "      record.{col_name} = rs.read({ty})").unwrap();
        }
        writeln!(out, "      dependents << record").unwrap();
        writeln!(out, "    end").unwrap();
        writeln!(out, "    dependents.each(&.destroy)").unwrap();
    }
    writeln!(
        out,
        "    Roundhouse::Db.conn.exec({delete_sql:?}, id)",
    )
    .unwrap();
    if has_broadcasts {
        writeln!(out, "    _broadcast_after_delete").unwrap();
    }
    writeln!(out, "  end").unwrap();

    // ----- count -----
    writeln!(out).unwrap();
    writeln!(out, "  def self.count : Int64").unwrap();
    writeln!(
        out,
        "    Roundhouse::Db.conn.scalar({count_sql:?}).as(Int64)",
    )
    .unwrap();
    writeln!(out, "  end").unwrap();

    // ----- find -----
    writeln!(out).unwrap();
    writeln!(out, "  def self.find(id : Int64) : {class}?").unwrap();
    writeln!(
        out,
        "    Roundhouse::Db.conn.query_one?({select_by_id_sql:?}, id) do |rs|",
    )
    .unwrap();
    writeln!(out, "      record = {class}.new").unwrap();
    for col in &lp.columns {
        let col_name = col.as_str();
        let ty = cr_rs_reader_type_for(model, col_name);
        writeln!(out, "      record.{col_name} = rs.read({ty})").unwrap();
    }
    writeln!(out, "      record").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- all -----
    writeln!(out).unwrap();
    let select_all_sql = &lp.select_all_sql;
    writeln!(out, "  def self.all : Array({class})").unwrap();
    writeln!(out, "    records = [] of {class}").unwrap();
    writeln!(
        out,
        "    Roundhouse::Db.conn.query_all({select_all_sql:?}) do |rs|"
    )
    .unwrap();
    writeln!(out, "      record = {class}.new").unwrap();
    for col in &lp.columns {
        let col_name = col.as_str();
        let ty = cr_rs_reader_type_for(model, col_name);
        writeln!(out, "      record.{col_name} = rs.read({ty})").unwrap();
    }
    writeln!(out, "      records << record").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "    records").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- last -----
    writeln!(out).unwrap();
    let last_sql = format!("{select_all_sql} ORDER BY id DESC LIMIT 1");
    writeln!(out, "  def self.last : {class}?").unwrap();
    writeln!(
        out,
        "    Roundhouse::Db.conn.query_one?({last_sql:?}) do |rs|"
    )
    .unwrap();
    writeln!(out, "      record = {class}.new").unwrap();
    for col in &lp.columns {
        let col_name = col.as_str();
        let ty = cr_rs_reader_type_for(model, col_name);
        writeln!(out, "      record.{col_name} = rs.read({ty})").unwrap();
    }
    writeln!(out, "      record").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- reload -----
    writeln!(out).unwrap();
    writeln!(out, "  def reload : self").unwrap();
    writeln!(out, "    fresh = {class}.find(id)").unwrap();
    writeln!(out, "    return self if fresh.nil?").unwrap();
    for col in &lp.columns {
        let col_name = col.as_str();
        writeln!(out, "    @{col_name} = fresh.{col_name}").unwrap();
    }
    writeln!(out, "    self").unwrap();
    writeln!(out, "  end").unwrap();
}

/// SQLite numbered placeholders (`?1`, `?2`) → crystal-db positional
/// (`?`). Same table, same semantics, different wire syntax — a
/// driver-level quirk we absorb at emit time. Keyed on `?` + digit
/// run; non-placeholder `?`s pass through.
fn positional_placeholders(sql: &str) -> String {
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

/// Crystal reader type for a struct field during cascade hydration.
/// Uses the model's declared type when available; defaults to
/// Int64/String by column-name heuristic otherwise (only happens when
/// a LoweredPersistence DependentChild carries columns whose owning
/// model isn't on hand in this callsite).
fn cr_rs_reader_type(col_name: &str) -> &'static str {
    match col_name {
        "id" => "Int64",
        _ if col_name.ends_with("_id") => "Int64",
        "created_at" | "updated_at" => "Time",
        _ => "String",
    }
}

fn cr_rs_reader_type_for(model: &Model, col_name: &str) -> String {
    if let Some(ty) = model
        .attributes
        .fields
        .iter()
        .find(|(k, _)| k.as_str() == col_name)
        .map(|(_, v)| v)
    {
        match ty {
            Ty::Int => "Int64".to_string(),
            Ty::Float => "Float64".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::Str | Ty::Sym => "String".to_string(),
            Ty::Class { id, .. } if id.0.as_str() == "Time" => "Time".to_string(),
            _ => "String".to_string(),
        }
    } else {
        cr_rs_reader_type(col_name).to_string()
    }
}

fn emit_validate_method(
    out: &mut String,
    validations: &[crate::lower::LoweredValidation],
) {
    writeln!(out, "  def validate : Array(Roundhouse::ValidationError)").unwrap();
    writeln!(out, "    errors = [] of Roundhouse::ValidationError").unwrap();
    for lv in validations {
        for check in &lv.checks {
            emit_check_inline_cr(out, lv.attribute.as_str(), check);
        }
    }
    writeln!(out, "    errors").unwrap();
    writeln!(out, "  end").unwrap();
}

fn emit_check_inline_cr(out: &mut String, attr: &str, check: &crate::lower::Check) {
    use crate::lower::Check;
    let msg = check.default_message();
    let push = |cond: &str| -> String {
        format!(
            "    if {cond}\n      errors << Roundhouse::ValidationError.new({attr:?}, {msg:?})\n    end",
        )
    };
    let block = match check {
        Check::Presence => push(&format!("{attr}.empty?")),
        Check::Absence => push(&format!("!{attr}.empty?")),
        Check::MinLength { n } => push(&format!("{attr}.size < {n}")),
        Check::MaxLength { n } => push(&format!("{attr}.size > {n}")),
        Check::GreaterThan { threshold } => push(&format!("{attr} <= {threshold}")),
        Check::LessThan { threshold } => push(&format!("{attr} >= {threshold}")),
        Check::OnlyInteger => {
            format!("    # OnlyInteger on {attr:?} — enforced by Crystal's type system")
        }
        Check::Inclusion { values } => {
            let parts: Vec<String> = values.iter().map(inclusion_value_to_cr).collect();
            push(&format!("![{}].includes?({attr})", parts.join(", ")))
        }
        Check::Format { pattern } => {
            format!(
                "    # TODO: Format check on {attr:?} requires runtime regex ({pattern:?})"
            )
        }
        Check::Uniqueness { .. } => format!(
            "    # TODO: Uniqueness on {attr:?} requires DB access at runtime"
        ),
        Check::Custom { method } => format!("    {method}(errors)"),
    };
    writeln!(out, "{block}").unwrap();
}

fn inclusion_value_to_cr(v: &crate::lower::InclusionValue) -> String {
    use crate::lower::InclusionValue;
    match v {
        InclusionValue::Str { value } => format!("{value:?}"),
        InclusionValue::Int { value } => format!("{value}_i64"),
        InclusionValue::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { format!("{s}_f64") } else { format!("{s}.0_f64") }
        }
        InclusionValue::Bool { value } => value.to_string(),
    }
}

/// A Crystal literal expression for the given type's zero value.
/// Crystal's `property` declarations must be initialized — unlike
/// Rust's `#[derive(Default)]` we have to write the value inline.
/// Keep aligned with `crystal_ty`: whatever a field renders as, its
/// default must be a valid expression of that type.
fn crystal_default(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0_i64".to_string(),
        Ty::Float => "0.0_f64".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Nil => "nil".to_string(),
        Ty::Array { .. } => "[] of typeof({})".to_string(),
        Ty::Hash { .. } => "{} of String => String".to_string(),
        // Class types we emit ourselves get a .new; stdlib Time gets
        // Time.utc; anything else falls back to .new and trusts the
        // class defines a zero-arg initializer.
        Ty::Class { id, .. } => match id.0.as_str() {
            "Time" => "Time.utc".to_string(),
            other => format!("{other}.new"),
        },
        _ => "nil".to_string(),
    }
}

fn emit_model_method(out: &mut String, m: &MethodDef) {
    let name = m.name.as_str();
    let ret = m.body.ty.clone().unwrap_or(Ty::Nil);
    let ret_annot = format!(" : {}", crystal_ty(&ret));
    let receiver = match m.receiver {
        crate::dialect::MethodReceiver::Instance => "",
        crate::dialect::MethodReceiver::Class => "self.",
    };
    let params = if m.params.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = m.params.iter().map(|p| p.to_string()).collect();
        format!("({})", ps.join(", "))
    };
    writeln!(out, "  def {receiver}{name}{params}{ret_annot}").unwrap();
    let body = emit_body(&m.body);
    for line in body.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

// Controllers ----------------------------------------------------------
//
// Phase 4c: actions and private helpers render through the controller-
// scope Send rewrites (`emit_controller_send_cr`), matching the Rust
// emitter's shape. Every action returns `Roundhouse::Http::Response`;
// bodies emit their natural content as statements, with `Response.new`
// tacked on as the tail (Rails' convention: ivars feed the view, not
// the action's return value).
//
// Rewrites handled for controller-body Sends:
//   * bare `params`          → `Roundhouse::Http.params`
//   * `params.expect(...)`   → passes through; `Params#expect` is a
//                              stub that accepts any shape
//   * `respond_to { ... }`   → `Roundhouse::Http.respond_to do |__fr|
//                              ... end`
//   * `format.html { body }` → `__fr.html { body }`
//   * `format.json { body }` → replaced by a `# TODO: JSON branch`
//                              comment at the call site
//   * bare `redirect_to` / `render` / `head` → `Roundhouse::Http.*`
//   * bare `*_path` / `*_url` → `""` (placeholder string — Crystal
//                              lacks Rust's `!` divergence trick, so
//                              we emit a typed default)
//   * `Model.new(anything)`  → `Model.new` (emitted models have no
//                              keyword/positional arg constructor)
//   * `Model.find(x)`        → `Model.find(x).not_nil!` (the class
//                              method returns `Model?`; the declared
//                              ivar type is non-nil)
//   * unsupported query chains (`.all`, `.order`, `.includes`, …) →
//                              `[] of Target`

fn emit_controllers(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"./http\"").unwrap();
    if !app.models.is_empty() {
        writeln!(s, "require \"./models\"").unwrap();
    }
    writeln!(s, "require \"./route_helpers\"").unwrap();
    writeln!(s, "require \"./views\"").unwrap();
    for controller in &app.controllers {
        writeln!(s).unwrap();
        emit_controller_pass2(&mut s, controller, app);
    }
    EmittedFile { path: PathBuf::from("src/controllers.cr"), content: s }
}

fn emit_controller_pass2(out: &mut String, c: &Controller, app: &App) {
    let name = c.name.0.as_str();
    let actions_mod = format!("{name}Actions");
    writeln!(out, "module {actions_mod}").unwrap();

    // Crystal traditionally computed a plural `resource` and singular
    // separately; with the shared helpers, we take the singular from
    // `resource_from_controller_name` and derive plural for downstream
    // uses (emit helpers still key off plural).
    let singular = crate::lower::resource_from_controller_name(name);
    let resource = crate::naming::pluralize_snake(&crate::naming::camelize(&singular));
    let model_class = crate::naming::camelize(&singular);
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let has_model = known_models.iter().any(|m| m.as_str() == model_class);
    let parent = crate::lower::find_nested_parent(app, name);
    let permitted = crate::lower::permitted_fields_for(c, &singular)
        .unwrap_or_else(|| crate::lower::default_permitted_fields(app, &model_class));

    let _ = resource; // Plural-resource path only appears in destroy's index redirect.
    for action in c.actions() {
        let name = action.name.as_str();
        if !matches!(
            name,
            "index" | "show" | "new" | "edit" | "create" | "update" | "destroy"
        ) {
            continue;
        }
        writeln!(out).unwrap();
        let la = crate::lower::lower_action(
            name,
            &singular,
            &model_class,
            has_model,
            parent.as_ref(),
            &permitted,
        );
        emit_cr_action(out, &la, &action.body, &known_models, c);
    }

    writeln!(out, "end").unwrap();
}

/// Render one LoweredAction as a Crystal `def self.*`. Crystal's
/// `new` action renames to `new_action` (avoids collision with
/// Crystal's type-level `.new`). Body emission delegates to the
/// walker; the ActionKind dispatch is gone (TS/Rust/Python precedent).
fn emit_cr_action(
    out: &mut String,
    la: &crate::lower::LoweredAction,
    body: &Expr,
    known_models: &[Symbol],
    controller: &Controller,
) {
    let response_ty = "Roundhouse::Http::ActionResponse";
    let ctx_ty = "Roundhouse::Http::ActionContext";
    let action_method_name = match la.name.as_str() {
        "new" => "new_action",
        other => other,
    };

    let mut body_src = String::new();
    let mut uses_context = false;
    if la.has_model {
        use crate::lower::CtrlWalker;
        let normalized =
            crate::lower::normalize_action_body(controller, la.name.as_str(), body);
        let mut emitter = CrEmitter {
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
        body_src = emitter.walk_action_body(&normalized);
        uses_context = emitter.state.uses_context;
    } else {
        writeln!(body_src, "  {response_ty}.new").unwrap();
    }

    let ctx_param = if uses_context || body_src.contains("context") {
        "context"
    } else {
        "_context"
    };
    writeln!(
        out,
        "  def self.{action_method_name}({ctx_param} : {ctx_ty}) : {response_ty}"
    )
    .unwrap();
    out.push_str(&body_src);
    writeln!(out, "  end").unwrap();
}

struct CrEmitter<'a> {
    ctx: crate::lower::WalkCtx<'a>,
    state: crate::lower::WalkState,
}

impl<'a> crate::lower::CtrlWalker<'a> for CrEmitter<'a> {
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
        writeln!(out, "{indent}{var_name} = {class}.new").unwrap();
        if let Some(parent) = self.ctx.parent {
            writeln!(
                out,
                "{indent}{var_name}.{0}_id = context.params.fetch(\"{0}_id\", \"0\").to_i64",
                parent.singular,
            )
            .unwrap();
            self.state.uses_context = true;
        }
        let permitted: Vec<String> = self.ctx.permitted.iter().cloned().collect();
        let resource = self.ctx.resource.to_string();
        for field in &permitted {
            writeln!(
                out,
                "{indent}{var_name}.{field} = context.params.fetch(\"{resource}[{field}]\", \"\")",
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
        let cond_s = self.render_expr(cond);
        writeln!(out, "{indent}if {cond_s}").unwrap();
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
            writeln!(out, "{indent}if v = context.params[\"{resource}[{field}]\"]?").unwrap();
            writeln!(out, "{indent}  {recv_s}.{field} = v").unwrap();
            writeln!(out, "{indent}end").unwrap();
            self.state.uses_context = true;
        }
        writeln!(out, "{indent}if {recv_s}.save").unwrap();
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
                    let recv_s = self.render_expr(r);
                    if args.is_empty() {
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
        emit_expr(expr)
    }

    fn render_send_stmt(
        &mut self,
        recv: Option<&Expr>,
        method: &str,
        args: &[Expr],
        block: Option<&Expr>,
        // Crystal concurrency lives in fibers; no async syntax,
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
                let s = match &*key.node {
                    ExprNode::Lit { value: Literal::Sym { value: k } } => {
                        format!("context.params[\"{}\"].to_i64", k.as_str())
                    }
                    _ => {
                        let k = self.render_expr(key);
                        format!("context.params[{k}]")
                    }
                };
                Stmt::Expr(s)
            }
            SendKind::ParamsExpect { args: pe_args } => {
                self.state.uses_context = true;
                let s = match pe_args.first().map(|e| &*e.node) {
                    Some(ExprNode::Lit { value: Literal::Sym { value: k } }) => {
                        format!("context.params[\"{}\"].to_i64", k.as_str())
                    }
                    _ => "context.params # TODO: params.expect hash".to_string(),
                };
                Stmt::Expr(s)
            }
            SendKind::ModelNew { class } => Stmt::Expr(format!("{}.new", class.as_str())),
            SendKind::ModelFind { class, id } => {
                let id_s = self.render_expr(id);
                Stmt::Expr(format!("{0}.find({id_s}) || {0}.new", class.as_str()))
            }
            SendKind::QueryChain { target: Some(target), method: cm, args: ca, recv: cr } => {
                let mut out = format!("{}.all", target.as_str());
                let mods = crate::lower::collect_chain_modifiers(cm, ca, cr.as_deref());
                for m in mods {
                    out = apply_cr_chain_modifier(out, m);
                }
                Stmt::Expr(out)
            }
            SendKind::QueryChain { target: None, .. } => Stmt::Expr("[] of String".to_string()),
            SendKind::AssocLookup { target, outer_method } => match outer_method {
                "find" => {
                    let id_s = args.first().map(|a| self.render_expr(a))
                        .unwrap_or_else(|| "0_i64".to_string());
                    Stmt::Expr(format!("{0}.find({id_s}) || {0}.new", target.as_str()))
                }
                _ => Stmt::Expr(format!("{}.new", target.as_str())),
            },
            SendKind::BangStrip { recv, stripped_method, args: bs_args } => {
                let recv_s = self.render_expr(recv);
                if bs_args.is_empty() {
                    Stmt::Expr(format!("{recv_s}.{stripped_method}"))
                } else {
                    let args_s: Vec<String> =
                        bs_args.iter().map(|a| self.render_expr(a)).collect();
                    Stmt::Expr(format!("{recv_s}.{stripped_method}({})", args_s.join(", ")))
                }
            }
            SendKind::InstanceUpdate => Stmt::Expr("false".to_string()),
            SendKind::PathOrUrlHelper => {
                let helper = method.strip_suffix("_path").or_else(|| method.strip_suffix("_url"))
                    .unwrap_or(method);
                Stmt::Expr(format!("RouteHelpers.{helper}_path"))
            }
            SendKind::Render { args: r_args } => Stmt::Response(self.render_cr_render(r_args)),
            SendKind::RedirectTo { args: r_args } => Stmt::Response(self.render_cr_redirect(r_args)),
            SendKind::Head { args: h_args } => {
                let status = h_args.first().and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Sym { value: s } } =>
                        Some(crate::lower::status_sym_to_code(s.as_str())),
                    ExprNode::Lit { value: Literal::Int { value: n } } => Some(*n as u16),
                    _ => None,
                }).unwrap_or(200);
                Stmt::Response(format!(
                    "Roundhouse::Http::ActionResponse.new(status: {status})"
                ))
            }
            SendKind::RespondToBlock { .. }
            | SendKind::FormatHtml { .. }
            | SendKind::FormatJson => Stmt::Expr(
                "Roundhouse::Http::ActionResponse.new # unreachable: respond_to not normalized".to_string(),
            ),
        })
    }
}

impl<'a> CrEmitter<'a> {
    fn render_cr_render(&mut self, args: &[Expr]) -> String {
        let response_ty = "Roundhouse::Http::ActionResponse";
        if let Some(first) = args.first() {
            if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*first.node {
                let view_fn = cr_view_fn(self.ctx.model_class, sym.as_str());
                let arg = self.state.last_local.clone().unwrap_or_else(|| "nil".to_string());
                let body_part = format!("body: Views.{view_fn}({arg})");
                return match crate::lower::extract_status_from_kwargs(&args[1..]) {
                    Some(status) => format!("{response_ty}.new(status: {status}, {body_part})"),
                    None => format!("{response_ty}.new({body_part})"),
                };
            }
            let body_s = self.render_expr(first);
            return format!("{response_ty}.new(body: {body_s})");
        }
        format!("{response_ty}.new")
    }

    fn render_cr_redirect(&mut self, args: &[Expr]) -> String {
        let response_ty = "Roundhouse::Http::ActionResponse";
        let Some(first) = args.first() else {
            return format!("{response_ty}.new(status: 303)");
        };
        let loc = self.render_expr(first);
        let status = crate::lower::extract_status_from_kwargs(&args[1..]).unwrap_or(303);
        if is_bare_cr_ident(&loc) {
            let id_access = format!("{loc}.id");
            return format!(
                "{response_ty}.new(status: {status}, location: RouteHelpers.{loc}_path({id_access}))",
            );
        }
        format!("{response_ty}.new(status: {status}, location: {loc})")
    }
}

fn is_bare_cr_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return false; }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first == b'_') { return false; }
    bytes.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}


/// Map a model class name + view action (`"Article" + "Show"`) to
/// the Crystal view fn name (`render_articles_show`). Still used by
/// the controller template-per-action emit to reference views.
fn cr_view_fn(model_class: &str, suffix: &str) -> String {
    let plural = crate::naming::pluralize_snake(model_class);
    format!("render_{plural}_{}", suffix.to_lowercase())
}

/// Emit `src/views.cr` — a single Views module holding one
/// `def self.render_<plural>_<action>` per ingested view plus
/// stubs for standard CRUD views the fixture didn't supply.
/// Walks the ERB IR (`_buf = _buf + X`) produced by `src/erb.rs`,
/// same shape the TS/Rust degrade-gracefully consumers use;
/// unknown shapes become `io << "" # TODO ERB …` so the module
/// still compiles under Crystal's strict typing.
fn emit_views_cr(app: &App) -> Vec<EmittedFile> {
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let attrs_by_class: std::collections::BTreeMap<String, Vec<String>> = app
        .models
        .iter()
        .map(|m| {
            (
                m.name.0.as_str().to_string(),
                m.attributes
                    .fields
                    .keys()
                    .map(|k| k.as_str().to_string())
                    .collect(),
            )
        })
        .collect();
    let has_manys = crate::lower::build_has_many_table(app);
    let stylesheets = app.stylesheets.clone();

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"./view_helpers\"").unwrap();
    writeln!(s, "require \"./route_helpers\"").unwrap();
    writeln!(s, "require \"./importmap\"").unwrap();
    if !app.models.is_empty() {
        writeln!(s, "require \"./models\"").unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "module Views").unwrap();

    let mut emitted: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for view in &app.views {
        emit_cr_view_fn(&mut s, view, &known_models, &attrs_by_class, &has_manys, &stylesheets);
        emitted.insert(cr_view_function_name(view.name.as_str()));
    }

    emit_cr_missing_view_stubs(&mut s, app, &known_models, &emitted);

    writeln!(s, "end").unwrap();
    vec![EmittedFile {
        path: PathBuf::from("src/views.cr"),
        content: s,
    }]
}

fn emit_cr_view_fn(
    out: &mut String,
    view: &crate::dialect::View,
    known_models: &[Symbol],
    attrs_by_class: &std::collections::BTreeMap<String, Vec<String>>,
    has_manys: &[crate::lower::HasManyRow],
    stylesheets: &[String],
) {
    let rewritten = rewrite_view_body_ivars_cr(&view.body);
    let ivar_names = collect_ivar_names_cr(&view.body);
    let fn_name = cr_view_function_name(view.name.as_str());

    let (sig, arg_name, arg_model) = cr_view_signature(view.name.as_str(), known_models);
    let attrs = arg_model
        .as_ref()
        .and_then(|c| attrs_by_class.get(c).cloned())
        .unwrap_or_default();
    let resource_dir = view
        .name
        .as_str()
        .rsplit_once('/')
        .map(|(d, _): (&str, &str)| d.to_string())
        .unwrap_or_default();

    let mut locals: Vec<String> = vec![arg_name.clone()];
    for n in &ivar_names {
        if !locals.iter().any(|x| x == n) {
            locals.push(n.clone());
        }
    }
    let ctx = CrViewCtx {
        locals,
        arg_name: arg_name.clone(),
        arg_attrs: attrs,
        resource_dir,
        has_manys: has_manys.to_vec(),
        stylesheets: stylesheets.to_vec(),
        form_records: Vec::new(),
        attrs_by_class: attrs_by_class.clone(),
    };

    // Apply the shared erubi-trim pass so `<% %>` statement tags
    // drop their trailing newline + leading indent the same way
    // Rails does. Same call site as rust/ts/python/go.
    let trimmed = crate::lower::erb_trim::trim_view(&rewritten);

    writeln!(out).unwrap();
    writeln!(out, "  def self.{fn_name}({sig}) : String").unwrap();
    writeln!(out, "    String.build do |io|").unwrap();
    for line in emit_cr_view_body_walker(&trimmed, &ctx) {
        writeln!(out, "      {line}").unwrap();
    }
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();
}

/// Emit stub `def self.render_<plural>_<action>` for every standard
/// CRUD view the fixture didn't supply. Keeps controllers
/// referencing `Views.render_*` resolvable regardless of which .erb
/// files the user provides.
fn emit_cr_missing_view_stubs(
    out: &mut String,
    app: &App,
    _known_models: &[Symbol],
    emitted: &std::collections::BTreeSet<String>,
) {
    for model in &app.models {
        if model.attributes.fields.is_empty() {
            continue;
        }
        let class = model.name.0.as_str();
        let plural = crate::naming::pluralize_snake(class);
        for action_stem in ["index", "show", "new", "edit"] {
            let view_name = format!("{plural}/{action_stem}");
            let fn_name = cr_view_function_name(&view_name);
            if emitted.contains(&fn_name) {
                continue;
            }
            let (arg_ty, arg_name) = if action_stem == "index" {
                (format!("Array({class})"), plural.clone())
            } else {
                (class.to_string(), crate::naming::singularize(&plural))
            };
            writeln!(out).unwrap();
            writeln!(
                out,
                "  def self.{fn_name}({arg_name} : {arg_ty}) : String"
            )
            .unwrap();
            writeln!(out, "    _ = {arg_name}").unwrap();
            writeln!(out, "    \"\"").unwrap();
            writeln!(out, "  end").unwrap();
        }
    }
}

/// Map a view path to its Crystal function name —
/// `articles/index` → `render_articles_index`; partial paths like
/// `articles/_article` map to `render_articles_article` (prefix
/// underscore dropped).
fn cr_view_function_name(name: &str) -> String {
    let mut out = String::from("render_");
    let mut first = true;
    for seg in name.split('/') {
        let trimmed = seg.strip_prefix('_').unwrap_or(seg);
        if !first {
            out.push('_');
        }
        first = false;
        out.push_str(trimmed);
    }
    out
}

/// Build the single-arg Crystal signature for a view function.
/// Returns `(signature, arg_name, model_class)` where `model_class`
/// is `Some(class)` when the resource name resolves to a known
/// model (used for `ctx.arg_has_attr` checks).
fn cr_view_signature(
    view_name: &str,
    known_models: &[Symbol],
) -> (String, String, Option<String>) {
    let (dir, base) = view_name.rsplit_once('/').unwrap_or(("", view_name));
    let is_partial = base.starts_with('_');
    let stem = base.trim_start_matches('_');
    let model_class = crate::naming::singularize_camelize(dir);
    let model_exists = known_models.iter().any(|m| m.as_str() == model_class);
    let singular = crate::naming::singularize(dir);

    if is_partial {
        let arg_name = if model_exists { singular.clone() } else { stem.to_string() };
        if model_exists {
            return (format!("{arg_name} : {model_class}"), arg_name, Some(model_class));
        }
        return (format!("{arg_name} : String"), arg_name, None);
    }
    match stem {
        "index" => {
            let arg_name = dir.to_string();
            if model_exists {
                return (
                    format!("{arg_name} : Array({model_class})"),
                    arg_name,
                    Some(model_class),
                );
            }
            (format!("{arg_name} : Array(String)"), arg_name, None)
        }
        _ => {
            let arg_name = singular.clone();
            if model_exists {
                return (format!("{arg_name} : {model_class}"), arg_name, Some(model_class));
            }
            (format!("{arg_name} : String"), arg_name, None)
        }
    }
}

/// Rewrite `@ivar` references inside a view body to bare locals,
/// matching the arg name the signature declared. Mirrors the
/// controller-side rewrite; views don't reference `params[:key]`
/// so we skip that intercept.
fn rewrite_view_body_ivars_cr(expr: &Expr) -> Expr {
    use crate::ident::VarId;
    let new_node: ExprNode = match &*expr.node {
        ExprNode::Ivar { name } => ExprNode::Var { id: VarId(0), name: name.clone() },
        ExprNode::Assign { target: LValue::Ivar { name }, value } => ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value: rewrite_view_body_ivars_cr(value),
        },
        ExprNode::Assign { target: LValue::Var { id, name }, value } => ExprNode::Assign {
            target: LValue::Var { id: *id, name: name.clone() },
            value: rewrite_view_body_ivars_cr(value),
        },
        ExprNode::Assign { target: LValue::Attr { recv, name }, value } => ExprNode::Assign {
            target: LValue::Attr {
                recv: rewrite_view_body_ivars_cr(recv),
                name: name.clone(),
            },
            value: rewrite_view_body_ivars_cr(value),
        },
        ExprNode::Assign { target: LValue::Index { recv, index }, value } => ExprNode::Assign {
            target: LValue::Index {
                recv: rewrite_view_body_ivars_cr(recv),
                index: rewrite_view_body_ivars_cr(index),
            },
            value: rewrite_view_body_ivars_cr(value),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_view_body_ivars_cr),
            method: method.clone(),
            args: args.iter().map(rewrite_view_body_ivars_cr).collect(),
            block: block.as_ref().map(rewrite_view_body_ivars_cr),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_view_body_ivars_cr).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_view_body_ivars_cr(cond),
            then_branch: rewrite_view_body_ivars_cr(then_branch),
            else_branch: rewrite_view_body_ivars_cr(else_branch),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_view_body_ivars_cr(left),
            right: rewrite_view_body_ivars_cr(right),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(rewrite_view_body_ivars_cr).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_view_body_ivars_cr(k), rewrite_view_body_ivars_cr(v)))
                .collect(),
            braced: *braced,
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_view_body_ivars_cr(body),
            block_style: *block_style,
        },
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
    }
}

fn collect_ivar_names_cr(expr: &Expr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_ivars_into_cr(expr, &mut out);
    out
}

fn collect_ivars_into_cr(expr: &Expr, out: &mut Vec<String>) {
    match &*expr.node {
        ExprNode::Ivar { name } => {
            let n = name.to_string();
            if !out.iter().any(|e| e == &n) {
                out.push(n);
            }
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Ivar { name } = target {
                let n = name.to_string();
                if !out.iter().any(|e| e == &n) {
                    out.push(n);
                }
            }
            collect_ivars_into_cr(value, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivars_into_cr(r, out);
            }
            for a in args {
                collect_ivars_into_cr(a, out);
            }
            if let Some(b) = block {
                collect_ivars_into_cr(b, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_ivars_into_cr(e, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivars_into_cr(cond, out);
            collect_ivars_into_cr(then_branch, out);
            collect_ivars_into_cr(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_ivars_into_cr(left, out);
            collect_ivars_into_cr(right, out);
        }
        ExprNode::Array { elements, .. } => {
            for e in elements {
                collect_ivars_into_cr(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivars_into_cr(k, out);
                collect_ivars_into_cr(v, out);
            }
        }
        ExprNode::Lambda { body, .. } => collect_ivars_into_cr(body, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    collect_ivars_into_cr(expr, out);
                }
            }
        }
        _ => {}
    }
}

#[derive(Clone)]
struct CrViewCtx {
    locals: Vec<String>,
    arg_name: String,
    arg_attrs: Vec<String>,
    resource_dir: String,
    has_manys: Vec<crate::lower::HasManyRow>,
    stylesheets: Vec<String>,
    /// Active FormBuilder bindings: `(builder_local_name,
    /// record_crystal_expr)` pairs. Populated on `form_with` block
    /// entry; consumed by FormBuilder field emit so `form.text_field
    /// :title` resolves to the bound record's attr.
    form_records: Vec<(String, String)>,
    attrs_by_class: std::collections::BTreeMap<String, Vec<String>>,
}

impl CrViewCtx {
    fn is_local(&self, n: &str) -> bool {
        self.locals.iter().any(|x| x == n)
    }
    fn arg_has_attr(&self, name: &str, attr: &str) -> bool {
        name == self.arg_name && self.arg_attrs.iter().any(|a| a == attr)
    }
    fn local_has_attr(&self, local: &str, attr: &str) -> bool {
        if self.arg_has_attr(local, attr) {
            return true;
        }
        let class = crate::naming::singularize_camelize(local);
        self.attrs_by_class
            .get(&class)
            .map(|attrs| attrs.iter().any(|a| a == attr))
            .unwrap_or(false)
    }
    fn with_locals(&self, more: impl IntoIterator<Item = String>) -> Self {
        let mut next = self.clone();
        for n in more {
            if !next.locals.iter().any(|x| x == &n) {
                next.locals.push(n);
            }
        }
        next
    }
    fn resolve_has_many_on_local(&self, local: &str, assoc: &str) -> Option<(String, String)> {
        if !self.is_local(local) {
            return None;
        }
        crate::lower::resolve_has_many_on_local(&self.has_manys, local, assoc)
    }
}

fn emit_cr_view_body_walker(body: &Expr, ctx: &CrViewCtx) -> Vec<String> {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    for stmt in &stmts {
        out.extend(emit_cr_view_stmt(stmt, ctx));
    }
    out
}

fn emit_cr_view_stmt(stmt: &Expr, ctx: &CrViewCtx) -> Vec<String> {
    match &*stmt.node {
        // Prologue `_buf = ""` — String.build already owns the buffer.
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == "_buf" =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            return vec![emit_cr_view_append(&args[0], ctx)];
                        }
                    }
                }
            }
            vec!["io << \"\" # TODO ERB: _buf shape".to_string()]
        }
        // Epilogue: bare `_buf` — dropped; String.build returns
        // the buffer implicitly.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        // `if cond ... else ... end`.
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_cr = if is_cr_simple_expr(cond, ctx) {
                emit_cr_view_expr_raw(cond, ctx)
            } else {
                "false # TODO ERB cond".to_string()
            };
            let mut out = vec![format!("if {cond_cr}")];
            for line in emit_cr_view_body_walker(then_branch, ctx) {
                out.push(format!("  {line}"));
            }
            let has_else = !matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if has_else {
                out.push("else".to_string());
                for line in emit_cr_view_body_walker(else_branch, ctx) {
                    out.push(format!("  {line}"));
                }
            }
            out.push("end".to_string());
            out
        }
        // `coll.each do |x| ... end` — only if coll is a simple
        // local; Crystal can't iterate an unresolved association.
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if method.as_str() == "each" && args.is_empty() =>
        {
            if !is_cr_simple_expr(recv, ctx) {
                return vec!["io << \"\" # TODO ERB: each over complex coll".to_string()];
            }
            let ExprNode::Lambda { params, body, .. } = &*block.node else {
                return vec!["io << \"\" # TODO ERB: each block shape".to_string()];
            };
            let coll_s = emit_cr_view_expr_raw(recv, ctx);
            let var = params
                .first()
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "item".into());
            let inner_ctx = ctx.with_locals([var.clone()]);
            let mut out = vec![format!("{coll_s}.each do |{var}|")];
            for line in emit_cr_view_body_walker(body, &inner_ctx) {
                out.push(format!("  {line}"));
            }
            out.push("end".to_string());
            out
        }
        // Statement-form `<% content_for :title, "Articles" %>`.
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            if let Some(crate::lower::ViewHelperKind::ContentForSetter { slot, body }) =
                crate::lower::classify_view_helper(method.as_str(), args)
            {
                if is_cr_simple_expr(body, ctx) {
                    let body_cr = emit_cr_view_expr_raw(body, ctx);
                    return vec![format!(
                        "Roundhouse::ViewHelpers.content_for_set({slot:?}, ({body_cr}).to_s)"
                    )];
                }
            }
            vec!["io << \"\" # TODO ERB: unknown stmt".to_string()]
        }
        _ => vec!["io << \"\" # TODO ERB: unknown stmt".to_string()],
    }
}

fn emit_cr_view_append(arg: &Expr, ctx: &CrViewCtx) -> String {
    if let ExprNode::Lit { value: Literal::Str { value: s } } = &*arg.node {
        return format!("io << {}", cr_string_literal(s));
    }
    let inner = unwrap_to_s_cr(arg);

    // `<%= yield %>` / `<%= yield :head %>`.
    if let ExprNode::Yield { args } = &*inner.node {
        if let Some(first) = args.first() {
            if let Some(slot) = cr_extract_slot_name(first) {
                return format!("io << Roundhouse::ViewHelpers.get_slot({slot:?})");
            }
        }
        return "io << Roundhouse::ViewHelpers.get_yield".to_string();
    }

    // `<%= content_for(:slot) || "default" %>` — BoolOp fallback.
    if let ExprNode::BoolOp { op, left, right, .. } = &*inner.node {
        if matches!(op, crate::expr::BoolOpKind::Or) {
            if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*left.node {
                if method.as_str() == "content_for" && args.len() == 1 {
                    if let Some(slot) = cr_extract_slot_name(&args[0]) {
                        if is_cr_simple_expr(right, ctx) {
                            let fb = emit_cr_view_expr_raw(right, ctx);
                            return format!(
                                "io << (Roundhouse::ViewHelpers.content_for_get({slot:?}).presence || ({fb}).to_s)"
                            );
                        }
                    }
                }
            }
        }
    }

    // `render @coll` / `render "partial", hash`.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if method.as_str() == "render" {
            if args.len() == 1 {
                return emit_cr_render_call(&args[0], ctx);
            }
            if args.len() == 2 {
                if let (
                    ExprNode::Lit { value: Literal::Str { value: partial } },
                    ExprNode::Hash { entries, .. },
                ) = (&*args[0].node, &*args[1].node)
                {
                    let partial_fn = format!("render_{}_{}", ctx.resource_dir, partial);
                    if let Some((_, v)) = entries.first() {
                        if is_cr_simple_expr(v, ctx) {
                            let arg_expr = emit_cr_view_expr_raw(v, ctx);
                            return format!("io << Views.{partial_fn}({arg_expr})");
                        }
                    }
                    return "io << \"\" # TODO ERB: render partial".to_string();
                }
            }
        }
    }

    // Capturing helpers.
    if let ExprNode::Send {
        recv: None,
        method,
        args,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if is_cr_capturing_helper(method.as_str()) {
            return emit_cr_captured_helper(method.as_str(), args, block, ctx);
        }
    }

    // View-helper classifier dispatch.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if let Some(kind) = crate::lower::classify_view_helper(method.as_str(), args) {
            if let Some(line) = emit_cr_view_helper(&kind, ctx) {
                return line;
            }
        }
    }

    // FormBuilder method calls (`form.label :title`, etc.).
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*inner.node {
        if let ExprNode::Var { name, .. } | ExprNode::Ivar { name } = &*r.node {
            if ctx.form_records.iter().any(|(n, _)| n == name.as_str()) {
                if let Some(fb) = crate::lower::classify_form_builder_method(method.as_str()) {
                    if let Some(call) = emit_cr_form_builder_call(name.as_str(), fb, args, ctx)
                    {
                        return format!("io << {call}");
                    }
                }
            }
        }
    }

    if is_cr_simple_expr(inner, ctx) {
        let raw = emit_cr_view_expr_raw(inner, ctx);
        if matches!(&*inner.node, ExprNode::Lit { value: Literal::Str { .. } }) {
            return format!("io << {raw}");
        }
        return format!("io << ({raw}).to_s");
    }

    "io << \"\" # TODO ERB: complex interpolation".to_string()
}

fn cr_extract_slot_name(arg: &Expr) -> Option<&str> {
    match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.as_str()),
        _ => None,
    }
}

fn is_cr_capturing_helper(method: &str) -> bool {
    matches!(method, "form_with" | "content_for")
}

fn emit_cr_captured_helper(
    method: &str,
    args: &[Expr],
    block: &Expr,
    ctx: &CrViewCtx,
) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return format!("io << \"\" # TODO ERB: {method}");
    };
    let cls_expr = args
        .iter()
        .find_map(|a| cr_extract_kwarg(a, "class"))
        .filter(|e| is_cr_simple_expr(e, ctx))
        .map(|e| emit_cr_view_expr_raw(e, ctx))
        .unwrap_or_else(|| "\"\"".to_string());
    match method {
        "form_with" => {
            let model_expr = args.iter().find_map(|a| cr_extract_kwarg(a, "model"));
            let model_nested: Option<Vec<Expr>> = model_expr.and_then(|e| match &*e.node {
                ExprNode::Array { elements, .. } if elements.len() >= 2 => {
                    Some(elements.clone())
                }
                _ => None,
            });
            let record_ref: Option<String> = if let Some(elems) = &model_nested {
                Some(emit_cr_nested_form_record(&elems[elems.len() - 1]))
            } else {
                model_expr
                    .filter(|e| is_cr_simple_expr(e, ctx))
                    .map(|e| emit_cr_view_expr_raw(e, ctx))
                    .or_else(|| {
                        if !ctx.arg_name.is_empty() && ctx.is_local(&ctx.arg_name) {
                            Some(ctx.arg_name.clone())
                        } else {
                            None
                        }
                    })
            };
            let prefix = if let Some(elems) = &model_nested {
                cr_nested_form_child_prefix(&elems[elems.len() - 1])
                    .unwrap_or_else(|| ctx.arg_name.clone())
            } else if !ctx.resource_dir.is_empty() {
                crate::naming::singularize(&ctx.resource_dir)
            } else {
                ctx.arg_name.clone()
            };

            let pname = params.first().map(|p| p.as_str()).unwrap_or("form");
            let plural = if !ctx.resource_dir.is_empty() {
                ctx.resource_dir.clone()
            } else {
                crate::naming::pluralize_snake(&prefix)
            };
            // Crystal's route helpers are snake_case fns like
            // rails (ArticlePath → article_path) — use naming.
            let prefix_snake = crate::naming::snake_case(&prefix);
            let plural_snake = crate::naming::snake_case(&plural);

            let is_persisted_expr = record_ref
                .as_deref()
                .map(|r| format!("({r}.id != 0)"))
                .unwrap_or_else(|| "false".to_string());
            let action_expr = match (&record_ref, &model_nested) {
                (Some(record), Some(elems)) => {
                    cr_nested_form_path_expr(elems, ctx, record, &prefix)
                }
                (Some(record), None) => format!(
                    "({record}.id != 0 ? RouteHelpers.{prefix_snake}_path({record}.id) : RouteHelpers.{plural_snake}_path)",
                ),
                (None, _) => format!("RouteHelpers.{plural_snake}_path"),
            };

            let mut inner_ctx = ctx.with_locals([pname.to_string()]);
            if let Some(record) = &record_ref {
                inner_ctx
                    .form_records
                    .push((pname.to_string(), record.clone()));
            }
            let mut lines: Vec<String> = Vec::new();
            lines.push("io << (begin".to_string());
            lines.push("  __inner = String.build do |io|".to_string());
            lines.push(format!(
                "    {pname} = Roundhouse::ViewHelpers::FormBuilder.new({prefix:?}, {cls_expr}, {is_persisted_expr})"
            ));
            if let Some(record) = &record_ref {
                lines.push(format!(
                    "    io << Roundhouse::ViewHelpers.error_messages_for({record}.errors, {prefix:?})"
                ));
            }
            for line in emit_cr_view_body_walker(body, &inner_ctx) {
                lines.push(format!("    {line}"));
            }
            lines.push("  end".to_string());
            lines.push(format!(
                "  Roundhouse::ViewHelpers.form_wrap({action_expr}, {is_persisted_expr}, {cls_expr}, __inner)"
            ));
            lines.push("end)".to_string());
            lines.join("\n")
        }
        "content_for" => {
            let slot = args.first().and_then(cr_extract_slot_name);
            let Some(slot) = slot else {
                let _ = cls_expr;
                return "io << \"\"".to_string();
            };
            let mut lines: Vec<String> = Vec::new();
            lines.push("(begin".to_string());
            lines.push("  __cf = String.build do |io|".to_string());
            for line in emit_cr_view_body_walker(body, ctx) {
                lines.push(format!("    {line}"));
            }
            lines.push("  end".to_string());
            lines.push(format!(
                "  Roundhouse::ViewHelpers.content_for_set({slot:?}, __cf)"
            ));
            lines.push("end)".to_string());
            lines.join("\n")
        }
        _ => "io << \"\"".to_string(),
    }
}

fn emit_cr_nested_form_record(el: &Expr) -> String {
    match crate::lower::classify_nested_form_child(el) {
        Some(crate::lower::NestedFormChild::ClassNew { class }) => format!("{class}.new"),
        Some(crate::lower::NestedFormChild::Local { name }) => name.to_string(),
        None => "nil".to_string(),
    }
}

fn cr_nested_form_child_prefix(el: &Expr) -> Option<String> {
    crate::lower::classify_nested_form_child(el).map(|k| k.prefix())
}

fn cr_nested_element_parts(kind: &crate::lower::NestedUrlElement<'_>) -> (String, String) {
    match kind {
        crate::lower::NestedUrlElement::DirectLocal { name } => {
            ((*name).to_string(), format!("{name}.id"))
        }
        crate::lower::NestedUrlElement::Association { owner, assoc } => {
            ((*assoc).to_string(), format!("{owner}.{assoc}_id"))
        }
    }
}

fn cr_nested_form_path_expr(
    elems: &[Expr],
    ctx: &CrViewCtx,
    record_ref: &str,
    child_prefix: &str,
) -> String {
    let is_local = |n: &str| ctx.is_local(n);
    let mut parent_ids: Vec<String> = Vec::new();
    let mut parent_singulars: Vec<String> = Vec::new();
    for parent in &elems[..elems.len() - 1] {
        let Some(kind) = crate::lower::classify_nested_url_element(parent, &is_local) else {
            return "\"\"".to_string();
        };
        let (singular, id_expr) = cr_nested_element_parts(&kind);
        parent_singulars.push(singular);
        parent_ids.push(id_expr);
    }
    let member_name = format!(
        "{}_{}_path",
        parent_singulars.join("_"),
        child_prefix,
    );
    let child_plural = crate::naming::pluralize_snake(child_prefix);
    let collection_name = format!(
        "{}_{}_path",
        parent_singulars.join("_"),
        child_plural,
    );
    let parent_args = parent_ids.join(", ");
    let member_args = if parent_args.is_empty() {
        format!("{record_ref}.id")
    } else {
        format!("{parent_args}, {record_ref}.id")
    };
    let collection_call = if parent_args.is_empty() {
        format!("RouteHelpers.{collection_name}")
    } else {
        format!("RouteHelpers.{collection_name}({parent_args})")
    };
    format!(
        "({record_ref}.id != 0 ? RouteHelpers.{member_name}({member_args}) : {collection_call})",
    )
}

/// Render a classified `ViewHelperKind` to a Crystal `io << …`
/// statement. Returns None when arg shapes aren't renderable.
/// Compose one AR chain modifier onto a running Crystal slice
/// expression. Mirrors rust/ts/python/go: `all`/`includes`/etc.
/// pass through; `order({field: :dir})` wraps with `sort_by`;
/// `limit(N)` slices `[0, N]`.
fn apply_cr_chain_modifier(prev: String, m: crate::lower::ChainModifier<'_>) -> String {
    match m.method {
        "all" | "includes" | "preload" | "joins" | "distinct" | "select" => prev,
        "order" => {
            let Some(hash) = m.args.first() else { return prev };
            let ExprNode::Hash { entries, .. } = &*hash.node else { return prev };
            let Some((k, v)) = entries.first() else { return prev };
            let field = match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => return prev,
            };
            let dir = match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => "asc".to_string(),
            };
            if dir == "desc" {
                format!("{prev}.sort_by(&.{field}).reverse")
            } else {
                format!("{prev}.sort_by(&.{field})")
            }
        }
        "limit" => {
            let Some(n) = m.args.first() else { return prev };
            if let ExprNode::Lit { value: Literal::Int { value, .. } } = &*n.node {
                return format!("{prev}[0, {value}]");
            }
            prev
        }
        _ => prev,
    }
}

fn emit_cr_view_helper(
    kind: &crate::lower::ViewHelperKind<'_>,
    ctx: &CrViewCtx,
) -> Option<String> {
    use crate::lower::ViewHelperKind::*;
    match kind {
        CsrfMetaTags => Some("io << Roundhouse::ViewHelpers.csrf_meta_tags".to_string()),
        CspMetaTag => Some("io << Roundhouse::ViewHelpers.csp_meta_tag".to_string()),
        JavascriptImportmapTags => Some(
            "io << Roundhouse::ViewHelpers.javascript_importmap_tags(Importmap::PINS, \"application\")"
                .to_string(),
        ),
        TurboStreamFrom { channel } => {
            if !is_cr_simple_expr(channel, ctx) {
                return None;
            }
            let arg = emit_cr_view_expr_raw(channel, ctx);
            Some(format!("io << Roundhouse::ViewHelpers.turbo_stream_from({arg})"))
        }
        DomId { record, prefix } => {
            let (singular, id_expr) = cr_resolve_dom_id_record(record, ctx)?;
            match prefix {
                None => Some(format!(
                    "io << Roundhouse::ViewHelpers.dom_id({singular:?}, {id_expr})"
                )),
                Some(p) => {
                    let prefix_str = match &*p.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            format!("{:?}", value.as_str())
                        }
                        ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
                        _ if is_cr_simple_expr(p, ctx) => emit_cr_view_expr_raw(p, ctx),
                        _ => return None,
                    };
                    Some(format!(
                        "io << Roundhouse::ViewHelpers.dom_id({singular:?}, {id_expr}, {prefix_str})"
                    ))
                }
            }
        }
        Pluralize { count, word } => {
            if !is_cr_simple_expr(count, ctx) || !is_cr_simple_expr(word, ctx) {
                return None;
            }
            let c = emit_cr_view_expr_raw(count, ctx);
            let w = emit_cr_view_expr_raw(word, ctx);
            Some(format!("io << Roundhouse::ViewHelpers.pluralize({c}, {w})"))
        }
        Truncate { text, opts } => {
            if !is_cr_simple_expr(text, ctx) {
                return None;
            }
            let t = emit_cr_view_expr_raw(text, ctx);
            let opts_code = cr_opts_from_expr(opts.as_deref(), ctx);
            Some(format!("io << Roundhouse::ViewHelpers.truncate({t}, {opts_code})"))
        }
        StylesheetLinkTag { name, opts } => {
            let opts_code = cr_opts_from_expr(opts.as_deref(), ctx);
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*name.node {
                if value.as_str() == "app" && !ctx.stylesheets.is_empty() {
                    let lines: Vec<String> = ctx
                        .stylesheets
                        .iter()
                        .map(|n| {
                            format!("io << Roundhouse::ViewHelpers.stylesheet_link_tag({n:?}, {opts_code})")
                        })
                        .collect();
                    return Some(lines.join("\nio << \"\\n\"\n"));
                }
            }
            let name_expr = match &*name.node {
                ExprNode::Lit { value: Literal::Sym { value } } => format!("{:?}", value.as_str()),
                ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
                _ => return None,
            };
            Some(format!(
                "io << Roundhouse::ViewHelpers.stylesheet_link_tag({name_expr}, {opts_code})"
            ))
        }
        ContentForGetter { slot } => {
            Some(format!("io << Roundhouse::ViewHelpers.content_for_get({slot:?})"))
        }
        ContentForSetter { .. } => None,
        LinkTo { text, url, opts } => {
            emit_cr_link_or_button("link_to", text, url, *opts, ctx)
                .map(|call| format!("io << {call}"))
        }
        ButtonTo { text, target, opts } => {
            emit_cr_link_or_button("button_to", text, target, *opts, ctx)
                .map(|call| format!("io << {call}"))
        }
    }
}

fn cr_resolve_dom_id_record(record: &Expr, ctx: &CrViewCtx) -> Option<(String, String)> {
    let name = match &*record.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if ctx.is_local(name.as_str()) => {
            name.as_str().to_string()
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => method.as_str().to_string(),
        _ => return None,
    };
    let singular = crate::naming::singularize(&name);
    Some((singular, format!("{name}.id")))
}

fn emit_cr_link_or_button(
    helper: &str,
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &CrViewCtx,
) -> Option<String> {
    if !is_cr_simple_expr(text, ctx) {
        return None;
    }
    let text_raw = emit_cr_view_expr_raw(text, ctx);
    let text_expr = match &*text.node {
        ExprNode::Lit { value: Literal::Str { .. } } => text_raw,
        _ => format!("({text_raw}).to_s"),
    };
    let is_local = |n: &str| ctx.is_local(n);
    let url_kind = crate::lower::classify_view_url_arg(url, &is_local)?;
    let url_expr = match url_kind {
        crate::lower::ViewUrlArg::Literal { value } => format!("{value:?}"),
        crate::lower::ViewUrlArg::PathHelper { name, args } => {
            let args_s: Vec<String> = args.iter().map(|a| cr_path_arg(a, ctx)).collect();
            if args_s.is_empty() {
                format!("RouteHelpers.{name}")
            } else {
                format!("RouteHelpers.{name}({})", args_s.join(", "))
            }
        }
        crate::lower::ViewUrlArg::RecordRef { name } => {
            format!(
                "RouteHelpers.{}_path({name}.id)",
                crate::naming::singularize(name),
            )
        }
        crate::lower::ViewUrlArg::NestedArray { elements } => cr_emit_nested_path(elements, ctx)?,
    };
    let opts_code = cr_opts_from_expr(opts, ctx);
    Some(format!(
        "Roundhouse::ViewHelpers.{helper}({text_expr}, {url_expr}, {opts_code})"
    ))
}

fn cr_path_arg(arg: &Expr, ctx: &CrViewCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if ctx.is_local(name.as_str()) => {
            format!("{}.id", name.as_str())
        }
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && ctx.is_local(method.as_str()) =>
        {
            format!("{}.id", method.as_str())
        }
        _ => emit_cr_view_expr_raw(arg, ctx),
    }
}

fn cr_emit_nested_path(elements: &[Expr], ctx: &CrViewCtx) -> Option<String> {
    let is_local = |n: &str| ctx.is_local(n);
    let mut singulars: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for el in elements {
        let kind = crate::lower::classify_nested_url_element(el, &is_local)?;
        let (singular, id_expr) = cr_nested_element_parts(&kind);
        singulars.push(singular);
        args.push(id_expr);
    }
    let name = format!("{}_path", singulars.join("_"));
    Some(format!("RouteHelpers.{name}({})", args.join(", ")))
}

fn cr_opts_from_expr(opts: Option<&Expr>, ctx: &CrViewCtx) -> String {
    match opts.map(|e| &*e.node) {
        Some(ExprNode::Hash { entries, .. }) => cr_hash_to_map(entries, ctx),
        _ => "{} of String => String".to_string(),
    }
}

fn cr_hash_to_map(entries: &[(Expr, Expr)], ctx: &CrViewCtx) -> String {
    let mut items: Vec<(String, String)> = Vec::new();
    for (k, v) in entries {
        let key = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => continue,
        };
        if key == "data" {
            if let ExprNode::Hash { entries: de, .. } = &*v.node {
                for (dk, dv) in de {
                    let dk_str = match &*dk.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            value.as_str().replace('_', "-")
                        }
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            value.replace('_', "-")
                        }
                        _ => continue,
                    };
                    let dv_s = cr_opt_value(dv, ctx);
                    items.push((format!("data-{dk_str}"), dv_s));
                }
                continue;
            }
        }
        let val = if key == "class" {
            cr_class_value(v, ctx)
        } else {
            cr_opt_value(v, ctx)
        };
        items.push((key, val));
    }
    if items.is_empty() {
        return "{} of String => String".to_string();
    }
    let parts: Vec<String> = items
        .into_iter()
        .map(|(k, v)| format!("{k:?} => {v}"))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn cr_opt_value(v: &Expr, ctx: &CrViewCtx) -> String {
    match &*v.node {
        ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
        ExprNode::Lit { value: Literal::Int { value, .. } } => {
            format!("\"{value}\"")
        }
        ExprNode::Lit { value: Literal::Sym { value } } => format!("{:?}", value.as_str()),
        ExprNode::Array { elements, .. } => match elements.first() {
            Some(first) => match &*first.node {
                ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
                _ => "\"\"".to_string(),
            },
            None => "\"\"".to_string(),
        },
        _ if is_cr_simple_expr(v, ctx) => {
            let raw = emit_cr_view_expr_raw(v, ctx);
            match &*v.node {
                ExprNode::Lit { .. } => raw,
                _ => format!("({raw}).to_s"),
            }
        }
        _ => "\"\"".to_string(),
    }
}

fn cr_class_value(v: &Expr, ctx: &CrViewCtx) -> String {
    let is_local = |n: &str| ctx.is_local(n);
    let simple_as_cr = |e: &Expr| -> Option<String> {
        if is_cr_simple_expr(e, ctx) {
            Some(match &*e.node {
                ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
                _ => {
                    let raw = emit_cr_view_expr_raw(e, ctx);
                    match &*e.node {
                        ExprNode::Lit { .. } => raw,
                        _ => format!("({raw}).to_s"),
                    }
                }
            })
        } else {
            None
        }
    };
    match crate::lower::classify_class_value(v, &is_local) {
        crate::lower::ClassValueShape::Simple { expr } => {
            simple_as_cr(expr).unwrap_or_else(|| "\"\"".to_string())
        }
        crate::lower::ClassValueShape::Conditional { base, clauses } => {
            let Some(base_cr) = simple_as_cr(base) else {
                return "\"\"".to_string();
            };
            if clauses.is_empty() {
                return base_cr;
            }
            let extras: Vec<String> = clauses
                .iter()
                .map(|(cls_text, pred)| {
                    let cond = cr_render_errors_field_predicate(pred);
                    format!(" + ({cond} ? \" {cls_text}\" : \"\")")
                })
                .collect();
            format!("({base_cr}{})", extras.join(""))
        }
        crate::lower::ClassValueShape::Unknown => "\"\"".to_string(),
    }
}

fn cr_render_errors_field_predicate(pred: &crate::lower::ErrorsFieldPredicate<'_>) -> String {
    let call = format!(
        "Roundhouse::ViewHelpers.field_has_error({}.errors, {:?})",
        pred.record, pred.field,
    );
    if pred.expect_present {
        call
    } else {
        format!("!{call}")
    }
}

fn emit_cr_form_builder_call(
    recv: &str,
    kind: crate::lower::FormBuilderMethod,
    args: &[Expr],
    ctx: &CrViewCtx,
) -> Option<String> {
    use crate::lower::FormBuilderMethod::*;
    let (field, opts_entries) = crate::lower::classify_form_builder_args(args);
    let field = field.map(str::to_string);
    let opts = opts_entries
        .map(|entries| cr_hash_to_map(entries, ctx))
        .unwrap_or_else(|| "{} of String => String".to_string());
    match kind {
        Label => {
            let field = field?;
            Some(format!("{recv}.label({field:?}, {opts})"))
        }
        TextField | TextArea => {
            let cr_method = if matches!(kind, TextField) { "text_field" } else { "textarea" };
            let field = field?;
            let value_expr = ctx
                .form_records
                .iter()
                .find(|(name, _)| name == recv)
                .and_then(|(_, record)| {
                    if ctx.local_has_attr(record, &field) {
                        Some(format!("{record}.{field}"))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "\"\"".to_string());
            Some(format!(
                "{recv}.{cr_method}({field:?}, {value_expr}, {opts})"
            ))
        }
        Submit => {
            let label_str = args.iter().find_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                _ => None,
            });
            let opts_expr = if let Some(lbl) = label_str {
                format!(
                    "({opts}).merge({{\"label\" => {lbl:?}}})",
                )
            } else {
                opts
            };
            Some(format!("{recv}.submit({opts_expr})"))
        }
    }
}


fn emit_cr_render_call(arg: &Expr, ctx: &CrViewCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name }
            if ctx.is_local(name.as_str()) =>
        {
            let plural = name.as_str();
            let singular = crate::naming::singularize(plural);
            let partial_fn = format!("render_{plural}_{singular}");
            let coll = name.to_string();
            format!("{coll}.each {{ |__r| io << Views.{partial_fn}(__r) }}")
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if args.is_empty()
                && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) =>
        {
            // `render @article.comments` — has_many association.
            // Resolve via the flattened `has_manys` table and inline
            // a filter query, matching the expr_raw path.
            let owner_name = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.as_str().to_string(),
                _ => return "io << \"\" # TODO ERB: render".to_string(),
            };
            if let Some((target_class, fk)) =
                ctx.resolve_has_many_on_local(&owner_name, method.as_str())
            {
                let assoc_plural = method.as_str();
                let singular = crate::naming::singularize(assoc_plural);
                let partial_fn = format!("render_{assoc_plural}_{singular}");
                return format!(
                    "{target_class}.all.select {{ |__r| __r.{fk} == {owner_name}.id }}.each {{ |__r| io << Views.{partial_fn}(__r) }}"
                );
            }
            "io << \"\" # TODO ERB: render assoc (unresolved)".to_string()
        }
        _ => "io << \"\" # TODO ERB: render".to_string(),
    }
}

fn cr_extract_kwarg<'a>(arg: &'a Expr, key: &str) -> Option<&'a Expr> {
    if let ExprNode::Hash { entries, .. } = &*arg.node {
        for (k, v) in entries {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                if value.as_str() == key {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn unwrap_to_s_cr(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

/// Escape a string for a Crystal double-quoted literal. Rust's
/// debug format (`{:?}`) handles the common cases (quote, backslash,
/// newline) the same way Crystal does; `#` also needs escaping to
/// avoid accidental `#{...}` interpolation.
fn cr_string_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '#' => out.push_str("\\#"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn is_cr_simple_expr(expr: &Expr, ctx: &CrViewCtx) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => ctx.is_local(name.as_str()),
        // Partial-scope local parsed as bare Send — same as rust/
        // python/go: treat as a simple local-read.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && ctx.is_local(method.as_str()) =>
        {
            true
        }
        ExprNode::Send { recv: Some(r), method, args, block, .. } => {
            if !args.is_empty() || block.is_some() {
                return false;
            }
            let clean = method.as_str().trim_end_matches('?').trim_end_matches('!');
            if clean.is_empty() {
                return false;
            }
            let recv_local = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.as_str()),
                ExprNode::Send {
                    recv: None,
                    method: m,
                    args: ra,
                    block: None,
                    ..
                } if ra.is_empty() && ctx.is_local(m.as_str()) => Some(m.as_str()),
                _ => None,
            };
            if let Some(local_name) = recv_local {
                if ctx.local_has_attr(local_name, clean) {
                    return true;
                }
                if ctx.is_local(local_name)
                    && matches!(method.as_str(), "any?" | "none?" | "present?" | "empty?")
                {
                    return true;
                }
                if ctx
                    .resolve_has_many_on_local(local_name, method.as_str())
                    .is_some()
                {
                    return true;
                }
            }
            if is_cr_simple_expr(r, ctx) {
                return true;
            }
            false
        }
        ExprNode::StringInterp { parts } => parts.iter().all(|p| match p {
            crate::expr::InterpPart::Text { .. } => true,
            crate::expr::InterpPart::Expr { expr } => is_cr_simple_expr(expr, ctx),
        }),
        _ => false,
    }
}

fn emit_cr_view_expr_raw(expr: &Expr, ctx: &CrViewCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Send { recv, method, args, .. } => {
            let method_s = method.as_str();
            // Bare local ref (partial scope).
            if recv.is_none() && args.is_empty() && ctx.is_local(method_s) {
                return method_s.to_string();
            }
            if let Some(r) = recv {
                if args.is_empty() {
                    // has_many association read → inline filter.
                    // Crystal is statically typed; the target class
                    // has a `Class.all` and each record has
                    // `.field_name_id` fields, so we build an array
                    // via select.
                    let owner_local = match &*r.node {
                        ExprNode::Var { name, .. } | ExprNode::Ivar { name }
                            if ctx.is_local(name.as_str()) =>
                        {
                            Some(name.as_str().to_string())
                        }
                        ExprNode::Send {
                            recv: None,
                            method: m,
                            args: ra,
                            block: None,
                            ..
                        } if ra.is_empty() && ctx.is_local(m.as_str()) => {
                            Some(m.as_str().to_string())
                        }
                        _ => None,
                    };
                    if let Some(owner) = &owner_local {
                        if let Some((target_class, fk)) =
                            ctx.resolve_has_many_on_local(owner, method_s)
                        {
                            return format!(
                                "{target_class}.all.select {{ |__r| __r.{fk} == {owner}.id }}"
                            );
                        }
                    }
                    // Collection predicates on any recv.
                    match method_s {
                        "any?" | "present?" => {
                            let recv_s = emit_cr_view_expr_raw(r, ctx);
                            return format!("!{recv_s}.empty?");
                        }
                        "none?" | "empty?" => {
                            let recv_s = emit_cr_view_expr_raw(r, ctx);
                            return format!("{recv_s}.empty?");
                        }
                        "size" | "count" | "length" => {
                            let recv_s = emit_cr_view_expr_raw(r, ctx);
                            return format!("{recv_s}.size");
                        }
                        "full_message" => {
                            let recv_s = emit_cr_view_expr_raw(r, ctx);
                            return format!("{recv_s}.full_message");
                        }
                        _ => {}
                    }
                }
                let recv_s = emit_cr_view_expr_raw(r, ctx);
                let clean = method_s.trim_end_matches('?').trim_end_matches('!');
                if args.is_empty() {
                    return format!("{recv_s}.{clean}");
                }
                let args_s: Vec<String> =
                    args.iter().map(|a| emit_cr_view_expr_raw(a, ctx)).collect();
                return format!("{recv_s}.{clean}({})", args_s.join(", "));
            }
            // Bare fn call with no recv → assume helper.
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_view_expr_raw(a, ctx)).collect();
            if args.is_empty() {
                format!("Roundhouse::ViewHelpers.{method_s}")
            } else {
                format!("Roundhouse::ViewHelpers.{method_s}({})", args_s.join(", "))
            }
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '"' || c == '\\' {
                                out.push('\\');
                            }
                            if c == '#' {
                                out.push('\\');
                            }
                            out.push(c);
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_cr_view_expr_raw(expr, ctx));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        _ => "\"\"".to_string(),
    }
}

// --- Crystal controller-test emit (uses shared classifier) ----------

fn emit_cr_controller_test(out: &mut String, test: &Test, app: &App) {
    writeln!(out, "  it {:?} do", test.name.as_str()).unwrap();
    writeln!(out, "    client = Roundhouse::TestSupport::TestClient.new").unwrap();

    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(&crate::naming::camelize(ivar.as_str()));
        let fixture_mod = crate::naming::camelize(&plural);
        writeln!(
            out,
            "    {} = Fixtures::{}.one",
            ivar.as_str(),
            fixture_mod,
        )
        .unwrap();
    }

    let stmts = crate::lower::test_body_stmts(&test.body);
    for stmt in stmts {
        let rendered = emit_cr_ctrl_test_stmt(stmt, app);
        for line in rendered.lines() {
            writeln!(out, "    {line}").unwrap();
        }
    }

    writeln!(out, "  end").unwrap();
}

fn emit_cr_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_cr_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_cr_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.reload");
            }
            let recv_s = emit_cr_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{name} = {}", emit_cr_ctrl_test_expr(value, app))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{name} = {}", emit_cr_ctrl_test_expr(value, app))
        }
        _ => emit_cr_ctrl_test_expr(stmt, app),
    }
}

fn emit_cr_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_cr_url_expr(url, app);
            format!("resp = client.get({u})")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_cr_url_expr(url, app);
            let body = params
                .map(|h| flatten_cr_params_to_form(h, None, app))
                .unwrap_or_else(|| "{} of String => String".to_string());
            format!("resp = client.{method}({u}, {body})")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_cr_url_expr(url, app);
            format!("resp = client.delete({u})")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assert_ok".to_string(),
            "unprocessable_entity" => "resp.assert_unprocessable".to_string(),
            other => format!("resp.assert_status(200) # TODO {other:?}"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_cr_url_expr(url, app);
            format!("resp.assert_redirected_to({u})")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_cr_assert_select(selector, kind, app)
        }
        Some(ControllerTestSend::AssertDifference { method, count_expr, delta, block }) => {
            let _ = method;
            emit_cr_assert_difference(count_expr, delta, block, app)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_cr_ctrl_test_expr(expected, app);
            let a = emit_cr_ctrl_test_expr(actual, app);
            format!("({a}).should eq({e})")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_cr_url_expr(expr: &Expr, app: &App) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_cr_ctrl_test_expr(expr, app);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last.not_nil!.id", class.as_str()),
            UrlArg::Raw(e) => emit_cr_ctrl_test_expr(e, app),
        })
        .collect();
    format!("RouteHelpers.{helper_name}({})", args_s.join(", "))
}

fn emit_cr_assert_select(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } = &*selector_expr.node
    else {
        return format!(
            "# TODO: dynamic selector\nresp.assert_select({})",
            emit_cr_ctrl_test_expr(selector_expr, app),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_cr_ctrl_test_expr(expr, app);
            format!("resp.assert_select_text({selector:?}, {text})")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_cr_ctrl_test_expr(expr, app);
            format!("resp.assert_select_min({selector:?}, {n})")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("resp.assert_select({selector:?})\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in crate::lower::test_body_stmts(inner_body) {
                out.push_str(&emit_cr_ctrl_test_stmt(stmt, app));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assert_select({selector:?})")
        }
    }
}

fn emit_cr_assert_difference(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    // "Article.count" → `Article.count` (already valid Crystal).
    let count_expr = count_expr_str.clone();

    let mut out = String::new();
    out.push_str(&format!("_before = {count_expr}\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in crate::lower::test_body_stmts(inner_body) {
            out.push_str(&emit_cr_ctrl_test_stmt(stmt, app));
            out.push('\n');
        }
    }
    out.push_str(&format!("_after = {count_expr}\n"));
    out.push_str(&format!("(_after - _before).should eq({expected_delta})"));
    out
}

fn emit_cr_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => path
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last.not_nil!");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_cr_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_cr_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_cr_url_expr(expr, app);
            }
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("# TODO expr {:?}", std::mem::discriminant(&*expr.node)),
    }
}

fn flatten_cr_params_to_form(expr: &Expr, scope: Option<&str>, app: &App) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_cr_ctrl_test_expr(value, app);
            format!("{key:?} => {val}.to_s")
        })
        .collect();
    if pairs.is_empty() {
        return "{} of String => String".to_string();
    }
    format!("{{ {} }} of String => String", pairs.join(", "))
}



// Routes ---------------------------------------------------------------

fn emit_routes(app: &App) -> EmittedFile {
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"./http\"").unwrap();
    writeln!(s, "require \"./controllers\"").unwrap();
    writeln!(s).unwrap();
    for r in &flat {
        let action_str = r.action.as_str();
        let handler_method = if action_str == "new" { "new_action" } else { action_str };
        writeln!(
            s,
            "Roundhouse::Http::Router.add({:?}, {:?}, ->(ctx : Roundhouse::Http::ActionContext) {{ {}Actions.{}(ctx) }})",
            http_verb_cr(&r.method),
            r.path,
            r.controller.0,
            handler_method,
        )
        .unwrap();
    }
    EmittedFile { path: PathBuf::from("src/routes.cr"), content: s }
}

// Bodies + expressions -------------------------------------------------

fn emit_body(body: &Expr) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt(e));
            }
            lines.join("\n")
        }
        _ => emit_stmt(body),
    }
}

fn emit_stmt(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("@{} = {}", name, emit_expr(value))
        }
        _ => emit_expr(e),
    }
}

fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("@{name}"),
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // Crystal's if/else/end is identical to Ruby's.
            let cond_s = emit_expr(cond);
            let then_s = emit_body(then_branch);
            let else_s = emit_body(else_branch);
            format!(
                "if {cond_s}\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            )
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::{BoolOpKind, BoolOpSurface};
            // Crystal supports both `&&` / `||` and `and` / `or` — we
            // preserve the surface form from the IR the same way the
            // Ruby emitter does.
            let op_s = match (op, &e.node) {
                (BoolOpKind::Or, _) => {
                    if let ExprNode::BoolOp { surface: BoolOpSurface::Word, .. } = &*e.node {
                        "or"
                    } else {
                        "||"
                    }
                }
                (BoolOpKind::And, _) => {
                    if let ExprNode::BoolOp { surface: BoolOpSurface::Word, .. } = &*e.node {
                        "and"
                    } else {
                        "&&"
                    }
                }
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_expr(v))
                    } else {
                        format!("{} => {}", emit_expr(k), emit_expr(v))
                    }
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Crystal interpolation is identical to Ruby's.
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
                                '#' => out.push_str("\\#"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ExprNode::Yield { args } => {
            let parts: Vec<String> = args.iter().map(emit_expr).collect();
            if parts.is_empty() {
                "yield".to_string()
            } else {
                format!("yield {}", parts.join(", "))
            }
        }
        other => format!("# TODO: emit {:?}", std::mem::discriminant(other)),
    }
}

fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
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
            let recv_s = emit_expr(r);
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
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
        // Crystal has first-class symbols just like Ruby.
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

// Types ----------------------------------------------------------------

pub fn crystal_ty(ty: &Ty) -> String {
    match ty {
        // Crystal's default integer is Int32; Rails schemas typically
        // use BigInt for IDs, so Int64 is the safer default for the
        // scaffold.
        Ty::Int => "Int64".to_string(),
        Ty::Float => "Float64".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Str => "String".to_string(),
        // Crystal has native Symbol.
        Ty::Sym => "Symbol".to_string(),
        Ty::Nil => "Nil".to_string(),
        Ty::Array { elem } => format!("Array({})", crystal_ty(elem)),
        Ty::Hash { key, value } => format!("Hash({}, {})", crystal_ty(key), crystal_ty(value)),
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(crystal_ty).collect();
            format!("Tuple({})", parts.join(", "))
        }
        Ty::Record { .. } => "Hash(String, String)".to_string(),
        Ty::Union { variants } => {
            // Crystal union: `A | B | C`.
            let parts: Vec<String> = variants.iter().map(crystal_ty).collect();
            parts.join(" | ")
        }
        Ty::Class { id, .. } => id.0.to_string(),
        Ty::Fn { .. } => "Proc(Nil)".to_string(),
        Ty::Var { .. } => "_".to_string(),
    }
}

// Fixtures + specs ----------------------------------------------------

/// Emit one fixture file as `spec/fixtures/<table>.cr` — a `Fixtures::X`
/// module with one `self.<label>` accessor per fixture record. IDs
/// assigned in insertion order (1..N), matching the Rust emitter.
fn emit_crystal_fixture(lowered: &crate::lower::LoweredFixture) -> EmittedFile {
    let fixture_name = lowered.name.as_str();
    let class_name = lowered.class.0.as_str();
    let module_name = crate::naming::camelize(fixture_name);

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"../../src/app\"").unwrap();
    writeln!(s, "require \"../fixtures\"").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "module Fixtures").unwrap();
    writeln!(s, "  module {module_name}").unwrap();

    // `_load_all` — called from Fixtures.setup. Builds each record
    // through the model's `save` (so validations apply), captures
    // the AUTOINCREMENT id, registers it with the shared lookup map.
    writeln!(s, "    def self._load_all").unwrap();
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s, "      record = {class_name}.new").unwrap();
        for field in &record.fields {
            let col = field.column.as_str();
            let val = match &field.value {
                crate::lower::LoweredFixtureValue::Literal { ty, raw } => {
                    crystal_literal_for(raw, ty)
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
            writeln!(s, "      record.{col} = {val}").unwrap();
        }
        writeln!(
            s,
            "      raise \"fixture {fixture_name}/{label} failed to save\" unless record.save",
        )
        .unwrap();
        writeln!(
            s,
            "      Fixtures::FIXTURE_IDS[{{{fixture_name:?}, {label:?}}}] = record.id",
        )
        .unwrap();
    }
    writeln!(s, "    end").unwrap();

    // Named getters — `Fixtures::Articles.one` reads the record this
    // thread's `_load_all` inserted. A nil `find` means the spec's
    // before_each didn't fire.
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s).unwrap();
        writeln!(s, "    def self.{label} : {class_name}").unwrap();
        writeln!(
            s,
            "      id = Fixtures.fixture_id({fixture_name:?}, {label:?})",
        )
        .unwrap();
        writeln!(
            s,
            "      {class_name}.find(id).not_nil!"
        )
        .unwrap();
        writeln!(s, "    end").unwrap();
    }
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();

    EmittedFile {
        path: PathBuf::from(format!("spec/fixtures/{fixture_name}.cr")),
        content: s,
    }
}

/// `spec/fixtures.cr` — top-level `Fixtures` module that owns the
/// `(fixture_name, label) → id` lookup and the `setup` entrypoint
/// every spec's `before_each` calls. Per-class submodules reopen
/// `Fixtures` in `spec/fixtures/<table>.cr`.
fn emit_fixtures_helper(lowered: &crate::lower::LoweredFixtureSet) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"../src/app\"").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "module Fixtures").unwrap();
    writeln!(s, "  FIXTURE_IDS = {{}} of {{String, String}} => Int64").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "  # Per-spec entry point. Opens a fresh :memory: SQLite\n  # connection, runs the schema DDL, and loads every fixture in\n  # declaration order. Idempotent across repeat calls.",
    )
    .unwrap();
    writeln!(s, "  def self.setup").unwrap();
    writeln!(
        s,
        "    Roundhouse::Db.setup_test_db(Roundhouse::SchemaSQL::CREATE_TABLES)"
    )
    .unwrap();
    writeln!(s, "    FIXTURE_IDS.clear").unwrap();
    for f in &lowered.fixtures {
        let mod_name = crate::naming::camelize(f.name.as_str());
        writeln!(s, "    {mod_name}._load_all").unwrap();
    }
    writeln!(s, "  end").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "  def self.fixture_id(fixture : String, label : String) : Int64",
    )
    .unwrap();
    writeln!(s, "    FIXTURE_IDS[{{fixture, label}}]").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("spec/fixtures.cr"),
        content: s,
    }
}

fn crystal_literal_for(value: &str, ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                format!("{value}_i64")
            } else {
                format!("0_i64 # TODO: coerce {value:?}")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                format!("{value}_f64")
            } else {
                format!("0.0_f64 # TODO: coerce {value:?}")
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

fn emit_spec_helper(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"spec\"").unwrap();
    writeln!(s, "require \"../src/app\"").unwrap();
    if !app.fixtures.is_empty() {
        writeln!(s, "require \"./fixtures\"").unwrap();
    }
    for fixture in &app.fixtures {
        writeln!(s, "require \"./fixtures/{}\"", fixture.name).unwrap();
    }
    if !app.fixtures.is_empty() {
        writeln!(s).unwrap();
        // Fresh :memory: DB + reloaded fixtures before every spec,
        // mirroring Rails' transactional-fixture isolation.
        writeln!(s, "Spec.before_each do").unwrap();
        writeln!(s, "  Fixtures.setup").unwrap();
        writeln!(s, "end").unwrap();
    }
    EmittedFile {
        path: PathBuf::from("spec/spec_helper.cr"),
        content: s,
    }
}

fn emit_crystal_spec(tm: &TestModule, app: &App) -> EmittedFile {
    // Target class: Article for ArticleTest. Used as the `describe`
    // subject so the spec output reads naturally. For controller
    // tests we use the test-module name as a string instead —
    // Crystal resolves bare-identifier describe subjects at parse
    // time and the controller class isn't required into the spec
    // build yet (Phase 4 skips).
    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");
    let subject = if is_controller_test {
        format!("{:?}", tm.name.0.as_str())
    } else {
        tm.target
            .as_ref()
            .map(|c| c.0.as_str().to_string())
            .unwrap_or_else(|| tm.name.0.as_str().to_string())
    };

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

    let ctx = SpecCtx {
        app,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
    };

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "require \"../spec_helper\"").unwrap();
    if is_controller_test {
        writeln!(s, "require \"../../src/routes\"").unwrap();
        writeln!(s, "require \"../../src/route_helpers\"").unwrap();
        writeln!(s, "require \"../../src/test_support\"").unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "describe {subject} do").unwrap();

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");
    for test in &tm.tests {
        if is_controller_test {
            emit_cr_controller_test(&mut s, test, app);
        } else if test_needs_runtime_unsupported_cr(test) {
            writeln!(
                s,
                "  pending {:?} do",
                test.name
            )
            .unwrap();
            writeln!(s, "    # Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "  end").unwrap();
        } else {
            writeln!(s, "  it {:?} do", test.name).unwrap();
            for line in emit_spec_body(&test.body, ctx).lines() {
                writeln!(s, "    {line}").unwrap();
            }
            writeln!(s, "  end").unwrap();
        }
    }

    writeln!(s, "end").unwrap();

    let filename = crate::naming::snake_case(tm.name.0.as_str());
    // spec/models/article_spec.cr from ArticleTest.
    let filename = filename.replace("_test", "_spec");
    EmittedFile {
        path: PathBuf::from(format!("spec/models/{filename}.cr")),
        content: s,
    }
}

/// Context threaded through spec body emission.
#[derive(Clone, Copy)]
struct SpecCtx<'a> {
    app: &'a App,
    fixture_names: &'a [Symbol],
    known_models: &'a [Symbol],
    model_attrs: &'a [Symbol],
}

/// Top-level emit for a test body — a `Seq` renders line by line; a
/// single expression renders as one line.
fn emit_spec_body(body: &Expr, ctx: SpecCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|e| emit_spec_stmt(e, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_spec_stmt(body, ctx),
    }
}

fn emit_spec_stmt(e: &Expr, ctx: SpecCtx) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_spec_expr(value, ctx))
        }
        _ => emit_spec_expr(e, ctx),
    }
}

fn emit_spec_expr(e: &Expr, ctx: SpecCtx) -> String {
    use crate::expr::InterpPart;
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!("{} => {}", emit_spec_expr(k, ctx), emit_spec_expr(v, ctx))
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_spec_expr(e, ctx)).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            let mut out = String::from("\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => out.push_str(value),
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_spec_expr(expr, ctx));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_spec_send(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!(
                "{} {} {}",
                emit_spec_expr(left, ctx),
                op_s,
                emit_spec_expr(right, ctx)
            )
        }
        ExprNode::Assign { target, value } => {
            // Only in rare inline positions; renders as a Crystal
            // assignment expression.
            let v = emit_spec_expr(value, ctx);
            match target {
                LValue::Var { name, .. } => format!("{name} = {v}"),
                LValue::Ivar { name } => format!("@{name} = {v}"),
                _ => v,
            }
        }
        _ => format!("# TODO: spec emit for {:?}", std::mem::discriminant(&*e.node)),
    }
}

fn emit_spec_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: SpecCtx,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_spec_expr(a, ctx)).collect();

    // Fixture accessor: articles(:one) → Fixtures::Articles.one
    if recv.is_none()
        && args.len() == 1
        && ctx.fixture_names.iter().any(|s| s.as_str() == method)
    {
        if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*args[0].node {
            let module_name = crate::naming::camelize(method);
            return format!("Fixtures::{module_name}.{}", sym.as_str());
        }
    }

    // assert_difference("Class.count", delta) do ... end
    if recv.is_none() && method == "assert_difference" {
        if let Some(body) = block {
            if let Some(count_expr) = args
                .first()
                .and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => {
                        rewrite_ruby_dot_call_cr(value)
                    }
                    _ => None,
                })
            {
                let delta = args_s.get(1).cloned().unwrap_or_else(|| "1_i64".into());
                let body_s = emit_block_body_cr(body, ctx);
                return format!(
                    "_before = {count_expr}\n    {body_s}\n    ({count_expr} - _before).should eq({delta})"
                );
            }
        }
    }

    // owner.<assoc>.create(hash) / .build(hash) — HasMany rewrite.
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
                    if let Some(s) = try_emit_assoc_create_cr(
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

    // Assertion macros → Crystal spec's `should` matchers.
    // Ruby's `assert_equal expected, actual` → `actual.should eq(expected)`.
    if recv.is_none() {
        match (method, args_s.len()) {
            ("assert_equal", 2) => {
                return format!("{}.should eq({})", args_s[1], args_s[0]);
            }
            ("assert_not_equal", 2) => {
                return format!("{}.should_not eq({})", args_s[1], args_s[0]);
            }
            ("assert_not", 1) => {
                return format!("{}.should be_false", args_s[0]);
            }
            ("assert", 1) => {
                return format!("{}.should be_true", args_s[0]);
            }
            ("assert_nil", 1) => {
                return format!("{}.should be_nil", args_s[0]);
            }
            ("assert_not_nil", 1) => {
                return format!("{}.should_not be_nil", args_s[0]);
            }
            _ => {}
        }
    }

    // `Class.new(hash)` → Crystal requires setting properties after .new.
    // Use `.tap do |r| r.k = v end` pattern when the target is a known
    // model.
    if let Some(r) = recv {
        if method == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class_name) = path.last() {
                    if ctx.known_models.iter().any(|s| s == class_name) {
                        if let ExprNode::Hash { entries, .. } = &*args[0].node {
                            let mut lines: Vec<String> =
                                vec![format!("{class_name}.new.tap do |record|")];
                            for (k, v) in entries {
                                if let ExprNode::Lit {
                                    value: Literal::Sym { value: f },
                                } = &*k.node
                                {
                                    lines.push(format!(
                                        "      record.{} = {}",
                                        f.as_str(),
                                        emit_spec_expr(v, ctx)
                                    ));
                                }
                            }
                            lines.push("    end".to_string());
                            return lines.join("\n");
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
            let recv_s = emit_spec_expr(r, ctx);
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            let sep = if is_class_call { "." } else { "." };
            // Crystal doesn't distinguish field vs method call syntactically
            // (property getters ARE method calls), so we can emit both as
            // `recv.method` regardless — but attribute reads should be
            // bare, no parens.
            let is_attr_read = !is_class_call
                && args_s.is_empty()
                && ctx.model_attrs.iter().any(|s| s.as_str() == method);
            if is_attr_read {
                format!("{recv_s}{sep}{method}")
            } else if args_s.is_empty() {
                // Crystal allows omitting parens for zero-arg calls;
                // keep them off for consistency with Ruby idiom.
                format!("{recv_s}{sep}{method}")
            } else {
                format!("{recv_s}{sep}{method}({})", args_s.join(", "))
            }
        }
    }
}

/// Phase 3 rounded out the Crystal emitter's handling of
/// assert_difference, destroy, Class.count, build/create, and
/// belongs_to existence. Keep the predicate as a future-guard; no
/// real-blog shape currently forces a skip.
fn test_needs_runtime_unsupported_cr(_test: &Test) -> bool {
    false
}

/// Parse a Ruby-style `"Class.method"` expression string into Crystal
/// `Class.method` syntax (Crystal uses `.` for both class and instance
/// method calls). Only alphanumeric identifiers on both sides.
fn rewrite_ruby_dot_call_cr(expr: &str) -> Option<String> {
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
    Some(format!("{lhs}.{rhs}"))
}

/// Render a Ruby block body (a single expression or a Seq) as Crystal
/// spec statements at spec-body indent. Peels one Lambda layer — Ruby
/// `do ... end` lowers to `ExprNode::Lambda` in the IR but translates
/// to plain statements in Crystal.
fn emit_block_body_cr(e: &Expr, ctx: SpecCtx) -> String {
    let inner = match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    };
    match &*inner.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|s| emit_spec_stmt(s, ctx))
            .collect::<Vec<_>>()
            .join("\n    "),
        _ => emit_spec_stmt(inner, ctx),
    }
}

fn try_emit_assoc_create_cr(
    owner: &Expr,
    assoc_name: &str,
    args: &[Expr],
    outer_method: &str,
    ctx: SpecCtx,
) -> Option<String> {
    let resolved = crate::lower::resolve_has_many(
        &Symbol::from(assoc_name),
        owner.ty.as_ref(),
        ctx.app,
    )?;
    let target_class = resolved.target_class.0.as_str();
    let foreign_key = resolved.foreign_key.as_str();

    let owner_s = emit_spec_expr(owner, ctx);
    let hash_entries = match &args.first()?.node.as_ref() {
        ExprNode::Hash { entries, .. } => entries.clone(),
        _ => return None,
    };

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("{target_class}.new.tap do |record|"));
    lines.push(format!("      record.{foreign_key} = {owner_s}.id"));
    for (k, v) in &hash_entries {
        if let ExprNode::Lit { value: Literal::Sym { value: field_name } } = &*k.node {
            lines.push(format!(
                "      record.{} = {}",
                field_name.as_str(),
                emit_spec_expr(v, ctx),
            ));
        }
    }
    // `.build` returns the unsaved record; `.create` saves it first.
    // Both yield the record so tests can read `.article_id` etc.
    if outer_method == "create" {
        lines.push("      record.save".to_string());
    }
    lines.push("    end".to_string());
    Some(lines.join("\n"))
}

#[allow(dead_code)]
fn test_body_uses_unsupported_cr(e: &Expr) -> bool {
    use crate::expr::InterpPart;
    let self_hit = false;
    if self_hit {
        return true;
    }
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if test_body_uses_unsupported_cr(r) {
                    return true;
                }
            }
            for a in args {
                if test_body_uses_unsupported_cr(a) {
                    return true;
                }
            }
            if let Some(b) = block {
                if test_body_uses_unsupported_cr(b) {
                    return true;
                }
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                if test_body_uses_unsupported_cr(e) {
                    return true;
                }
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                if test_body_uses_unsupported_cr(k) || test_body_uses_unsupported_cr(v) {
                    return true;
                }
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    if test_body_uses_unsupported_cr(expr) {
                        return true;
                    }
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            if test_body_uses_unsupported_cr(left) || test_body_uses_unsupported_cr(right) {
                return true;
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            if test_body_uses_unsupported_cr(cond)
                || test_body_uses_unsupported_cr(then_branch)
                || test_body_uses_unsupported_cr(else_branch)
            {
                return true;
            }
        }
        ExprNode::Let { value, body, .. } => {
            if test_body_uses_unsupported_cr(value) || test_body_uses_unsupported_cr(body) {
                return true;
            }
        }
        ExprNode::Lambda { body, .. } => {
            if test_body_uses_unsupported_cr(body) {
                return true;
            }
        }
        ExprNode::Assign { value, .. } => {
            if test_body_uses_unsupported_cr(value) {
                return true;
            }
        }
        _ => {}
    }
    false
}

// --- Pass-2 route helpers + views + router --------------------------------

/// `src/route_helpers.cr` — `def self.<name>_path(...) : String` per
/// entry in the flat route table. Mirrors `src/route_helpers.rs`/.ts
/// in shape; Crystal naming stays snake_case.
fn emit_route_helpers_cr(app: &App) -> EmittedFile {
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "module RouteHelpers").unwrap();
    let mut seen: std::collections::BTreeSet<(String, usize)> =
        std::collections::BTreeSet::new();
    for r in &flat {
        if !seen.insert((r.as_name.clone(), r.path_params.len())) {
            continue;
        }
        let params_sig: Vec<String> = r
            .path_params
            .iter()
            .map(|p| format!("{p} : Int64"))
            .collect();
        let sig = if params_sig.is_empty() {
            String::new()
        } else {
            format!("({})", params_sig.join(", "))
        };
        let path_expr = if r.path_params.is_empty() {
            format!("{:?}", r.path)
        } else {
            let mut out = String::from("\"");
            for part in r.path.split('/') {
                if part.is_empty() {
                    continue;
                }
                out.push('/');
                if let Some(name) = part.strip_prefix(':') {
                    out.push_str(&format!("#{{{}}}", name));
                } else {
                    out.push_str(part);
                }
            }
            out.push('"');
            out
        };
        writeln!(s, "  def self.{}_path{sig} : String", r.as_name).unwrap();
        writeln!(s, "    {path_expr}").unwrap();
        writeln!(s, "  end").unwrap();
    }
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("src/route_helpers.cr"),
        content: s,
    }
}

/// Map `dialect::HttpMethod` to the Crystal runtime's uppercase
/// method-string form used when registering routes.
fn http_verb_cr(m: &crate::dialect::HttpMethod) -> &'static str {
    use crate::dialect::HttpMethod;
    match m {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Any => "ANY",
    }
}
