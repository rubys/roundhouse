//! `db/seeds.rb` → `db/seed.sql`.
//!
//! The published archive is text-only, so it ships a SQL seed a human
//! (or the e2e harness, or `scripts/smoke`) loads with
//! `sqlite3 <db> < db/seed.sql`. That file used to be a HAND-MAINTAINED
//! transcription living in the compiler
//! (`runtime/spinel/scaffold/db/seed.sql`), injected into every archive
//! whose emit produced no seed of its own — which was all of them. Two
//! consequences, both bad:
//!
//!   * the blog shipped the same rows twice, once as the lowered
//!     `Seeds.run()` module derived from its `db/seeds.rb` and once as
//!     the transcription, with nothing keeping them in sync (the file's
//!     own header said "Regenerate … if data changes");
//!   * every OTHER app shipped the BLOG's rows. campfire got
//!     `INSERT INTO articles` against a fifteen-table chat schema, so
//!     its Setup step appeared to succeed and left the database empty.
//!
//! The data was in the IR the whole time. This renders it.
//!
//! ## What it evaluates
//!
//! A deliberately small interpreter over the shape Rails' own generated
//! seeds file uses, and nothing more:
//!
//! ```ruby
//! return if Article.count > 0            # guard — skipped
//! a = Article.create!(title: "…")        # INSERT, binds `a` to its id
//! Comment.create!(article_id: a.id, …)   # INSERT, resolves `a.id`
//! puts "…"                               # ignored
//! ```
//!
//! Anything else — a loop, a conditional creating rows, a computed
//! value — makes the whole thing decline and return `None`. FAILING
//! CLOSED matters here: a partial seed is worse than none, because it
//! looks like it worked. The caller ledgers the decline.
//!
//! ## Timestamps
//!
//! `seeds.rb` never sets `created_at`/`updated_at`, but the columns are
//! `NOT NULL` and the blog's index is `order(created_at: :desc)` — so
//! the values have to exist AND be distinct, or the row order the e2e
//! spec asserts becomes whatever SQLite feels like.
//!
//! They are therefore synthesized: a fixed base instant plus one
//! microsecond per INSERT, in execution order. Fixed rather than
//! wall-clock because the compiler's output must be reproducible; the
//! increment reproduces what running `seeds.rb` against Rails does,
//! including the INTERLEAVING (an article's comments are created
//! between it and the next article, so they sort between them).
//!
//! This is NOT byte-identical to the file it replaces — that one was a
//! `sqlite3 .dump` of a real May-2026 seeded database, so its
//! timestamps are wall-clock values from that run and cannot be
//! derived. Nothing depends on the exact instants: `scripts/compare`
//! copies Rails' own database with `.backup` and never reads this file,
//! and e2e only needs the relative order.

use std::fmt::Write;

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::schema::Schema;

use super::schema_sql::render_schema_statements;

/// Base instant for synthesized timestamps: `2026-01-01 00:00:00`, plus
/// one microsecond per row. Arbitrary but FIXED — the archive must be
/// reproducible from the same input.
const SEED_EPOCH: &str = "2026-01-01 00:00:00";

/// `db/seed.sql` for an app whose seeds could not be rendered: the
/// schema and nothing else.
///
/// Shipping ANOTHER app's rows is the bug this replaces — tiny-blog and
/// roda-blog have `posts`, campfire has fifteen chat tables, and all
/// three were handed the blog's `INSERT INTO articles`, which quietly
/// created a stray table and left the real ones empty. An empty
/// database is the honest answer, and a usable one: the app boots and
/// its own first-run/signup flow works, exactly as Rails behaves
/// against a fresh database.
pub fn render_schema_only_sql(app: &App) -> Option<String> {
    let stmts = render_schema_statements(&app.schema);
    if stmts.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str(
        "-- Schema only. This app defines no `db/seeds.rb` (or its seeds use a\n\
         -- shape the generator declines), so there are no rows to ship — an\n\
         -- empty database is the honest result, and the app's own signup /\n\
         -- first-run flow works against it exactly as it does under Rails.\n\
         -- `sqlite3 <db> < db/seed.sql` creates the tables.\n",
    );
    for stmt in stmts {
        out.push_str(&collapse_ws(&stmt));
        out.push_str(";\n");
    }
    Some(out)
}

/// Render `db/seed.sql` for `app`, or `None` when its seeds are absent
/// or use a shape this cannot evaluate.
pub fn render_seed_sql(app: &App) -> Option<String> {
    // The LOWERED body, not `app.seeds` itself: `seeds_to_library`
    // de-magics Rails' has-many shorthand
    // (`article.comments.create!(…)` → `Comment.create!(article_id:
    // article.id, …)`) on the way out and leaves `app.seeds` untouched.
    // Interpreting the raw form would decline on every association row.
    let seeds = crate::lower::seeds_to_library::rewrite_assoc_create(app.seeds.as_ref()?);
    let inserts = Interp::new(&app.schema).run(&seeds)?;
    if inserts.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str(
        "-- Seed data, generated from the app's own db/seeds.rb.\n\
         -- The archive is text-only, so no binary database ships; this file\n\
         -- populates a fresh one for any target: `sqlite3 <db> < db/seed.sql`.\n\
         -- INSERTs name their columns explicitly so the file stays valid whatever\n\
         -- order a target emits its schema columns in.\n\
         -- created_at/updated_at are synthesized (seeds.rb sets neither, and the\n\
         -- columns are NOT NULL): a fixed base instant plus one microsecond per\n\
         -- row, in creation order, so ordering by them is deterministic.\n",
    );
    for stmt in render_schema_statements(&app.schema) {
        // One `;`-terminated line per statement — this file is piped to
        // `sqlite3`, and the shared renderer's pretty multi-line form
        // would leave its indentation stranded mid-line.
        out.push_str(&collapse_ws(&stmt));
        out.push_str(";\n");
    }
    for insert in inserts {
        out.push_str(&insert);
        out.push('\n');
    }
    Some(out)
}

/// A local bound by `a = Model.create!(…)`, remembered as the row id
/// the INSERT assigned so `a.id` resolves.
struct Binding {
    name: String,
    id: i64,
}

struct Interp<'s> {
    schema: &'s Schema,
    /// Next id per table, 1-indexed like AUTOINCREMENT.
    next_id: std::collections::HashMap<String, i64>,
    bindings: Vec<Binding>,
    /// Microseconds past `SEED_EPOCH` for the next row.
    tick: i64,
    out: Vec<String>,
}

impl<'s> Interp<'s> {
    fn new(schema: &'s Schema) -> Self {
        Self {
            schema,
            next_id: std::collections::HashMap::new(),
            bindings: Vec::new(),
            tick: 0,
            out: Vec::new(),
        }
    }

    fn run(mut self, body: &Expr) -> Option<Vec<String>> {
        self.stmts(body)?;
        Some(self.out)
    }

    fn stmts(&mut self, e: &Expr) -> Option<()> {
        match &*e.node {
            ExprNode::Seq { exprs } => {
                for stmt in exprs {
                    self.stmts(stmt)?;
                }
                Some(())
            }
            // `return if Article.count > 0` — the idempotence guard. It
            // is about re-running against a live database; a freshly
            // generated file is always the empty case.
            ExprNode::If { then_branch, else_branch, .. } => {
                if is_guard(then_branch) {
                    return self.stmts(else_branch);
                }
                // A conditional that CREATES rows would make the file
                // depend on runtime state. Decline.
                None
            }
            // `a = Model.create!(…)` — insert, then bind the id.
            ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
                let id = self.insert(value)?;
                self.bindings.push(Binding { name: name.as_str().to_string(), id });
                Some(())
            }
            ExprNode::Lit { value: Literal::Nil } => Some(()),
            // A bare `Model.create!(…)` statement, or a `puts` to ignore.
            _ => {
                if is_ignorable(e) {
                    return Some(());
                }
                self.insert(e).map(|_| ())
            }
        }
    }

    /// Emit one INSERT for a `Model.create!(k: v, …)` call, returning
    /// the id it was given.
    fn insert(&mut self, e: &Expr) -> Option<i64> {
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else {
            return None;
        };
        if !matches!(method.as_str(), "create!" | "create") || args.len() != 1 {
            return None;
        }
        let ExprNode::Const { path } = &*recv.node else { return None };
        let model = path.last()?.as_str();
        let table = self.table_for(model)?;
        let ExprNode::Hash { entries, .. } = &*args[0].node else {
            return None;
        };

        let id = {
            let slot = self.next_id.entry(table.clone()).or_insert(0);
            *slot += 1;
            *slot
        };
        let stamp = self.next_stamp();

        let mut cols: Vec<String> = vec!["id".to_string()];
        let mut vals: Vec<String> = vec![id.to_string()];
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                return None;
            };
            cols.push(key.as_str().to_string());
            vals.push(self.value(v)?);
        }
        // Rails stamps these on every row; the columns are NOT NULL.
        for ts in ["created_at", "updated_at"] {
            if !cols.iter().any(|c| c == ts) && self.has_column(&table, ts) {
                cols.push(ts.to_string());
                vals.push(sql_str(&stamp));
            }
        }
        let mut stmt = String::new();
        write!(
            stmt,
            "INSERT INTO {} ({}) VALUES ({});",
            table,
            cols.join(", "),
            vals.join(",")
        )
        .ok()?;
        self.out.push(stmt);
        Some(id)
    }

    /// A literal, or `<local>.id` for a row created earlier.
    fn value(&self, e: &Expr) -> Option<String> {
        match &*e.node {
            ExprNode::Lit { value } => match value {
                Literal::Str { value } => Some(sql_str(value)),
                Literal::Int { value } => Some(value.to_string()),
                Literal::Float { value } => Some(value.to_string()),
                Literal::Bool { value } => Some(if *value { "1" } else { "0" }.to_string()),
                Literal::Nil => Some("NULL".to_string()),
                Literal::Sym { value } => Some(sql_str(value.as_str())),
                Literal::Regex { .. } => None,
            },
            // `article1.id` — the id the earlier INSERT assigned.
            ExprNode::Send { recv: Some(r), method, args, .. }
                if method.as_str() == "id" && args.is_empty() =>
            {
                let ExprNode::Var { name, .. } = &*r.node else { return None };
                let b = self
                    .bindings
                    .iter()
                    .find(|b| b.name == name.as_str())?;
                Some(b.id.to_string())
            }
            _ => None,
        }
    }

    /// `2026-01-01 00:00:00.000001`, one microsecond later each call.
    fn next_stamp(&mut self) -> String {
        self.tick += 1;
        format!("{SEED_EPOCH}.{:06}", self.tick)
    }

    fn table_for(&self, model: &str) -> Option<String> {
        let want = crate::naming::pluralize_snake(&crate::naming::underscore(model));
        self.schema
            .tables
            .iter()
            .find(|(_, t)| t.name.as_str() == want)
            .map(|(_, t)| t.name.as_str().to_string())
    }

    fn has_column(&self, table: &str, col: &str) -> bool {
        self.schema
            .tables
            .iter()
            .find(|(_, t)| t.name.as_str() == table)
            .is_some_and(|(_, t)| t.columns.iter().any(|c| c.name.as_str() == col))
    }
}

/// `return` / `nil` — the body of the idempotence guard.
fn is_guard(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Return { .. } | ExprNode::Lit { value: Literal::Nil }
    )
}

/// `puts "Created …"` — reporting, not data.
fn is_ignorable(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, .. }
        if matches!(method.as_str(), "puts" | "print" | "p"))
}

/// Fold a multi-line DDL statement onto one line, squeezing the
/// pretty-printer's indentation out.
fn collapse_ws(stmt: &str) -> String {
    let mut out = String::with_capacity(stmt.len());
    let mut pending_space = false;
    for c in stmt.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space && c != ')' && c != ',' {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    out
}

/// A single-quoted SQL string with `'` doubled — SQLite's only escape.
fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
