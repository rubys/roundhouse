//! Tests for transitive filter-target ivar-write discovery
//! (`collect_transitive_filter_ivars` in `src/analyze/mod.rs`).
//!
//! The gap: Roundhouse seeded a controller's ivars only from writes
//! syntactically present in the BODY of a filter target itself. A
//! filter target that merely dispatches to another method — which
//! dispatches to another, possibly on a concern or a parent controller
//! — contributed nothing, even though Ruby shares instance variables
//! across the whole call chain. Procore's real chain is
//! `before_action :authorize` → `authorize` (a concern method) →
//! `set_variables_in_authorize!` (a parent-controller method) →
//! `set_project_variables!` (same parent) → `@project = Project.find(...)`,
//! and every one of those hops used to make `@project` invisible to the
//! analyzer.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::expr::{Expr, ExprNode, InterpPart, LValue};
use roundhouse::ty::Ty;
use roundhouse::Symbol;

fn app_from_files(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Names of every `@ivar` the analyzer couldn't bind a type for.
fn ivar_unresolved_names(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::IvarUnresolved { name } => Some(name.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// Every `@ivar` read in `expr`, paired with its inferred type (mirrors
/// `tests/analyze.rs::collect_ivar_reads`, duplicated here to keep this
/// file independent).
fn collect_ivar_reads(expr: &Expr, out: &mut Vec<(Symbol, Option<Ty>)>) {
    match &*expr.node {
        ExprNode::Ivar { name } => out.push((name.clone(), expr.ty.clone())),
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                collect_ivar_reads(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivar_reads(k, out);
                collect_ivar_reads(v, out);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivar_reads(r, out);
            }
            for a in args {
                collect_ivar_reads(a, out);
            }
            if let Some(b) = block {
                collect_ivar_reads(b, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_ivar_reads(expr, out);
                }
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivar_reads(cond, out);
            collect_ivar_reads(then_branch, out);
            collect_ivar_reads(else_branch, out);
        }
        // Safe navigation (`@project&.name`) lowers to a `BoolOp`
        // (`@project && @project.name`) — without this arm the `@project`
        // read guarding a safe-nav chain is invisible.
        ExprNode::BoolOp { left, right, .. } | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_ivar_reads(left, out);
            collect_ivar_reads(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_ivar_reads(value, out);
            collect_ivar_reads(body, out);
        }
        ExprNode::Lambda { body, .. } => collect_ivar_reads(body, out),
        // ERB-compiled view bodies are almost entirely buffer-append
        // Assigns (`_buf = _buf + @project.name.to_s`) — without this
        // arm, nothing inside a compiled view's `Seq` of `Assign` nodes
        // is ever reached, which is where every ivar read in a real
        // view actually lives.
        ExprNode::Assign { target, value } | ExprNode::OpAssign { target, value, .. } => {
            collect_ivar_reads(value, out);
            if let LValue::Attr { recv, .. } = target {
                collect_ivar_reads(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                collect_ivar_reads(recv, out);
                collect_ivar_reads(index, out);
            }
        }
        _ => {}
    }
}

fn ivar_read_ty<'a>(reads: &'a [(Symbol, Option<Ty>)], name: &str) -> Option<&'a Ty> {
    reads.iter().find(|(n, _)| n.as_str() == name).and_then(|(_, t)| t.as_ref())
}

const SCHEMA_RB: &str = r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "projects", force: :cascade do |t|
    t.string "name"
  end
end
"#;

/// The scenario the whole feature exists for: a `before_action` target
/// defined on the controller itself dispatches into a method from an
/// INCLUDED CONCERN, which dispatches into a method defined on a
/// PARENT CONTROLLER several classes up, which is where the actual
/// `@project = ...` write lives. None of these three hops share a
/// class with the one before it — the same shape as Procore's real
/// `authorize` → `set_variables_in_authorize!` →
/// `set_project_variables!` chain quoted in the PR description.
#[test]
fn transitive_filter_ivars_resolve_through_concern_and_parent_controller() {
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/base_controller.rb",
            r#"class BaseController < ApplicationController
  private

  def assign_project
    @project = Project.find(params[:id])
  end
end
"#,
        ),
        (
            "app/controllers/concerns/project_concern.rb",
            r#"module ProjectConcern
  extend ActiveSupport::Concern

  private

  def set_project
    assign_project
  end
end
"#,
        ),
        (
            "app/controllers/projects_controller.rb",
            r#"class ProjectsController < BaseController
  include ProjectConcern

  before_action :load_stuff

  def show
  end

  private

  def load_stuff
    set_project
  end
end
"#,
        ),
        (
            "app/models/project.rb",
            "class Project < ApplicationRecord\nend\n",
        ),
        ("app/views/projects/show.html.erb", "<p><%= @project.name %></p>\n"),
        ("db/schema.rb", SCHEMA_RB),
    ]);

    let unresolved = ivar_unresolved_names(&app);
    assert!(
        !unresolved.iter().any(|n| n == "project"),
        "@project should resolve transitively through load_stuff -> \
         set_project (concern) -> assign_project (parent controller); \
         unresolved = {unresolved:?}"
    );

    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "projects/show")
        .expect("projects/show view");
    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);
    let ty = ivar_read_ty(&reads, "project").expect("@project read carries a type");
    // Unconditionally reached on every path through `load_stuff`, so
    // this should type as plain `Project`, not a nilable union.
    match ty {
        Ty::Class { id, .. } => assert_eq!(id.0.as_str(), "Project"),
        other => panic!("expected @project : Project, got {other:?}"),
    }
}

/// The depth cap: a write six methods away from the filter target (five
/// hops: target -> hop1 -> hop2 -> hop3 -> hop4 -> hop5, write inside
/// hop5) must NOT resolve — `MAX_FILTER_CALL_DEPTH` is 4, so the walk
/// gives up one hop short of `hop5`. This pins the cap's existence
/// (a runaway or accidentally-cyclic call graph must not make the
/// analyzer's whole-program fixpoint unbounded) rather than a specific
/// depth value; if the cap is ever deliberately raised, this test's
/// chain should grow with it rather than being deleted.
#[test]
fn transitive_filter_ivars_do_not_cross_the_depth_cap() {
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/deep_controller.rb",
            r#"class DeepController < ApplicationController
  before_action :load_chain

  def show
  end

  private

  def load_chain
    chain_hop1
  end

  def chain_hop1
    chain_hop2
  end

  def chain_hop2
    chain_hop3
  end

  def chain_hop3
    chain_hop4
  end

  def chain_hop4
    chain_hop5
  end

  def chain_hop5
    @deep = Project.find(params[:id])
  end
end
"#,
        ),
        (
            "app/models/project.rb",
            "class Project < ApplicationRecord\nend\n",
        ),
        ("app/views/deep/show.html.erb", "<p><%= @deep&.name %></p>\n"),
        ("db/schema.rb", SCHEMA_RB),
    ]);

    let unresolved = ivar_unresolved_names(&app);
    assert!(
        unresolved.iter().any(|n| n == "deep"),
        "@deep is 5 hops from the filter target, past the depth cap, \
         and must stay unresolved; unresolved = {unresolved:?}"
    );
}

/// A write reachable through only ONE alternative of a branching
/// construct (an `if` with no `else`) must come out nilable — the
/// analyzer never evaluates which branch runs, so "written on this arm,
/// not written on the other" is exactly as uncertain as Procore's real
/// `set_variables_in_authorize!`, where `@project` is written only on
/// the `project_area?` arm and left alone by `company_area?`/
/// `super_area?`/the trailing `else`.
#[test]
fn transitive_filter_ivars_from_a_conditional_write_are_nilable() {
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/flags_controller.rb",
            r#"class FlagsController < ApplicationController
  before_action :load_optional

  def show
  end

  private

  def load_optional
    maybe_assign_project
  end

  def maybe_assign_project
    if params[:eager]
      @project = Project.find(params[:id])
    end
  end
end
"#,
        ),
        (
            "app/models/project.rb",
            "class Project < ApplicationRecord\nend\n",
        ),
        ("app/views/flags/show.html.erb", "<p><%= @project&.name %></p>\n"),
        ("db/schema.rb", SCHEMA_RB),
    ]);

    let unresolved = ivar_unresolved_names(&app);
    assert!(
        !unresolved.iter().any(|n| n == "project"),
        "@project should resolve (nilably) rather than stay unresolved; \
         unresolved = {unresolved:?}"
    );

    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "flags/show")
        .expect("flags/show view");
    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);
    let ty = ivar_read_ty(&reads, "project").expect("@project read carries a type");
    match ty {
        Ty::Union { variants } => {
            assert!(variants.contains(&Ty::Nil), "expected a Nil arm, got {variants:?}");
            assert!(
                variants.iter().any(|v| matches!(v, Ty::Class { id, .. } if id.0.as_str() == "Project")),
                "expected a Project arm, got {variants:?}"
            );
        }
        other => panic!(
            "expected @project : Project? (a nilable union) since the write is \
             only reachable on one `if` arm with no `else`; got {other:?}"
        ),
    }
}

/// A filter target's OWN direct writes are not the walk's to report:
/// they are already in the chain's bindings, typed by the controller-
/// wide pass. The walk reads each body as the per-action pass typed it,
/// where `@account` — set by a different filter — is not known yet, so
/// re-collecting `@status = @account.statuses.find(...)` from there
/// unioned an `untyped` into the resolved `Status`. Mastodon's
/// `StatusesController#set_status` is this shape; the IDE's hover on
/// `@status` in `statuses/show` read `Status | untyped`.
#[test]
fn transitive_filter_ivars_leave_the_targets_own_writes_to_the_chain() {
    let app = app_from_files(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/statuses_controller.rb",
            r#"class StatusesController < ApplicationController
  before_action :set_account
  before_action :set_status

  def show
  end

  private

  def set_account
    @account = Account.find(params[:account_id])
  end

  def set_status
    @status = @account.statuses.find(params[:id])
    check_status
  end

  def check_status
  end
end
"#,
        ),
        (
            "app/models/account.rb",
            "class Account < ApplicationRecord\n  has_many :statuses\nend\n",
        ),
        (
            "app/models/status.rb",
            "class Status < ApplicationRecord\n  belongs_to :account\nend\n",
        ),
        ("app/views/statuses/show.html.erb", "<p><%= @status.text %></p>\n"),
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "accounts", force: :cascade do |t|
    t.string "name"
  end
  create_table "statuses", force: :cascade do |t|
    t.integer "account_id"
    t.string "text"
  end
end
"#,
        ),
    ]);

    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "statuses/show")
        .expect("statuses/show view");
    let mut reads = Vec::new();
    collect_ivar_reads(&view.body, &mut reads);
    let ty = ivar_read_ty(&reads, "status").expect("@status read carries a type");
    match ty {
        Ty::Class { id, .. } => assert_eq!(id.0.as_str(), "Status"),
        other => panic!("expected @status : Status, got {other:?}"),
    }
}
