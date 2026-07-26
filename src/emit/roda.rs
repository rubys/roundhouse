//! Rails → Roda + Sequel source-to-source converter (`--target roda`),
//! the issue #67 spike.
//!
//! Unlike every runtime target, this emitter consumes the INGEST-shape
//! `App` — `bin/roundhouse` skips `analyze_and_lower` for it. The
//! conversion is source-to-source through the typed surface IR
//! (`RouteSpec` → `FlatRoute`, `Model`, `Validation`, `Schema`) plus the
//! ingest `Expr` trees for action bodies, where the Rails idioms are
//! still visible (`Article.includes(:comments)`, `redirect_to @article,
//! notice: …`) and map near-1:1 onto their Sequel/Roda equivalents. The
//! post-lowering IR is runtime vocabulary (SQL-folded queries, `Views::`
//! calls) — the wrong altitude to re-idiomize from.
//!
//! The output runs on the REAL roda + sequel gems, not the roundhouse
//! runtime. The reviewed reference for output shape is the hand-written
//! exemplar rubys/roda-sequel-blog (vendored at `fixtures/roda-blog`),
//! domain-identical to `fixtures/real-blog`. Two gates:
//!
//!   1. behavioral — the ported oracle
//!      (`tests/roda_oracle/blog_oracle_test.rb`) driven through the
//!      emitted tree's config.ru;
//!   2. round-trip — re-ingest the emitted app through the Roda
//!      front-end and diff IR against the Rails ingest (Jeremy's
//!      proposed equivalence test, #67).
//!
//! Conversion rule (Jeremy's, #67): convert exactly what maps, and
//! leave everything else as a `# ROUNDHOUSE-TODO` comment carrying the
//! original Rails source, so a human finishes the residue by hand —
//! the source-comment rendering of the diagnostics-ledger discipline.
//!
//! Route re-nesting: the flat route table is rebuilt into a segment
//! trie, which structurally cannot emit duplicate branches (the
//! usefulness bar Jeremy named). `:id`-style params whose backing
//! column is an integer primary/foreign key become `Integer` matchers —
//! Rails doesn't constrain them in routes.rb, but its `find` raises
//! RecordNotFound on non-numeric ids, so the observable behavior
//! (404) is preserved while gaining Roda's idiomatic typed matcher.

use indexmap::IndexMap;
use std::path::PathBuf;

use crate::app::App;
use crate::dialect::{
    Action, Association, Controller, ControllerBodyItem, Dependent, FilterKind, HttpMethod,
    Model, ModelBodyItem, RenderTarget, ValidationRule,
};
use crate::expr::{Expr, ExprNode, Literal};
use crate::lower::routes::{flatten_routes, FlatRoute};
use crate::naming;
use crate::schema::{ColumnType, Table};

use super::ruby::emit_expr;
use super::EmittedFile;

mod views;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files: Vec<EmittedFile> = Vec::new();
    files.push(file("Gemfile", GEMFILE));
    files.push(file("config.ru", CONFIG_RU));
    files.push(file("db.rb", DB_RB));
    files.extend(emit_migrations(app));
    files.extend(emit_models(app));
    files.push(emit_app_rb(app));
    files.extend(emit_views(app));
    files
}

fn file(path: &str, content: &str) -> EmittedFile {
    EmittedFile { path: PathBuf::from(path), content: content.to_string() }
}

// ── Static scaffold ─────────────────────────────────────────────────

const GEMFILE: &str = r#"source "https://rubygems.org"

gem "roda"      # routing tree
gem "sequel"    # ORM (model + dataset levels)
gem "sqlite3"   # database

gem "erubi"     # ERB engine used by Roda's render plugin
gem "tilt"      # template interface

gem "rack"
gem "rackup"    # `rackup` CLI
gem "puma"      # app server

group :test do
  gem "minitest"
  gem "rack-test"
end
"#;

const CONFIG_RU: &str = r#"require_relative "app"

run App.freeze.app
"#;

const DB_RB: &str = r#"# Database connection + schema.
#
# Sequel connects before any model class is defined (models subclass
# Sequel::Model, which needs a DB handle at class-definition time), and the
# migrations in db/migrate are run on boot so the app is runnable with no
# separate setup step.
require "sequel"

DB = Sequel.sqlite(ENV.fetch("DATABASE", File.expand_path("db/blog.db", __dir__)))

Sequel.extension :migration
Sequel::Migrator.run(DB, File.expand_path("db/migrate", __dir__))

# Sequel raises Sequel::ValidationFailed from #save on an invalid model by
# default (like ActiveRecord's #save!). Turning that off makes #save return
# nil/false on failure, so an `if model.save` branch validates exactly once.
Sequel::Model.raise_on_save_failure = false

Sequel::Model.plugin :validation_helpers          # explicit validations in #validate
Sequel::Model.plugin :timestamps, update_on_create: true
"#;

// ── Migrations (Schema → Sequel.migration) ──────────────────────────

fn emit_migrations(app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();
    for (i, (_, table)) in app.schema.tables.iter().enumerate() {
        let path = format!("db/migrate/{:03}_create_{}.rb", i + 1, table.name);
        out.push(EmittedFile {
            path: PathBuf::from(path),
            content: migration_for(app, table),
        });
    }
    out
}

/// One `create_table` migration, exemplar-shaped. `dependent: :destroy`
/// on the owning Rails association becomes `on_delete: :cascade` on the
/// child's foreign key — Sequel has no model-level `dependent:` option;
/// the DB-level cascade is the idiomatic equivalent (and what the
/// reviewed exemplar does).
fn migration_for(app: &App, table: &Table) -> String {
    let fk_cols: Vec<&str> = table
        .foreign_keys
        .iter()
        .map(|fk| fk.from_column.as_str())
        .collect();
    let mut lines: Vec<String> = Vec::new();
    for col in &table.columns {
        if col.primary_key {
            lines.push(format!("      primary_key :{}", col.name));
            continue;
        }
        if fk_cols.contains(&col.name.as_str()) {
            let fk = table
                .foreign_keys
                .iter()
                .find(|fk| fk.from_column == col.name)
                .unwrap();
            let mut l = format!("      foreign_key :{}, :{}", col.name, fk.to_table);
            if !col.nullable {
                l.push_str(", null: false");
            }
            if owner_destroys_dependents(app, table, fk.to_table.0.as_str()) {
                l.push_str(", on_delete: :cascade");
            }
            lines.push(l);
            continue;
        }
        let (ty, extra) = sequel_column_type(&col.col_type);
        let mut l = format!("      {} :{}{}", ty, col.name, extra);
        if !col.nullable {
            l.push_str(", null: false");
        }
        lines.push(l);
    }
    for idx in &table.indexes {
        // The FK line above doesn't auto-index; keep the schema's
        // explicit indexes (minus any on the primary key).
        let cols = if idx.columns.len() == 1 {
            format!(":{}", idx.columns[0])
        } else {
            format!(
                "[{}]",
                idx.columns.iter().map(|c| format!(":{c}")).collect::<Vec<_>>().join(", ")
            )
        };
        let unique = if idx.unique { ", unique: true" } else { "" };
        lines.push(format!("      index {cols}{unique}"));
    }
    format!(
        "Sequel.migration do\n  change do\n    create_table(:{}) do\n{}\n    end\n  end\nend\n",
        table.name,
        lines.join("\n"),
    )
}

/// Does the model owning `parent_table` declare `dependent: :destroy`
/// (or `:delete_all`) on the association pointing back at `table`?
fn owner_destroys_dependents(app: &App, table: &Table, parent_table: &str) -> bool {
    app.models.iter().any(|m| {
        m.table.0.as_str() == parent_table
            && m.associations().any(|a| match a {
                Association::HasMany { target, dependent, .. } => {
                    matches!(dependent, Dependent::Destroy | Dependent::DeleteAll)
                        && model_table(app, target.0.as_str())
                            .is_some_and(|t| t == table.name.as_str())
                }
                _ => false,
            })
    })
}

fn model_table<'a>(app: &'a App, class_name: &str) -> Option<&'a str> {
    app.models
        .iter()
        .find(|m| m.name.0.as_str() == class_name)
        .map(|m| m.table.0.as_str())
}

fn sequel_column_type(ty: &ColumnType) -> (&'static str, &'static str) {
    match ty {
        ColumnType::Integer => ("Integer", ""),
        ColumnType::BigInt => ("Bignum", ""),
        ColumnType::Float => ("Float", ""),
        ColumnType::Decimal { .. } => ("BigDecimal", ""),
        ColumnType::String { .. } => ("String", ""),
        ColumnType::Text => ("String", ", text: true"),
        ColumnType::Boolean => ("TrueClass", ""),
        ColumnType::Date => ("Date", ""),
        ColumnType::DateTime => ("DateTime", ""),
        ColumnType::Time => ("Time", ""),
        ColumnType::Binary => ("File", ""),
        // No 1:1 Sequel generic type; store as text and note it.
        ColumnType::Json => ("String", ", text: true # was json"),
        ColumnType::Reference { .. } => ("Integer", " # was t.references"),
    }
}

// ── Models (Model → Sequel::Model) ──────────────────────────────────

/// Models that exist as tables — the Rails abstract base
/// (`ApplicationRecord`, `primary_abstract_class`) has no Sequel
/// equivalent: `Sequel::Model` itself plays that role, and emitting it
/// would crash at load (a Sequel::Model subclass resolves its table at
/// class-definition time).
fn concrete_models(app: &App) -> impl Iterator<Item = &Model> {
    app.models.iter().filter(|m| m.name.0.as_str() != "ApplicationRecord")
}

fn emit_models(app: &App) -> Vec<EmittedFile> {
    concrete_models(app)
        .map(|m| EmittedFile {
            path: PathBuf::from(format!(
                "models/{}.rb",
                naming::snake_case(m.name.0.as_str())
            )),
            content: model_for(m),
        })
        .collect()
}

fn model_for(model: &Model) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("class {} < Sequel::Model", model.name.0));

    let mut validate_lines: Vec<String> = Vec::new();
    let mut presence: Vec<String> = Vec::new();

    for item in &model.body {
        match item {
            ModelBodyItem::Association { assoc, .. } => {
                lines.push(format!("  {}", association_line(assoc)));
            }
            ModelBodyItem::Validation { validation, .. } => {
                for rule in &validation.rules {
                    match rule {
                        ValidationRule::Presence => {
                            presence.push(format!(":{}", validation.attribute));
                        }
                        ValidationRule::Length { min, max, message } => {
                            let msg = message
                                .as_ref()
                                .map(|m| format!(", message: {m:?}"))
                                .unwrap_or_default();
                            if let Some(min) = min {
                                validate_lines.push(format!(
                                    "    validates_min_length {min}, :{}{msg}",
                                    validation.attribute
                                ));
                            }
                            if let Some(max) = max {
                                validate_lines.push(format!(
                                    "    validates_max_length {max}, :{}{msg}",
                                    validation.attribute
                                ));
                            }
                        }
                        ValidationRule::Uniqueness { .. } => {
                            validate_lines.push(format!(
                                "    validates_unique :{}",
                                validation.attribute
                            ));
                        }
                        ValidationRule::Format { pattern } => {
                            validate_lines.push(format!(
                                "    validates_format /{pattern}/, :{}",
                                validation.attribute
                            ));
                        }
                        ValidationRule::Numericality { only_integer, .. } => {
                            let helper =
                                if *only_integer { "validates_integer" } else { "validates_numeric" };
                            validate_lines.push(format!(
                                "    {helper} :{}",
                                validation.attribute
                            ));
                        }
                        ValidationRule::Inclusion { values } => {
                            let list = values
                                .iter()
                                .map(literal_src)
                                .collect::<Vec<_>>()
                                .join(", ");
                            validate_lines.push(format!(
                                "    validates_includes [{list}], :{}",
                                validation.attribute
                            ));
                        }
                        other => {
                            validate_lines.push(format!(
                                "    # ROUNDHOUSE-TODO: unconverted validation on :{} ({other:?})",
                                validation.attribute
                            ));
                        }
                    }
                }
            }
            // No Roda/Sequel equivalent is wired for these (Turbo Stream
            // broadcasts, AR lifecycle callbacks, …): carry the original
            // source as a comment per the #67 conversion rule.
            ModelBodyItem::Callback { callback, .. } => {
                lines.push(format!(
                    "  # ROUNDHOUSE-TODO: unconverted Rails callback: {:?} -> {}",
                    callback.hook,
                    callback
                        .targets
                        .iter()
                        .map(|t| format!(":{t}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            ModelBodyItem::Scope { scope, .. } => {
                lines.push(format!(
                    "  # ROUNDHOUSE-TODO: unconverted scope :{} (map to a Sequel dataset method)",
                    scope.name
                ));
            }
            ModelBodyItem::Method { method, .. } => {
                // Plain instance methods carry over verbatim — Sequel
                // models are ordinary Ruby classes.
                for l in super::ruby::emit_method(method).lines() {
                    lines.push(format!("  {l}").trim_end().to_string());
                }
            }
            ModelBodyItem::Unknown { expr, .. } => {
                for l in emit_expr(expr).lines() {
                    lines.push(format!("  # ROUNDHOUSE-TODO: unconverted: {l}"));
                }
            }
        }
    }

    if !presence.is_empty() {
        let joined = presence.join(", ");
        validate_lines.insert(0, format!("    validates_presence [{joined}]"));
    }
    if !validate_lines.is_empty() {
        lines.push(String::new());
        lines.push("  def validate".to_string());
        lines.push("    super".to_string());
        lines.extend(validate_lines);
        lines.push("  end".to_string());
    }
    lines.push("end".to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn association_line(assoc: &Association) -> String {
    match assoc {
        Association::HasMany { name, through, dependent, scope, .. } => {
            if let Some(th) = through {
                return format!(
                    "# ROUNDHOUSE-TODO: unconverted: has_many :{name}, through: :{th} \
                     (Sequel: many_to_many or many_through_many)"
                );
            }
            let mut l = format!("one_to_many :{name}");
            if let Some(order) = scope.as_ref().and_then(assoc_scope_order) {
                l.push_str(&format!(", order: {order}"));
            }
            match dependent {
                // Enforced at the DB level: the child's foreign key is
                // emitted with on_delete: :cascade (see db/migrate).
                Dependent::Destroy | Dependent::DeleteAll => {
                    l.push_str("   # dependent: :destroy -> FK on_delete: :cascade");
                }
                Dependent::None => {}
                other => l.push_str(&format!("   # ROUNDHOUSE-TODO: dependent: {other:?}")),
            }
            l
        }
        Association::BelongsTo { name, .. } => format!("many_to_one :{name}"),
        Association::HasOne { name, .. } => format!("one_to_one :{name}"),
        Association::HasAndBelongsToMany { name, .. } => {
            format!("many_to_many :{name}")
        }
    }
}

/// `-> { order(created_at: :desc) }` association scope → `Sequel.desc(:created_at)`.
fn assoc_scope_order(scope: &Expr) -> Option<String> {
    let ExprNode::Send { recv: None, method, args, .. } = &*scope.node else { return None };
    if method.as_str() != "order" || args.len() != 1 {
        return None;
    }
    order_arg_to_sequel(&args[0])
}

/// `created_at: :desc` → `Sequel.desc(:created_at)`; `:created_at` → `:created_at`.
fn order_arg_to_sequel(arg: &Expr) -> Option<String> {
    match &*arg.node {
        ExprNode::Hash { entries, .. } if entries.len() == 1 => {
            let (k, v) = &entries[0];
            let (ExprNode::Lit { value: Literal::Sym { value: col } },
                 ExprNode::Lit { value: Literal::Sym { value: dir } }) = (&*k.node, &*v.node)
            else {
                return None;
            };
            match dir.as_str() {
                "desc" => Some(format!("Sequel.desc(:{col})")),
                "asc" => Some(format!(":{col}")),
                _ => None,
            }
        }
        ExprNode::Lit { value: Literal::Sym { value: col } } => Some(format!(":{col}")),
        _ => None,
    }
}

fn literal_src(l: &Literal) -> String {
    match l {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => value.to_string(),
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!(":{value}"),
        Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
    }
}

// ── Route trie ──────────────────────────────────────────────────────

/// One node of the path-segment trie rebuilt from the flat route table.
#[derive(Default)]
struct Node {
    /// Static segment children, in first-route-seen order.
    stat: IndexMap<String, Node>,
    /// The dynamic (`:param`) child — Rails routes at the same position
    /// share it even when the param is named differently per route
    /// (`:id` on the member routes, `:article_id` under the nested
    /// resource); the names are collected for binding decisions.
    dynamic: Option<(Vec<String>, Box<Node>)>,
    /// Routes whose path terminates exactly at this node.
    terminals: Vec<FlatRoute>,
}

fn build_trie(routes: &[FlatRoute]) -> Node {
    let mut root = Node::default();
    for r in routes {
        let mut node = &mut root;
        for seg in r.path.split('/').filter(|s| !s.is_empty()) {
            if let Some(param) = seg.strip_prefix(':') {
                let (names, child) =
                    node.dynamic.get_or_insert_with(|| (Vec::new(), Box::default()));
                if !names.iter().any(|n| n == param) {
                    names.push(param.to_string());
                }
                node = child;
            } else {
                node = node.stat.entry(seg.to_string()).or_default();
            }
        }
        node.terminals.push(r.clone());
    }
    root
}

fn verb(m: &HttpMethod) -> &'static str {
    match m {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
        HttpMethod::Any => "on",
    }
}

// ── Filters → interior loads ────────────────────────────────────────

/// A recognized `before_action` whose target method is the Rails
/// find-by-param idiom (`@article = Article.find(params.expect(:id))`),
/// convertible to Roda's interior-node load-and-abort
/// (`next unless @article = Article[id]`).
#[derive(Clone, Debug)]
struct FilterLoad {
    controller: String,
    ivar: String,
    model: String,
    param: String,
    only: Vec<String>,
    except: Vec<String>,
}

fn collect_filter_loads(app: &App) -> Vec<FilterLoad> {
    let mut out = Vec::new();
    for c in &app.controllers {
        for f in c.filters() {
            if !matches!(f.kind, FilterKind::Before) {
                continue;
            }
            let Some(action) = find_action(c, f.target.as_str()) else { continue };
            let Some((ivar, model, param)) = find_by_param_shape(&action.body) else {
                continue;
            };
            out.push(FilterLoad {
                controller: c.name.0.to_string(),
                ivar,
                model,
                param,
                only: f.only.iter().map(|s| s.to_string()).collect(),
                except: f.except.iter().map(|s| s.to_string()).collect(),
            });
        }
    }
    out
}

fn find_action<'a>(c: &'a Controller, name: &str) -> Option<&'a Action> {
    c.body.iter().find_map(|item| match item {
        ControllerBodyItem::Action { action, .. } if action.name.as_str() == name => {
            Some(action)
        }
        _ => None,
    })
}

/// Match `@x = Model.find(<expr mentioning :key>)` (possibly wrapped in
/// a Seq of one statement). Returns (ivar, model, key).
fn find_by_param_shape(body: &Expr) -> Option<(String, String, String)> {
    let stmt = single_statement(body)?;
    let ExprNode::Assign { target, value } = &*stmt.node else { return None };
    let crate::expr::LValue::Ivar { name: ivar } = target else { return None };
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node else {
        return None;
    };
    if method.as_str() != "find" || args.len() != 1 {
        return None;
    }
    let ExprNode::Const { path } = &*recv.node else { return None };
    let key = first_symbol_in(&args[0])?;
    Some((ivar.to_string(), path.last()?.to_string(), key))
}

fn single_statement(body: &Expr) -> Option<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => Some(&exprs[0]),
        ExprNode::Seq { .. } => None,
        _ => Some(body),
    }
}

fn first_symbol_in(e: &Expr) -> Option<String> {
    if let ExprNode::Lit { value: Literal::Sym { value } } = &*e.node {
        return Some(value.to_string());
    }
    let mut found = None;
    e.node.for_each_child(&mut |c| {
        if found.is_none() {
            found = first_symbol_in(c);
        }
    });
    found
}

/// Does `load` apply to `action` under its only/except lists?
fn filter_covers(load: &FilterLoad, action: &str) -> bool {
    if !load.only.is_empty() {
        return load.only.iter().any(|a| a == action);
    }
    if !load.except.is_empty() {
        return !load.except.iter().any(|a| a == action);
    }
    true
}

// ── app.rb ──────────────────────────────────────────────────────────

fn emit_app_rb(app: &App) -> EmittedFile {
    let routes = flatten_routes(app);
    let trie = build_trie(&routes);
    let loads = collect_filter_loads(app);

    let mut requires = vec!["require_relative \"db\"".to_string()];
    for m in concrete_models(app) {
        requires.push(format!(
            "require_relative \"models/{}\"",
            naming::snake_case(m.name.0.as_str())
        ));
    }
    requires.push("require \"roda\"".to_string());
    requires.push("require \"rack/method_override\"".to_string());

    let mut body = String::new();
    let ctx = EmitCtx { app, routes: &routes, loads: &loads };
    emit_node(&trie, &ctx, 2, None, &[], &mut body);

    let content = format!(
        r##"{requires}

# Converted from a Rails application by roundhouse (`--target roda`,
# issue #67). Convertible constructs are emitted as idiomatic
# Roda/Sequel; everything else is left as a ROUNDHOUSE-TODO comment
# carrying the original Rails source.
class App < Roda
  # Browser forms can only POST; a hidden `_method` field carries the real
  # verb (PATCH/DELETE) — the Roda-idiomatic equivalent of Rails' implicit
  # method override.
  use Rack::MethodOverride

  # `escape: true` makes `<%= %>` HTML-escape and `<%== %>` emit raw.
  plugin :render, escape: true, layout: "layout"
  plugin :part                       # render partials with locals
  plugin :all_verbs                  # r.patch / r.delete
  plugin :sessions, secret: ENV.fetch("SESSION_SECRET") {{ "dev-secret-" + "0" * 53 }}
  plugin :flash
  plugin :not_found do
    view "not_found"
  end

  route do |r|
{body}  end

  # --- view helpers ---------------------------------------------------

  def truncate(text, length: 100)
    text = text.to_s
    text.length > length ? "#{{text[0, length]}}…" : text
  end

  def pluralize(count, singular)
    "#{{count}} #{{count == 1 ? singular : "#{{singular}}s"}}"
  end
end
"##,
        requires = requires.join("\n"),
        body = body,
    );
    EmittedFile { path: PathBuf::from("app.rb"), content }
}

struct EmitCtx<'a> {
    app: &'a App,
    routes: &'a [FlatRoute],
    loads: &'a [FilterLoad],
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

/// Emit one trie node's routing code. Ordering: `r.root` (root node
/// only), then terminals (as an `r.is` block when several verbs
/// terminate here, or `r.<verb> true` when one does and branches
/// continue below), then static children (leaf single-verb children
/// collapse to `r.<verb> "seg"`), then the dynamic child.
fn emit_node(
    node: &Node,
    ctx: &EmitCtx,
    depth: usize,
    parent_seg: Option<&str>,
    bindings: &[(String, String)],
    out: &mut String,
) {
    let pad = indent(depth);

    // Root-node terminals: `GET /` becomes r.root. When the same
    // controller#action also serves a static path (Rails' root +
    // resources index duplication), redirect to the canonical URL —
    // idiomatic Roda avoids two paths serving the same content (and the
    // reviewed exemplar does exactly this).
    if depth == 2 {
        for t in &node.terminals {
            if t.method == HttpMethod::Get {
                if let Some(canonical) = canonical_static_path(ctx.routes, t) {
                    out.push_str(&format!(
                        "{pad}# GET / -> canonical {canonical} (Rails served the index at both\n\
                         {pad}# paths; idiomatic Roda redirects to one canonical URL).\n\
                         {pad}r.root do\n{pad}  r.redirect \"{canonical}\"\n{pad}end\n"
                    ));
                } else {
                    out.push_str(&format!("{pad}r.root do\n"));
                    emit_terminal_body(t, ctx, depth + 1, bindings, out);
                    out.push_str(&format!("{pad}end\n"));
                }
            } else {
                out.push_str(&format!(
                    "{pad}# ROUNDHOUSE-TODO: unconverted root-level {} route\n",
                    verb(&t.method)
                ));
            }
        }
    } else {
        let has_children = !node.stat.is_empty() || node.dynamic.is_some();
        // Group terminals by verb, preserving source order. Two routes
        // sharing this path+verb (Rails routes distinguished only by a
        // `constraints:` regexp — Lobsters' single- vs multi-`/t/:tag`)
        // land in the same group; emitting one `r.<verb>` block per
        // terminal would shadow all but the first. Each group emits ONE
        // verb block whose body disambiguates on the constraint.
        let groups = group_terminals_by_verb(&node.terminals);
        if groups.len() > 1 || (groups.len() == 1 && !has_children) {
            out.push_str(&format!("{pad}r.is do\n"));
            for (method, ts) in &groups {
                out.push_str(&format!("{}r.{} do\n", indent(depth + 1), verb(method)));
                emit_verb_group_body(ts, ctx, depth + 2, bindings, out);
                out.push_str(&format!("{}end\n", indent(depth + 1)));
            }
            out.push_str(&format!("{pad}end\n"));
        } else if groups.len() == 1 {
            // Single verb terminates here but deeper branches exist:
            // `r.post true` — the argument makes Roda require full path
            // consumption, so longer paths fall through to the branches.
            let (method, ts) = &groups[0];
            out.push_str(&format!("{pad}r.{} true do\n", verb(method)));
            emit_verb_group_body(ts, ctx, depth + 1, bindings, out);
            out.push_str(&format!("{pad}end\n"));
        }
    }

    // Static children. A leaf child serving one verb collapses to the
    // matcher-argument form (`r.get "new" do … end`).
    for (seg, child) in &node.stat {
        if child.stat.is_empty() && child.dynamic.is_none() && child.terminals.len() == 1 {
            let t = &child.terminals[0];
            out.push_str(&format!("{pad}r.{} \"{seg}\" do\n", verb(&t.method)));
            emit_terminal_body(t, ctx, depth + 1, bindings, out);
            out.push_str(&format!("{pad}end\n"));
        } else {
            out.push_str(&format!("{pad}r.on \"{seg}\" do\n"));
            emit_node(child, ctx, depth + 1, Some(seg), bindings, out);
            out.push_str(&format!("{pad}end\n"));
        }
    }

    // Dynamic child. Id-shaped params (`:id` / `:*_id`) become `Integer`
    // matchers — Rails ids are integer PKs and `find` on a non-numeric
    // id 404s, so the typed matcher preserves observable behavior.
    // Anything else (`:username`, `:tag`) takes Roda's `String` matcher:
    // typing those Integer would silently unroute every real request.
    if let Some((names, child)) = &node.dynamic {
        let matcher = matcher_for(names);
        let is_leaf =
            child.stat.is_empty() && child.dynamic.is_none() && child.terminals.len() == 1;
        let var = block_var(names, is_leaf, parent_seg);
        // `/avatars/:username_size.png` puts a literal suffix inside the
        // dynamic segment. The `String` matcher above captures the whole
        // segment, suffix included, where Rails matched the suffix and
        // kept it out of the param — say so rather than emit a quiet
        // divergence.
        if let Some(n) = names.iter().find(|n| n.contains('.')) {
            out.push_str(&format!(
                "{pad}# ROUNDHOUSE-TODO: `:{n}` carries a literal suffix Rails matched \
                 separately; this matcher captures it as part of `{var}`\n"
            ));
        }
        // Every source param name at this position binds to the one
        // block variable; later (deeper) entries shadow earlier ones,
        // so body conversion resolves a param to its innermost binding.
        let mut inner: Vec<(String, String)> = bindings.to_vec();
        for n in names {
            inner.push((n.clone(), var.clone()));
        }
        if is_leaf {
            let t = &child.terminals[0];
            out.push_str(&format!("{pad}r.{} {matcher} do |{var}|\n", verb(&t.method)));
            emit_interior_loads(child, ctx, &var, names, depth + 1, out);
            emit_terminal_body(t, ctx, depth + 1, &inner, out);
            out.push_str(&format!("{pad}end\n"));
        } else {
            out.push_str(&format!("{pad}r.on {matcher} do |{var}|\n"));
            emit_interior_loads(child, ctx, &var, names, depth + 1, out);
            emit_node(child, ctx, depth + 1, None, &inner, out);
            out.push_str(&format!("{pad}end\n"));
        }
    }
}

/// Group a node's terminals by HTTP verb, preserving first-seen order
/// both across verbs and within each verb's list. Routes that share a
/// path+verb (Rails routes distinguished only by a `constraints:`
/// regexp) collect into one group so the caller disambiguates them in
/// a single verb block instead of emitting shadowing duplicates.
fn group_terminals_by_verb(terminals: &[FlatRoute]) -> Vec<(HttpMethod, Vec<&FlatRoute>)> {
    let mut groups: Vec<(HttpMethod, Vec<&FlatRoute>)> = Vec::new();
    for t in terminals {
        if let Some((_, ts)) = groups.iter_mut().find(|(m, _)| *m == t.method) {
            ts.push(t);
        } else {
            groups.push((t.method.clone(), vec![t]));
        }
    }
    groups
}

/// Emit the body of one verb block. A single terminal emits its body
/// directly. Several terminals (same path+verb, differing only by a
/// `constraints:` regexp — Lobsters' single- vs multi-`/t/:tag`) emit
/// an `if <regex>.match?(var) … elsif … else …` chain in Rails route
/// order (first match wins). The lone unconstrained route is the
/// `else`; if every route is constrained the chain simply ends and a
/// non-matching segment falls through to 404, exactly as in Rails.
fn emit_verb_group_body(
    ts: &[&FlatRoute],
    ctx: &EmitCtx,
    depth: usize,
    bindings: &[(String, String)],
    out: &mut String,
) {
    if ts.len() == 1 {
        emit_terminal_body(ts[0], ctx, depth, bindings, out);
        return;
    }
    let pad = indent(depth);
    // Walk in Rails declaration order — first match wins. Each
    // constrained route becomes an `if`/`elsif <regex>.match?` guard.
    // The first UNCONSTRAINED route matches any segment, so it is the
    // `else`; any route after it (or a second unconstrained one) can
    // never be reached and is flagged rather than silently dropped.
    // Order-preserving on purpose: an app that declares the catch-all
    // before the constrained route has the constrained route dead in
    // Rails too, and the converted output must reproduce that.
    let mut guards = 0usize;
    let mut caught_all = false;
    for &t in ts {
        if caught_all {
            out.push_str(&format!(
                "{}# ROUNDHOUSE-TODO: unreachable route {}#{} (an earlier route \
                 with no constraint already matches every value)\n",
                indent(depth + 1),
                t.controller.0,
                t.action
            ));
            continue;
        }
        match constraint_guard(t, bindings) {
            Some(guard) => {
                let kw = if guards == 0 { "if" } else { "elsif" };
                out.push_str(&format!("{pad}{kw} {guard}\n"));
                emit_terminal_body(t, ctx, depth + 1, bindings, out);
                guards += 1;
            }
            None if guards == 0 => {
                // Catch-all with no preceding guard: no distinguishing
                // constraint (genuine duplicate, or constraint on an
                // unbound param). Emit its body bare as the whole block.
                emit_terminal_body(t, ctx, depth, bindings, out);
                caught_all = true;
            }
            None => {
                out.push_str(&format!("{pad}else\n"));
                emit_terminal_body(t, ctx, depth + 1, bindings, out);
                caught_all = true;
            }
        }
    }
    // Close the if/elsif[/else] chain. A bare catch-all body (guards ==
    // 0) opened no `if`, so it needs no `end`. An all-constrained chain
    // (no catch-all) ends here; a non-matching segment falls through to
    // 404, exactly as in Rails.
    if guards > 0 {
        out.push_str(&format!("{pad}end\n"));
    }
}

/// Build the Ruby guard for a route's non-digit `constraints:` —
/// `/\A<rx>\z/.match?(<var>)`, `&&`-joined across multiple constrained
/// params. Rails anchors constraints to the whole segment, so the
/// emitted regex is `\A…\z`-anchored. Returns None when the route has
/// no such constraint (or its param isn't bound here), so the caller
/// treats it as the fallback (`else`) branch.
fn constraint_guard(t: &FlatRoute, bindings: &[(String, String)]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (param, rx) in &t.constraints {
        let var = bindings.iter().find(|(p, _)| p == param).map(|(_, v)| v.clone())?;
        parts.push(format!("{}.match?({var})", anchored_regex_literal(rx)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" && "))
    }
}

/// Wrap a Rails constraint regex source in a Ruby `/\A…\z/` literal,
/// adding the implicit full-segment anchors Rails applies unless the
/// source already carries them.
fn anchored_regex_literal(rx: &str) -> String {
    let start = if rx.starts_with("\\A") || rx.starts_with('^') { "" } else { "\\A" };
    let end = if rx.ends_with("\\z") || rx.ends_with('$') { "" } else { "\\z" };
    format!("/{start}{rx}{end}/")
}

/// Roda matcher class for a dynamic node: id-shaped params get the
/// typed `Integer` matcher; anything else (`:username`, `:tag`) gets
/// `String` — typing those Integer would silently unroute every real
/// request.
fn matcher_for(names: &[String]) -> &'static str {
    if names.iter().all(|n| n == "id" || n.ends_with("_id")) {
        "Integer"
    } else {
        "String"
    }
}

/// Block variable for a dynamic node. Interior nodes bind `id` when any
/// route calls it that (mixed `:id` + `:article_id` naming collapses to
/// the one shared binding, like the exemplar). A leaf whose only source
/// name is the generic `:id` takes its parent segment's singular
/// (`comments` → `comment_id`) — the flat table's `:id` was scoped by
/// the Rails path; re-nested, the qualified name reads better.
fn block_var(names: &[String], is_leaf: bool, parent_seg: Option<&str>) -> String {
    if is_leaf {
        if let Some(name) = names.first() {
            if name != "id" {
                return sanitize_var(name);
            }
        }
        if let Some(seg) = parent_seg {
            return format!("{}_id", naming::singularize(seg));
        }
        return "id".to_string();
    }
    if names.iter().any(|n| n == "id") {
        return "id".to_string();
    }
    names.first().map(|n| sanitize_var(n)).unwrap_or_else(|| "id".to_string())
}

/// A path param can carry a literal suffix inside its segment
/// (`/avatars/:username_size.png` — Rails reads the param as
/// `username_size` and matches `.png` separately), and the raw name is
/// not a valid Ruby local. Keep the identifier-safe head; the dropped
/// suffix is flagged at the route site.
fn sanitize_var(name: &str) -> String {
    let head: String =
        name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    if head.is_empty() {
        "id".to_string()
    } else {
        head
    }
}

/// Interior-node loads: every recognized find-by-param filter whose
/// param binds at this node and which covers every subtree action of
/// its controller becomes `next unless @ivar = Model[var]` — the
/// idiomatic Roda shared-interior-state + interior-abort form (the
/// block returns nil, the route is unhandled, not_found renders 404 —
/// matching Rails' rescued RecordNotFound).
fn emit_interior_loads(
    subtree: &Node,
    ctx: &EmitCtx,
    var: &str,
    names: &[String],
    depth: usize,
    out: &mut String,
) {
    let mut subtree_routes: Vec<&FlatRoute> = Vec::new();
    collect_subtree_routes(subtree, &mut subtree_routes);

    let mut emitted: Vec<(String, String)> = Vec::new();
    for load in ctx.loads {
        if !names.iter().any(|n| *n == load.param) {
            continue;
        }
        let controller_routes: Vec<&&FlatRoute> = subtree_routes
            .iter()
            .filter(|r| r.controller.0.as_str() == load.controller)
            .collect();
        if controller_routes.is_empty() {
            continue;
        }
        let all_covered = controller_routes
            .iter()
            .all(|r| filter_covers(load, r.action.as_str()));
        if !all_covered {
            out.push_str(&format!(
                "{}# ROUNDHOUSE-TODO: before_action load of @{} covers only some \
                 actions below; converted per-action coverage is pending\n",
                indent(depth),
                load.ivar
            ));
            continue;
        }
        let key = (load.ivar.clone(), load.model.clone());
        if emitted.contains(&key) {
            continue;
        }
        out.push_str(&format!(
            "{}next unless @{} = {}[{}]\n",
            indent(depth),
            load.ivar,
            load.model,
            var
        ));
        emitted.push(key);
    }
    let _ = ctx.app;
}

fn collect_subtree_routes<'a>(node: &'a Node, out: &mut Vec<&'a FlatRoute>) {
    out.extend(node.terminals.iter());
    for (_, c) in &node.stat {
        collect_subtree_routes(c, out);
    }
    if let Some((_, c)) = &node.dynamic {
        collect_subtree_routes(c, out);
    }
}

/// The static GET path this root route duplicates, if any (`root
/// "articles#index"` + `resources :articles` → `/articles`).
fn canonical_static_path(routes: &[FlatRoute], root: &FlatRoute) -> Option<String> {
    routes
        .iter()
        .find(|r| {
            r.path != "/"
                && r.method == HttpMethod::Get
                && r.controller == root.controller
                && r.action == root.action
                && !r.path.contains(':')
        })
        .map(|r| r.path.clone())
}

// ── Terminal bodies (Action → handler block) ────────────────────────

fn emit_terminal_body(
    route: &FlatRoute,
    ctx: &EmitCtx,
    depth: usize,
    bindings: &[(String, String)],
    out: &mut String,
) {
    let pad = indent(depth);
    let Some(controller) = ctx
        .app
        .controllers
        .iter()
        .find(|c| c.name == route.controller)
    else {
        out.push_str(&format!(
            "{pad}# ROUNDHOUSE-TODO: controller {} not found in ingest\n{pad}r.halt [501, {{}}, [\"ROUNDHOUSE-TODO: not converted yet\"]]\n",
            route.controller.0
        ));
        return;
    };
    let Some(action) = find_action(controller, route.action.as_str()) else {
        out.push_str(&format!(
            "{pad}# ROUNDHOUSE-TODO: action {}#{} not found in ingest\n{pad}r.halt [501, {{}}, [\"ROUNDHOUSE-TODO: not converted yet\"]]\n",
            route.controller.0, route.action
        ));
        return;
    };

    let body = convert_body(route, controller, action, ctx, bindings);

    // A partially converted body is a DRAFT, not a working route:
    // running it would serve a plausible-looking wrong result, because
    // the statements that didn't convert are exactly the ones whose
    // absence is invisible in the response (a dropped authorization
    // check, an ivar the view reads unset). So the route stays honest
    // by default — halt first, with the draft readable below it, and
    // deleting the one halt line arms the body once the comments are
    // filled in.
    if !body.complete() {
        out.push_str(&format!(
            "{pad}# ROUNDHOUSE-TODO: {}/{} statements converted ({}#{}); the rest are\n",
            body.converted, body.total, route.controller.0, route.action
        ));
        out.push_str(&format!(
            "{pad}# commented inline below. Delete this halt once the body is complete.\n"
        ));
        out.push_str(&format!(
            "{pad}r.halt [501, {{}}, [\"ROUNDHOUSE-TODO: partially converted\"]]\n"
        ));
    }
    for l in body.lines {
        if l.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{pad}{l}\n"));
        }
    }
}

/// View directory for a controller: `ArticlesController` → `articles`.
fn view_dir(controller: &Controller) -> String {
    let snake = naming::snake_case(controller.name.0.as_str());
    snake.strip_suffix("_controller").unwrap_or(&snake).to_string()
}

/// Convert a whole action body. The `respond_to` wrapper is unwrapped
/// first (the html branch survives; json/turbo_stream branches drop —
/// the Roda exemplar is an html app, and the format asymmetry is part
/// of the honest conversion ledger), then each statement converts
/// through `convert_stmt`. Any statement outside the recognized set
/// fails the WHOLE body over to the commented-original path — partial
/// bodies would be behaviorally wrong, not machine-shaped.
fn convert_body(
    route: &FlatRoute,
    controller: &Controller,
    action: &Action,
    ctx: &EmitCtx,
    bindings: &[(String, String)],
) -> Converted {
    let body = unwrap_respond_to(&action.body);
    let cx = BodyCx { ctx, controller, bindings };
    let mut out = convert_stmts(&statements_owned(&body), &cx);
    if route.method == HttpMethod::Get {
        match &action.renders {
            RenderTarget::Template { name, .. } => {
                out.lines.push(format!("view \"{}/{}\"", view_dir(controller), name));
            }
            RenderTarget::Inferred => {
                out.lines.push(format!("view \"{}/{}\"", view_dir(controller), action.name));
            }
            // The response itself didn't convert. Count it like a
            // statement so the body reads as partial rather than as a
            // complete conversion that happens to render nothing.
            other => {
                out.lines
                    .push(format!("# ROUNDHOUSE-TODO: unconverted render target: {other:?}"));
                out.total += 1;
            }
        }
    }
    out
}

/// Statement-level conversion outcome: the emitted Roda/Sequel lines
/// plus how many of the body's statements translated. `converted ==
/// total` holds exactly when no `ROUNDHOUSE-TODO` comment was emitted
/// anywhere in the body, branches included — the invariant the
/// partial-body halt keys on.
#[derive(Default)]
struct Converted {
    lines: Vec<String>,
    converted: usize,
    total: usize,
}

impl Converted {
    /// One statement that translated in full.
    fn one(lines: Vec<String>) -> Self {
        Self { lines, converted: 1, total: 1 }
    }

    fn merge(&mut self, other: Converted) {
        self.lines.extend(other.lines);
        self.converted += other.converted;
        self.total += other.total;
    }

    fn complete(&self) -> bool {
        self.converted == self.total
    }
}

/// Per-body conversion context: the controller (for `<model>_params`
/// strong-parameter resolution) and the path-param → route-block-var
/// bindings accumulated down the trie (innermost last).
struct BodyCx<'a> {
    ctx: &'a EmitCtx<'a>,
    controller: &'a Controller,
    bindings: &'a [(String, String)],
}

impl BodyCx<'_> {
    /// Innermost binding for a path-param name (`:id` in a nested route
    /// resolves to the deepest Integer block var, e.g. `comment_id`).
    fn var_for(&self, param: &str) -> Option<&str> {
        self.bindings.iter().rev().find(|(p, _)| p == param).map(|(_, v)| v.as_str())
    }
}

fn statements(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}

fn statements_owned(body: &Expr) -> Vec<Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![body.clone()],
    }
}

/// Statement by statement: anything outside the recognized vocabulary
/// becomes a comment carrying its Rails original, and the walk
/// continues. The whole-body gate this replaces discarded every
/// statement around a single unrecognized one — on lobsters that hid
/// convertible code behind one unknown line in 158 actions (#67).
fn convert_stmts(stmts: &[Expr], cx: &BodyCx) -> Converted {
    let mut out = Converted::default();
    for s in stmts {
        // Nested sequences (respond_to splice residue) flatten, so
        // their statements are counted and commented individually
        // rather than as one opaque block.
        if let ExprNode::Seq { exprs } = &*s.node {
            let inner = convert_stmts(exprs, cx);
            out.merge(inner);
            continue;
        }
        match convert_stmt(s, cx) {
            Some(c) => out.merge(c),
            None => {
                out.lines.extend(unconverted_comment(s));
                out.total += 1;
            }
        }
    }
    out
}

/// The Rails original of a statement that didn't convert, as comment
/// lines. Deliberately unclassified: `ROUNDHOUSE-TODO` claims only
/// that roundhouse stopped here, which is the one thing we know —
/// whether the construct is ours to implement, the app's to change,
/// or has no Sequel spelling at all is a separate judgment.
fn unconverted_comment(stmt: &Expr) -> Vec<String> {
    let src = emit_expr(stmt);
    let mut lines: Vec<String> = src
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let line = if i == 0 {
                format!("# ROUNDHOUSE-TODO: {l}")
            } else {
                format!("#   {l}")
            };
            line.trim_end().to_string()
        })
        .collect();
    if lines.is_empty() {
        lines.push("# ROUNDHOUSE-TODO: (empty statement)".to_string());
    }
    lines
}

/// Rails / ActiveRecord method vocabulary that must never carry over
/// verbatim. Two families: persistence and query methods whose Sequel
/// spelling differs (`save!` → `save(raise_on_failure: true)`,
/// `find_each` → `paged_each`), and ActiveSupport surface that has no
/// Sequel meaning at all (`blank?`, `1.week.ago`).
///
/// Enumerable-shaped names (`select`, `count`, `first`, `sum`) are
/// listed even though plain Ruby uses them too. That costs coverage on
/// genuine Array receivers, and it is the right trade: emitting a
/// verbatim `.select` that turns out to be a relation is exactly the
/// false positive that makes converted code look finished and be
/// wrong.
const RAILS_VOCABULARY: &[&str] = &[
    // persistence
    "save", "save!", "update", "update!", "update_attribute", "update_attributes",
    "update_column", "update_columns", "touch", "reload", "becomes", "toggle!",
    "increment!", "decrement!", "destroy_all", "delete_all", "record_timestamps=",
    "valid?", "invalid?", "errors", "full_messages", "transaction",
    // model-object surface AR overrides and Sequel does not: `dup`
    // resets the primary key in Rails and copies it in Sequel, which
    // is a silent divergence in exactly the "looks converted" way.
    "dup", "clone", "attributes", "assign_attributes", "new_record?", "persisted?",
    "destroyed?", "changed?", "changes", "previous_changes",
    // query
    "where", "find", "find_by", "find_by!", "find_each", "find_in_batches", "order",
    "includes", "joins", "left_joins", "preload", "eager_load", "eager", "pluck",
    "select", "distinct", "group", "having", "limit", "offset", "count", "sum",
    "average", "minimum", "maximum", "exists?", "first", "last", "all", "none",
    "unscoped", "references", "merge", "arel_table", "to_sql",
    // Rails / ActiveSupport idioms
    "blank?", "present?", "presence", "try", "try!", "to_param", "human_attribute_name",
    "params", "session", "flash", "cookies", "request", "response", "headers",
    "ago", "from_now", "beginning_of_day", "end_of_day", "days", "hours", "minutes",
    "seconds", "weeks", "months", "years", "day", "hour", "minute", "second", "week",
    "month", "year",
];

fn rails_vocabulary(method: &str) -> bool {
    RAILS_VOCABULARY.contains(&method) || method.ends_with("_path") || method.ends_with("_url")
}

/// Is this statement plain Ruby — no Rails or ActiveRecord vocabulary
/// anywhere in its tree — and therefore safe to carry over verbatim,
/// the way model method bodies already do at the `ModelBodyItem`
/// level? `@title = "Login"` is not a framework construct, and
/// commenting it out taught nobody anything.
///
/// The walk is an ALLOWLIST, deliberately. Emitting a verbatim
/// statement that turns out to be ActiveRecord is precisely the
/// false positive Jeremy flagged on lobsters — `find_each`, `save!`
/// and `record_timestamps=` reading as valid Sequel while being
/// nothing of the kind. When a node's disposition is unclear the
/// statement is commented, which is honest, rather than emitted,
/// which may be wrong.
fn plain_ruby(e: &Expr) -> bool {
    match &*e.node {
        // Values and pure control flow. Children are still walked below.
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::SelfRef
        | ExprNode::Hash { .. }
        | ExprNode::Array { .. }
        | ExprNode::StringInterp { .. }
        | ExprNode::Range { .. }
        | ExprNode::Splat { .. }
        | ExprNode::BoolOp { .. }
        | ExprNode::If { .. }
        | ExprNode::Case { .. }
        | ExprNode::Seq { .. }
        | ExprNode::Return { .. }
        | ExprNode::Next { .. }
        | ExprNode::Break { .. }
        | ExprNode::While { .. }
        | ExprNode::Raise { .. }
        | ExprNode::RescueModifier { .. }
        | ExprNode::BeginRescue { .. }
        | ExprNode::MultiAssign { .. }
        | ExprNode::Let { .. }
        | ExprNode::Cast { .. } => {}

        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
            if !plain_lvalue(target) {
                return false;
            }
        }

        // A call carries over only with an explicit receiver that is
        // itself plain — a bare call would target a controller instance
        // the Roda app doesn't have — and a method outside the Rails
        // vocabulary. A `Const` receiver is a model or Rails namespace
        // needing a real Sequel spelling. Blocks are refused wholesale
        // in this pass: `each`/`map` on an ivar may be an Array or a
        // relation, and those don't survive the same translation.
        ExprNode::Send { recv, method, block, .. } => {
            let Some(recv) = recv else { return false };
            if block.is_some() || rails_vocabulary(method.as_str()) {
                return false;
            }
            if matches!(&*recv.node, ExprNode::Const { .. }) {
                return false;
            }
        }

        // Bare `Const` reads, `Apply`, `Lambda`, `Yield`, `Super`,
        // `Retry`, `Redo`: either a class reference that needs a Sequel
        // spelling, or a shape this pass hasn't reasoned about.
        _ => return false,
    }

    let mut ok = true;
    e.node.for_each_child(&mut |c| {
        if ok && !plain_ruby(c) {
            ok = false;
        }
    });
    ok
}

/// Assignment targets. `@story.is_deleted = true` and
/// `@story.editor = @user` carry over because Sequel generates column
/// and association setters with the same spelling; `flash[:error] = …`
/// does not, because its receiver isn't plain.
fn plain_lvalue(t: &crate::expr::LValue) -> bool {
    use crate::expr::LValue;
    match t {
        LValue::Var { .. } | LValue::Ivar { .. } => true,
        LValue::Attr { recv, name } => !rails_vocabulary(name.as_str()) && plain_ruby(recv),
        LValue::Index { recv, index } => plain_ruby(recv) && plain_ruby(index),
        LValue::Const { .. } => false,
    }
}

/// One statement → Roda/Sequel source lines, or None (not in the
/// recognized conversion set). The Rails-vocabulary translations run
/// first; anything they don't claim falls through to plain Ruby,
/// which carries over verbatim.
fn convert_stmt(stmt: &Expr, cx: &BodyCx) -> Option<Converted> {
    if let Some(c) = convert_framework_stmt(stmt, cx) {
        return Some(c);
    }
    if plain_ruby(stmt) {
        return Some(Converted::one(emit_expr(stmt).lines().map(str::to_string).collect()));
    }
    None
}

/// Statements whose Rails vocabulary needs a Sequel/Roda spelling.
fn convert_framework_stmt(stmt: &Expr, cx: &BodyCx) -> Option<Converted> {
    match &*stmt.node {
        ExprNode::Assign { target, value } => {
            let crate::expr::LValue::Ivar { name } = target else { return None };
            convert_ivar_assign(name.as_str(), value, cx).map(Converted::one)
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            convert_if(cond, then_branch, else_branch, cx)
        }
        ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "redirect_to" => {
            convert_redirect(args, cx).map(Converted::one)
        }
        ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "render" => {
            convert_render(args, cx).map(Converted::one)
        }
        // `@article.destroy!` → `@article.destroy` (Sequel #destroy
        // raises on hook failure already; the bang distinction is
        // Rails-side validation semantics the blog doesn't exercise).
        ExprNode::Send { recv: Some(r), method, args, .. }
            if (method.as_str() == "destroy" || method.as_str() == "destroy!")
                && args.is_empty() =>
        {
            let ExprNode::Ivar { name } = &*r.node else { return None };
            Some(Converted::one(vec![format!("@{name}.destroy")]))
        }
        _ => None,
    }
}

/// `@x = <value>` shapes: strong-params construction, association
/// build, association find-by-param, and the Day-1 query chains.
fn convert_ivar_assign(name: &str, value: &Expr, cx: &BodyCx) -> Option<Vec<String>> {
    // `@article = Article.new(article_params)` →
    // `@article = Article.new.set_fields(r.params["article"], %w[title body])`
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*value.node {
        if method.as_str() == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*recv.node {
                if is_model(cx.ctx, path) {
                    let (key, fields) = params_fields(cx, &args[0])?;
                    return Some(vec![format!(
                        "@{name} = {}.new.set_fields(r.params[\"{key}\"], {})",
                        path.last().unwrap(),
                        fields_list(&fields)
                    )]);
                }
            }
        }
        // `@comment = @article.comments.build(comment_params)` →
        //   `@comment = Comment.new.set_fields(r.params["comment"], %w[…])`
        //   `@comment.article = @article`
        // (NOT `add_comment` — Rails `build` doesn't save; the explicit
        // association assignment + later `save` matches, and is what the
        // exemplar does.)
        if method.as_str() == "build" && args.len() == 1 {
            if let ExprNode::Send { recv: Some(owner), method: assoc, args: aargs, .. } =
                &*recv.node
            {
                if aargs.is_empty() {
                    if let ExprNode::Ivar { name: owner_ivar } = &*owner.node {
                        let target = assoc_target_model(cx.ctx, assoc.as_str())?;
                        let belongs = belongs_to_name(cx.ctx, &target, owner_ivar.as_str())?;
                        let (key, fields) = params_fields(cx, &args[0])?;
                        return Some(vec![
                            format!(
                                "@{name} = {target}.new.set_fields(r.params[\"{key}\"], {})",
                                fields_list(&fields)
                            ),
                            format!("@{name}.{belongs} = @{owner_ivar}"),
                        ]);
                    }
                }
            }
        }
        // `@comment = @article.comments.find(params.expect(:id))` →
        // `next unless @comment = @article.comments_dataset.with_pk(comment_id)`
        // Rails `find` raises RecordNotFound (→ rescued 404); with_pk
        // returns nil and `next` abandons the route → not_found 404.
        if method.as_str() == "find" && args.len() == 1 {
            if let ExprNode::Send { recv: Some(owner), method: assoc, args: aargs, .. } =
                &*recv.node
            {
                if aargs.is_empty() {
                    if let ExprNode::Ivar { name: owner_ivar } = &*owner.node {
                        let key = first_symbol_in(&args[0])?;
                        let var = cx.var_for(&key)?;
                        return Some(vec![format!(
                            "next unless @{name} = @{owner_ivar}.{assoc}_dataset.with_pk({var})"
                        )]);
                    }
                }
            }
        }
    }
    // Day-1 query chains (`Article.includes(...).order(...)`, `Article.new`).
    let converted = sequelize_query(value, cx.ctx)?;
    Some(vec![format!("@{name} = {converted}")])
}

/// `if @x.save` / `if @x.update(<params>)` conditionals. `update`
/// splits into `set_fields` + `if save` (Sequel's #update saves
/// immediately and raises-or-returns-self — the two-step form keeps the
/// Rails branch semantics with validation-once, like the exemplar).
fn convert_if(
    cond: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    cx: &BodyCx,
) -> Option<Converted> {
    let mut lines: Vec<String> = Vec::new();
    let cond_str = match &*cond.node {
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "save" && args.is_empty() =>
        {
            let ExprNode::Ivar { name } = &*r.node else { return None };
            format!("if @{name}.save")
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "update" && args.len() == 1 =>
        {
            let ExprNode::Ivar { name } = &*r.node else { return None };
            let (key, fields) = params_fields(cx, &args[0])?;
            lines.push(format!(
                "@{name}.set_fields(r.params[\"{key}\"], {})",
                fields_list(&fields)
            ));
            format!("if @{name}.save")
        }
        // A plain-Ruby condition keeps the `if` frame, so the branches
        // convert statement by statement instead of the whole
        // conditional collapsing into one comment. Single-line only —
        // a multi-line condition would need its own indentation pass.
        _ if plain_ruby(cond) && !emit_expr(cond).contains('\n') => {
            format!("if {}", emit_expr(cond))
        }
        _ => return None,
    };
    lines.push(cond_str);

    // The `if` itself counts as one converted statement; each branch
    // contributes its own tally, so an unrecognized statement nested in
    // a branch still marks the enclosing body partial.
    let mut converted = 1;
    let mut total = 1;

    let then_c = convert_stmts(&statements_owned(then_branch), cx);
    lines.extend(indented(&then_c.lines));
    converted += then_c.converted;
    total += then_c.total;

    let else_stmts = statements_owned(else_branch);
    let else_empty = else_stmts.len() == 1
        && matches!(&*else_stmts[0].node, ExprNode::Lit { value: Literal::Nil })
        || else_stmts.is_empty();
    if !else_empty {
        lines.push("else".to_string());
        let else_c = convert_stmts(&else_stmts, cx);
        lines.extend(indented(&else_c.lines));
        converted += else_c.converted;
        total += else_c.total;
    }
    lines.push("end".to_string());
    Some(Converted { lines, converted, total })
}

/// Branch lines, indented one level (blank lines stay blank).
fn indented(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| if l.is_empty() { String::new() } else { format!("  {l}") })
        .collect()
}

/// `redirect_to <target>, notice: "…"` → flash assignment(s) + a
/// literal-path `r.redirect`. `status: :see_other` drops — Roda's
/// redirect issues 302 and browsers treat both identically for
/// post-form navigation (exemplar parity).
fn convert_redirect(args: &[Expr], cx: &BodyCx) -> Option<Vec<String>> {
    let target = args.first()?;
    let mut lines = Vec::new();
    for arg in &args[1..] {
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                    return None;
                };
                match key.as_str() {
                    "notice" | "alert" => {
                        lines.push(format!("flash[\"{key}\"] = {}", emit_expr(v)));
                    }
                    "status" => {}
                    _ => return None,
                }
            }
        } else {
            return None;
        }
    }
    lines.push(format!("r.redirect {}", redirect_path(target, cx)?));
    Some(lines)
}

/// The redirect target as a Ruby path expression (with surrounding
/// quotes). `@article` → `"/articles/#{@article.id}"` via the
/// resource's named show route; `articles_path` etc. resolve through
/// the flat table's helper names.
fn redirect_path(target: &Expr, cx: &BodyCx) -> Option<String> {
    match &*target.node {
        ExprNode::Ivar { name } => {
            let route = named_route(cx.ctx, name.as_str())?;
            Some(format!("\"{}\"", substitute_params(&route.path, &format!("#{{@{name}.id}}"))))
        }
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str().ends_with("_path") =>
        {
            let as_name = method.as_str().strip_suffix("_path").unwrap();
            let route = named_route(cx.ctx, as_name)?;
            if route.path_params.is_empty() && args.is_empty() {
                return Some(format!("\"{}\"", route.path));
            }
            if route.path_params.len() == 1 && args.len() == 1 {
                if let ExprNode::Ivar { name } = &*args[0].node {
                    return Some(format!(
                        "\"{}\"",
                        substitute_params(&route.path, &format!("#{{@{name}.id}}"))
                    ));
                }
            }
            None
        }
        _ => None,
    }
}

/// The named flat route for a helper stem (`article` → GET show route
/// whose as_name is `article`).
fn named_route<'a>(ctx: &'a EmitCtx, as_name: &str) -> Option<&'a FlatRoute> {
    ctx.routes.iter().find(|r| r.named && r.as_name == as_name && r.method == HttpMethod::Get)
        .or_else(|| ctx.routes.iter().find(|r| r.named && r.as_name == as_name))
}

/// Replace every `:param` segment with the given interpolation.
fn substitute_params(path: &str, interp: &str) -> String {
    path.split('/')
        .map(|seg| if seg.starts_with(':') { interp } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

/// `render :new, status: :unprocessable_content` → `view "articles/new"`.
/// The status drops (exemplar parity: the Roda exemplar re-renders the
/// form at 200 — carried in the conversion ledger).
fn convert_render(args: &[Expr], cx: &BodyCx) -> Option<Vec<String>> {
    let first = args.first()?;
    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*first.node else {
        return None;
    };
    Some(vec![format!("view \"{}/{name}\"", view_dir(cx.controller))])
}

/// Resolve a `<model>_params` strong-parameter method reference to its
/// (params key, permitted fields). Handles both modern
/// `params.expect(article: [:title, :body])` and classic
/// `params.require(:article).permit(:title, :body)`.
fn params_fields(cx: &BodyCx, call: &Expr) -> Option<(String, Vec<String>)> {
    let ExprNode::Send { recv, method, args, .. } = &*call.node else { return None };
    let self_recv = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    if !self_recv || !args.is_empty() {
        return None;
    }
    let action = find_action(cx.controller, method.as_str())?;
    let body = single_statement(&action.body)?;
    strong_params_shape(body)
}

fn strong_params_shape(e: &Expr) -> Option<(String, Vec<String>)> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else { return None };
    match method.as_str() {
        // params.expect(article: [:title, :body])
        "expect" => {
            let ExprNode::Hash { entries, .. } = &*args.first()?.node else { return None };
            let (k, v) = entries.first()?;
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                return None;
            };
            let ExprNode::Array { elements, .. } = &*v.node else { return None };
            let fields = elements
                .iter()
                .map(|el| match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.to_string()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            let _ = recv;
            Some((key.to_string(), fields))
        }
        // params.require(:article).permit(:title, :body)
        "permit" => {
            let ExprNode::Send { method: req, args: rargs, .. } = &*recv.node else {
                return None;
            };
            if req.as_str() != "require" {
                return None;
            }
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*rargs.first()?.node
            else {
                return None;
            };
            let fields = args
                .iter()
                .map(|el| match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.to_string()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some((key.to_string(), fields))
        }
        _ => None,
    }
}

fn fields_list(fields: &[String]) -> String {
    format!("%w[{}]", fields.join(" "))
}

/// `comments` (association name) → the `Comment` model class name.
fn assoc_target_model(ctx: &EmitCtx, assoc: &str) -> Option<String> {
    let singular = naming::singularize(assoc);
    ctx.app
        .models
        .iter()
        .find(|m| naming::snake_case(m.name.0.as_str()) == singular)
        .map(|m| m.name.0.to_string())
}

/// The belongs-to/many_to_one association name on `model_name` that
/// points back at the owner ivar (`@article` → `:article` on Comment).
fn belongs_to_name(ctx: &EmitCtx, model_name: &str, owner_ivar: &str) -> Option<String> {
    let model = ctx.app.models.iter().find(|m| m.name.0.as_str() == model_name)?;
    model.associations().find_map(|a| match a {
        Association::BelongsTo { name, .. } if name.as_str() == owner_ivar => {
            Some(name.to_string())
        }
        _ => None,
    })
}

/// Strip `respond_to do |format| … end` wrappers: the `format.html`
/// branch bodies splice in place; other formats drop. Bodies without a
/// respond_to pass through unchanged.
fn unwrap_respond_to(body: &Expr) -> Expr {
    let mut out = body.clone();
    unwrap_respond_to_mut(&mut out);
    out
}

fn unwrap_respond_to_mut(e: &mut Expr) {
    // Top-down, checking each statement BEFORE recursing into it —
    // children-first would replace the respond_to at child level and
    // leave the parent Seq unable to splice multi-statement branches.
    if let ExprNode::Seq { exprs } = &mut *e.node {
        let mut new_exprs: Vec<Expr> = Vec::new();
        for mut ex in exprs.drain(..) {
            match respond_to_html_body(&ex) {
                Some(html) => {
                    for mut h in statements_owned(&html) {
                        unwrap_respond_to_mut(&mut h);
                        new_exprs.push(h);
                    }
                }
                None => {
                    unwrap_respond_to_mut(&mut ex);
                    new_exprs.push(ex);
                }
            }
        }
        *exprs = new_exprs;
        return;
    }
    if let Some(html) = respond_to_html_body(&e.clone()) {
        *e = html;
        unwrap_respond_to_mut(e);
        return;
    }
    e.node.for_each_child_mut(&mut unwrap_respond_to_mut);
}

/// If `e` is `respond_to do |format| … end`, return the spliced html
/// branch (format-call selection applied through its whole subtree).
fn respond_to_html_body(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "respond_to" || !args.is_empty() {
        return None;
    }
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
    let format_var = params.first()?.as_str().to_string();
    let mut out = body.clone();
    select_html_format(&mut out, &format_var);
    Some(out)
}

/// Rewrite a respond_to block body: `format.html { X }` → X,
/// `format.json { … }` (any other format) → removed. Statements are
/// checked BEFORE recursion (same reasoning as unwrap_respond_to_mut).
fn select_html_format(e: &mut Expr, format_var: &str) {
    if let ExprNode::Seq { exprs } = &mut *e.node {
        let mut new_exprs: Vec<Expr> = Vec::new();
        for mut ex in exprs.drain(..) {
            match format_call_body(&ex, format_var) {
                Some(Some(html)) => {
                    for mut h in statements_owned(&html) {
                        select_html_format(&mut h, format_var);
                        new_exprs.push(h);
                    }
                }
                Some(None) => {} // non-html format: dropped
                None => {
                    select_html_format(&mut ex, format_var);
                    new_exprs.push(ex);
                }
            }
        }
        *exprs = new_exprs;
        return;
    }
    // A bare format call in branch position (If then/else that isn't a
    // Seq) replaces with its body — or with an empty Seq when dropped.
    if let Some(repl) = format_call_body(&e.clone(), format_var) {
        match repl {
            Some(html) => {
                *e = html;
                select_html_format(e, format_var);
            }
            None => {
                *e = Expr::new(crate::span::Span::synthetic(), ExprNode::Seq { exprs: vec![] })
            }
        }
        return;
    }
    e.node.for_each_child_mut(&mut |c| select_html_format(c, format_var));
}

/// `format.html { X }` → Some(Some(X)); `format.<other> { … }` →
/// Some(None); anything else → None.
fn format_call_body(e: &Expr, format_var: &str) -> Option<Option<Expr>> {
    let ExprNode::Send { recv: Some(recv), method, block, .. } = &*e.node else { return None };
    let ExprNode::Var { name, .. } = &*recv.node else { return None };
    if name.as_str() != format_var {
        return None;
    }
    if method.as_str() == "html" {
        if let Some(b) = block {
            if let ExprNode::Lambda { body, .. } = &*b.node {
                return Some(Some(body.clone()));
            }
        }
        return Some(None);
    }
    Some(None)
}

/// Rails AR query chain → Sequel dataset chain, as source text.
///
///   Article.includes(:comments).order(created_at: :desc)
///     → Article.eager(:comments).reverse(:created_at).all
///   Article.new → Article.new
///   Article.all → Article.all
///
/// Chains rooted at a model constant only; a non-query value returns
/// None so the caller falls back to the commented-original path.
fn sequelize_query(e: &Expr, ctx: &EmitCtx) -> Option<String> {
    // Bare `Model.new` (and `Model.new` with no args) carries over.
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node {
        if method.as_str() == "new" && args.is_empty() {
            if let ExprNode::Const { path } = &*recv.node {
                if is_model(ctx, path) {
                    return Some(format!("{}.new", path.last().unwrap()));
                }
            }
        }
    }
    let (root, calls) = unroll_chain(e)?;
    let ExprNode::Const { path } = &*root.node else { return None };
    if !is_model(ctx, path) {
        return None;
    }
    let mut out = path.last().unwrap().to_string();
    let mut relation = false;
    for (method, args) in &calls {
        match method.as_str() {
            "includes" if args.len() == 1 => {
                out.push_str(&format!(".eager({})", emit_expr(&args[0])));
                relation = true;
            }
            "order" if args.len() == 1 => {
                let arg = order_arg_to_sequel(&args[0])?;
                if let Some(col) = arg.strip_prefix("Sequel.desc(").and_then(|s| s.strip_suffix(')'))
                {
                    out.push_str(&format!(".reverse({col})"));
                } else {
                    out.push_str(&format!(".order({arg})"));
                }
                relation = true;
            }
            "all" if args.is_empty() => {
                out.push_str(".all");
                relation = false;
            }
            _ => return None,
        }
    }
    if relation {
        // Materialize once, like the exemplar — the view iterates an
        // Array, not a live dataset.
        out.push_str(".all");
    }
    Some(out)
}

/// `A.b(x).c(y)` → (A, [(b, [x]), (c, [y])]). Blocks bail (None).
fn unroll_chain(e: &Expr) -> Option<(&Expr, Vec<(String, Vec<Expr>)>)> {
    let mut calls: Vec<(String, Vec<Expr>)> = Vec::new();
    let mut cur = e;
    loop {
        match &*cur.node {
            ExprNode::Send { recv: Some(recv), method, args, block: None, .. } => {
                calls.push((method.to_string(), args.clone()));
                cur = recv;
            }
            ExprNode::Const { .. } => {
                calls.reverse();
                return Some((cur, calls));
            }
            _ => return None,
        }
    }
}

fn is_model(ctx: &EmitCtx, path: &[crate::ident::Symbol]) -> bool {
    let Some(last) = path.last() else { return false };
    ctx.app.models.iter().any(|m| m.name.0.as_str() == last.as_str())
}

// ── Views (Day-1 placeholders) ──────────────────────────────────────

/// Views: the synthesized layout + not_found, plus the translated
/// Rails ERB (see `views::translate_views`).
fn emit_views(app: &App) -> Vec<EmittedFile> {
    let routes = flatten_routes(app);
    let mut out = vec![
        file("views/layout.erb", LAYOUT_ERB),
        file("views/not_found.erb", "<h1>404 Not Found</h1>\n"),
    ];
    out.extend(views::translate_views(app, &routes));
    out
}

const LAYOUT_ERB: &str = r#"<!DOCTYPE html>
<html>
  <head>
    <title>Blog</title>
  </head>
  <body>
    <% if flash["notice"] %><p class="notice"><%= flash["notice"] %></p><% end %>
    <% if flash["alert"] %><p class="alert"><%= flash["alert"] %></p><% end %>
    <%== yield %>
  </body>
</html>
"#;

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Symbol;
    use crate::ClassId;

    fn flat(method: HttpMethod, path: &str, controller: &str, action: &str) -> FlatRoute {
        let params: Vec<String> = path
            .split('/')
            .filter_map(|s| s.strip_prefix(':').map(|p| p.to_string()))
            .collect();
        FlatRoute {
            method,
            path: path.to_string(),
            controller: ClassId(Symbol::from(controller)),
            action: Symbol::from(action),
            as_name: String::new(),
            required_params: params.len(),
            path_params: params,
            named: false,
            format: None,
            int_params: vec![],
            constraints: vec![],
        }
    }

    fn ex(node: ExprNode) -> Expr {
        Expr::new(crate::span::Span::synthetic(), node)
    }

    fn ivar(name: &str) -> Expr {
        ex(ExprNode::Ivar { name: Symbol::from(name) })
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        ex(ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }

    /// The plain-Ruby pass-through is an allowlist: a statement carries
    /// over verbatim only when nothing in its tree needs a Sequel
    /// spelling. Anything else is commented, which is honest, rather
    /// than emitted, which may be wrong (#67 — the `find_each` /
    /// `save!` / `record_timestamps=` false positives).
    #[test]
    fn pass_through_admits_plain_ruby_and_refuses_rails_vocabulary() {
        let lit = || ex(ExprNode::Lit { value: Literal::Str { value: "Login".to_string() } });

        // `@title = "Login"` — no framework vocabulary anywhere.
        assert!(plain_ruby(&ex(ExprNode::Assign {
            target: crate::expr::LValue::Ivar { name: Symbol::from("title") },
            value: lit(),
        })));

        // `@story.is_deleted = true` — Sequel generates the setter.
        assert!(plain_ruby(&ex(ExprNode::Assign {
            target: crate::expr::LValue::Attr {
                recv: ivar("story"),
                name: Symbol::from("is_deleted"),
            },
            value: ex(ExprNode::Lit { value: Literal::Bool { value: true } }),
        })));

        // `@user.is_moderator?` — an app-defined model predicate, which
        // carries over with the model class itself.
        assert!(plain_ruby(&send(Some(ivar("user")), "is_moderator?", vec![])));

        // `@comments.find_each` — AR persistence/query vocabulary.
        assert!(!plain_ruby(&send(Some(ivar("comments")), "find_each", vec![])));

        // `@user.dup` — AR resets the pk on dup, Sequel copies it.
        assert!(!plain_ruby(&send(Some(ivar("user")), "dup", vec![])));

        // `Story.where(...)` — a Const receiver needs a real spelling.
        assert!(!plain_ruby(&send(
            Some(ex(ExprNode::Const { path: vec![Symbol::from("Story")] })),
            "where",
            vec![],
        )));

        // `paginate(...)` — a bare call has no receiver in the Roda app.
        assert!(!plain_ruby(&send(None, "paginate", vec![])));

        // `story_path(@story)` — a route helper, not plain Ruby.
        assert!(!plain_ruby(&send(Some(ivar("routes")), "story_path", vec![])));

        // `flash[:error] = "…"` — the receiver is a Rails request bag.
        assert!(!plain_ruby(&ex(ExprNode::Assign {
            target: crate::expr::LValue::Index {
                recv: send(None, "flash", vec![]),
                index: lit(),
            },
            value: lit(),
        })));
    }

    /// Two routes sharing a path+verb, distinguished only by a
    /// `constraints:` regexp (Lobsters' single- vs multi-`/t/:tag`,
    /// #67), must emit ONE verb block with an `if <regex>.match?`
    /// guard — not two shadowing `r.get` blocks (the second dead).
    #[test]
    fn constraint_distinguished_routes_emit_guard_not_duplicate() {
        use HttpMethod::*;
        let mut single = flat(Get, "/t/:tag", "HomeController", "single_tag");
        single.constraints = vec![("tag".to_string(), "[^,.\\/]+".to_string())];
        let multi = flat(Get, "/t/:tag", "HomeController", "multi_tag");
        let routes = vec![single, multi];
        let trie = build_trie(&routes);
        let app = App::default();
        let loads: Vec<FilterLoad> = vec![];
        let ctx = EmitCtx { app: &app, routes: &routes, loads: &loads };
        let mut out = String::new();
        emit_node(&trie, &ctx, 2, None, &[], &mut out);

        assert_eq!(
            out.matches("r.get").count(),
            1,
            "one verb block, not shadowing duplicates:\n{out}"
        );
        assert!(
            out.contains("if /\\A[^,.\\/]+\\z/.match?(tag)"),
            "anchored constraint guard emitted:\n{out}"
        );
        assert!(out.contains("else"), "unconstrained route is the else branch:\n{out}");
        assert!(
            !out.contains("unreachable duplicate"),
            "constraint recovered — no duplicate TODO:\n{out}"
        );
    }

    /// The blog's flat table re-nests into the exemplar's tree shape:
    /// no duplicate branches, r.is for multi-verb terminals, collapsed
    /// single-verb leaves, one Integer node shared by `:id` and
    /// `:article_id`, and `r.post true` where branches continue below.
    #[test]
    fn trie_renests_blog_routes_without_duplicates() {
        use HttpMethod::*;
        let routes = vec![
            flat(Get, "/articles", "ArticlesController", "index"),
            flat(Post, "/articles", "ArticlesController", "create"),
            flat(Get, "/articles/new", "ArticlesController", "new"),
            flat(Get, "/articles/:id/edit", "ArticlesController", "edit"),
            flat(Get, "/articles/:id", "ArticlesController", "show"),
            flat(Patch, "/articles/:id", "ArticlesController", "update"),
            flat(Delete, "/articles/:id", "ArticlesController", "destroy"),
            flat(Post, "/articles/:article_id/comments", "CommentsController", "create"),
            flat(
                Delete,
                "/articles/:article_id/comments/:id",
                "CommentsController",
                "destroy",
            ),
        ];
        let trie = build_trie(&routes);

        // One "articles" branch total (no duplicates).
        assert_eq!(trie.stat.len(), 1);
        let articles = &trie.stat["articles"];
        // Collection: GET + POST terminate at /articles.
        assert_eq!(articles.terminals.len(), 2);
        // Static "new" before the shared dynamic node.
        assert!(articles.stat.contains_key("new"));
        let (names, member) = articles.dynamic.as_ref().expect("Integer node");
        // :id and :article_id share one position → one node, both names.
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"id".to_string()));
        assert!(names.contains(&"article_id".to_string()));
        // Member terminals: GET/PATCH/DELETE on /articles/:id.
        assert_eq!(member.terminals.len(), 3);
        assert!(member.stat.contains_key("edit"));
        let comments = &member.stat["comments"];
        // POST terminates at /comments while DELETE continues below —
        // the `r.post true` shape.
        assert_eq!(comments.terminals.len(), 1);
        assert!(comments.dynamic.is_some());
    }

    #[test]
    fn matcher_picks_integer_for_ids_only() {
        assert_eq!(matcher_for(&["id".to_string(), "article_id".to_string()]), "Integer");
        assert_eq!(matcher_for(&["username".to_string()]), "String");
        assert_eq!(matcher_for(&["id".to_string(), "tag".to_string()]), "String");
    }

    #[test]
    fn block_var_naming() {
        // Interior node with mixed :id/:article_id → one shared `id`.
        assert_eq!(
            block_var(&["id".to_string(), "article_id".to_string()], false, None),
            "id"
        );
        // Leaf keeping a specific source name keeps it.
        assert_eq!(
            block_var(&["comment_id".to_string()], true, Some("comments")),
            "comment_id"
        );
        // Leaf with only the generic :id qualifies by parent segment.
        assert_eq!(block_var(&["id".to_string()], true, Some("comments")), "comment_id");
    }
}
