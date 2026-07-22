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
    emit_node(&trie, &ctx, 2, None, &mut body);

    let content = format!(
        r#"{requires}

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
end
"#,
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
                    emit_terminal_body(t, ctx, depth + 1, out);
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
                emit_terminal_body(t, ctx, depth + 2, out);
                out.push_str(&format!("{}end\n", indent(depth + 1)));
            }
            out.push_str(&format!("{pad}end\n"));
        } else if node.terminals.len() == 1 {
            // Single verb terminates here but deeper branches exist:
            // `r.post true` — the argument makes Roda require full path
            // consumption, so longer paths fall through to the branches.
            let t = &node.terminals[0];
            out.push_str(&format!("{pad}r.{} true do\n", verb(&t.method)));
            emit_terminal_body(t, ctx, depth + 1, out);
            out.push_str(&format!("{pad}end\n"));
        }
    }

    // Static children. A leaf child serving one verb collapses to the
    // matcher-argument form (`r.get "new" do … end`).
    for (seg, child) in &node.stat {
        if child.stat.is_empty() && child.dynamic.is_none() && child.terminals.len() == 1 {
            let t = &child.terminals[0];
            out.push_str(&format!("{pad}r.{} \"{seg}\" do\n", verb(&t.method)));
            emit_terminal_body(t, ctx, depth + 1, out);
            out.push_str(&format!("{pad}end\n"));
        } else {
            out.push_str(&format!("{pad}r.on \"{seg}\" do\n"));
            emit_node(child, ctx, depth + 1, Some(seg), out);
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
        if is_leaf {
            let t = &child.terminals[0];
            out.push_str(&format!("{pad}r.{} Integer do |{var}|\n", verb(&t.method)));
            emit_interior_loads(child, ctx, &var, names, depth + 1, out);
            emit_terminal_body(t, ctx, depth + 1, out);
            out.push_str(&format!("{pad}end\n"));
        } else {
            out.push_str(&format!("{pad}r.on Integer do |{var}|\n"));
            emit_interior_loads(child, ctx, &var, names, depth + 1, out);
            emit_node(child, ctx, depth + 1, None, out);
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

fn emit_terminal_body(route: &FlatRoute, ctx: &EmitCtx, depth: usize, out: &mut String) {
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

    match convert_get_body(route, controller, action, ctx) {
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

/// Day-1 conversion subset: GET actions whose bodies are (possibly
/// empty) sequences of assignments over ActiveRecord query chains —
/// index/show/new/edit shapes. Anything else returns None and falls to
/// the commented-original path. The mutating verbs land next.
fn convert_get_body(
    route: &FlatRoute,
    controller: &Controller,
    action: &Action,
    ctx: &EmitCtx,
) -> Option<Vec<String>> {
    if route.method != HttpMethod::Get {
        return None;
    }
    let mut lines = Vec::new();
    for stmt in statements(&action.body) {
        // Statements the interior load already performs (the
        // before_action find) never appear in ingest GET bodies — the
        // filter is a separate method — so every statement here must
        // convert or the whole body falls back.
        let converted = convert_statement(stmt, ctx)?;
        lines.push(converted);
    }
    let template = match &action.renders {
        RenderTarget::Template { name, .. } => name.to_string(),
        RenderTarget::Inferred => action.name.to_string(),
        _ => return None,
    };
    lines.push(format!("view \"{}/{}\"", view_dir(controller), template));
    Some(lines)
}

fn statements(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}

/// Convert one ingest statement to Sequel-idiom source, or None when it
/// isn't in the Day-1 subset.
fn convert_statement(stmt: &Expr, ctx: &EmitCtx) -> Option<String> {
    let ExprNode::Assign { target, value } = &*stmt.node else { return None };
    let crate::expr::LValue::Ivar { name } = target else { return None };
    let converted = sequelize_query(value, ctx)?;
    Some(format!("@{name} = {converted}"))
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

/// Day-1 placeholder views: enough structure for the converted GET
/// actions to render 200 on the real gems. Day 2 replaces these with
/// translations of the Rails ERB.
fn emit_views(app: &App) -> Vec<EmittedFile> {
    let mut out = vec![
        file("views/layout.erb", LAYOUT_ERB),
        file("views/not_found.erb", "<h1>404 Not Found</h1>\n"),
    ];
    let routes = flatten_routes(app);
    for r in &routes {
        if r.method != HttpMethod::Get {
            continue;
        }
        let Some(c) = app.controllers.iter().find(|c| c.name == r.controller) else {
            continue;
        };
        let Some(action) = find_action(c, r.action.as_str()) else { continue };
        let template = match &action.renders {
            RenderTarget::Template { name, .. } => name.to_string(),
            RenderTarget::Inferred => action.name.to_string(),
            _ => continue,
        };
        let path = format!("views/{}/{}.erb", view_dir(c), template);
        if out.iter().any(|f| f.path.to_string_lossy() == path) {
            continue;
        }
        out.push(EmittedFile {
            path: PathBuf::from(path),
            content: format!(
                "<%# ROUNDHOUSE-TODO(day2): translate app/views/{}/{}.html.erb %>\n\
                 <h1>{}/{}</h1>\n",
                view_dir(c),
                template,
                view_dir(c),
                template
            ),
        });
    }
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
