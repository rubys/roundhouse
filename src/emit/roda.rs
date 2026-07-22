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

pub fn emit(app: &App, fixture: &std::path::Path) -> Vec<EmittedFile> {
    let mut files: Vec<EmittedFile> = Vec::new();
    files.push(file("Gemfile", GEMFILE));
    files.push(file("config.ru", CONFIG_RU));
    files.push(file("db.rb", DB_RB));
    files.extend(emit_migrations(app));
    files.extend(emit_models(app));
    files.push(emit_app_rb(app));
    files.extend(emit_views(app, fixture));
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
        if node.terminals.len() > 1 || (node.terminals.len() == 1 && !has_children) {
            out.push_str(&format!("{pad}r.is do\n"));
            for t in &node.terminals {
                out.push_str(&format!("{}r.{} do\n", indent(depth + 1), verb(&t.method)));
                emit_terminal_body(t, ctx, depth + 2, bindings, out);
                out.push_str(&format!("{}end\n", indent(depth + 1)));
            }
            out.push_str(&format!("{pad}end\n"));
        } else if node.terminals.len() == 1 {
            // Single verb terminates here but deeper branches exist:
            // `r.post true` — the argument makes Roda require full path
            // consumption, so longer paths fall through to the branches.
            let t = &node.terminals[0];
            out.push_str(&format!("{pad}r.{} true do\n", verb(&t.method)));
            emit_terminal_body(t, ctx, depth + 1, bindings, out);
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

    // Dynamic child: an `Integer` matcher (Rails ids are integer PKs;
    // `find` on a non-numeric id 404s, so the typed matcher preserves
    // observable behavior).
    if let Some((names, child)) = &node.dynamic {
        let is_leaf =
            child.stat.is_empty() && child.dynamic.is_none() && child.terminals.len() == 1;
        let var = block_var(names, is_leaf, parent_seg);
        // Every source param name at this position binds to the one
        // block variable; later (deeper) entries shadow earlier ones,
        // so body conversion resolves a param to its innermost binding.
        let mut inner: Vec<(String, String)> = bindings.to_vec();
        for n in names {
            inner.push((n.clone(), var.clone()));
        }
        if is_leaf {
            let t = &child.terminals[0];
            out.push_str(&format!("{pad}r.{} Integer do |{var}|\n", verb(&t.method)));
            emit_interior_loads(child, ctx, &var, names, depth + 1, out);
            emit_terminal_body(t, ctx, depth + 1, &inner, out);
            out.push_str(&format!("{pad}end\n"));
        } else {
            out.push_str(&format!("{pad}r.on Integer do |{var}|\n"));
            emit_interior_loads(child, ctx, &var, names, depth + 1, out);
            emit_node(child, ctx, depth + 1, None, &inner, out);
            out.push_str(&format!("{pad}end\n"));
        }
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
                return name.clone();
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
    names.first().cloned().unwrap_or_else(|| "id".to_string())
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

    match convert_body(route, controller, action, ctx, bindings) {
        Some(lines) => {
            for l in lines {
                if l.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(&format!("{pad}{l}\n"));
                }
            }
        }
        None => {
            // Not yet convertible: carry the original Rails body as a
            // comment (Jeremy's rule) behind a 501 so the tree still
            // routes deterministically.
            out.push_str(&format!(
                "{pad}# ROUNDHOUSE-TODO: convert this action body \
                 ({}#{}, Rails original below):\n",
                route.controller.0, route.action
            ));
            for l in emit_expr(&action.body).lines() {
                let line = format!("{pad}#   {l}");
                out.push_str(line.trim_end());
                out.push('\n');
            }
            out.push_str(&format!("{pad}r.halt [501, {{}}, [\"ROUNDHOUSE-TODO: not converted yet\"]]\n"));
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
) -> Option<Vec<String>> {
    let body = unwrap_respond_to(&action.body);
    let cx = BodyCx { ctx, controller, bindings };
    let mut lines = convert_stmts(&statements_owned(&body), &cx)?;
    if route.method == HttpMethod::Get {
        let template = match &action.renders {
            RenderTarget::Template { name, .. } => name.to_string(),
            RenderTarget::Inferred => action.name.to_string(),
            _ => return None,
        };
        lines.push(format!("view \"{}/{}\"", view_dir(controller), template));
    }
    Some(lines)
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

fn convert_stmts(stmts: &[Expr], cx: &BodyCx) -> Option<Vec<String>> {
    let mut lines = Vec::new();
    for s in stmts {
        lines.extend(convert_stmt(s, cx)?);
    }
    Some(lines)
}

/// One statement → Roda/Sequel source lines, or None (not in the
/// recognized conversion set).
fn convert_stmt(stmt: &Expr, cx: &BodyCx) -> Option<Vec<String>> {
    match &*stmt.node {
        // Format-drop residue / nested sequences: convert recursively.
        ExprNode::Seq { exprs } => convert_stmts(exprs, cx),
        ExprNode::Assign { target, value } => {
            let crate::expr::LValue::Ivar { name } = target else { return None };
            convert_ivar_assign(name.as_str(), value, cx)
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            convert_if(cond, then_branch, else_branch, cx)
        }
        ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "redirect_to" => {
            convert_redirect(args, cx)
        }
        ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "render" => {
            convert_render(args, cx)
        }
        // `@article.destroy!` → `@article.destroy` (Sequel #destroy
        // raises on hook failure already; the bang distinction is
        // Rails-side validation semantics the blog doesn't exercise).
        ExprNode::Send { recv: Some(r), method, args, .. }
            if (method.as_str() == "destroy" || method.as_str() == "destroy!")
                && args.is_empty() =>
        {
            let ExprNode::Ivar { name } = &*r.node else { return None };
            Some(vec![format!("@{name}.destroy")])
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
) -> Option<Vec<String>> {
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
        _ => return None,
    };
    lines.push(cond_str);
    for l in convert_stmts(&statements_owned(then_branch), cx)? {
        lines.push(format!("  {l}"));
    }
    let else_stmts = statements_owned(else_branch);
    let else_empty = else_stmts.len() == 1
        && matches!(&*else_stmts[0].node, ExprNode::Lit { value: Literal::Nil })
        || else_stmts.is_empty();
    if !else_empty {
        lines.push("else".to_string());
        for l in convert_stmts(&else_stmts, cx)? {
            lines.push(format!("  {l}"));
        }
    }
    lines.push("end".to_string());
    Some(lines)
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
fn emit_views(app: &App, fixture: &std::path::Path) -> Vec<EmittedFile> {
    let routes = flatten_routes(app);
    let mut out = vec![
        file("views/layout.erb", LAYOUT_ERB),
        file("views/not_found.erb", "<h1>404 Not Found</h1>\n"),
    ];
    out.extend(views::translate_views(app, fixture, &routes));
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
        }
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
