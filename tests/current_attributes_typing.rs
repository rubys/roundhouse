//! `class Current < ActiveSupport::CurrentAttributes` — the class every
//! per-request read in campfire goes through, and the one the analyzer
//! could see nothing inside.
//!
//! Three separate reasons it was shapeless, each of which had to go:
//!
//! 1. **The writes are outside the class.** `Current.session = session`
//!    is app code; the only write the class's own syntax shows is the
//!    generated `def session=(value); @session = value; end`, whose
//!    parameter has no type. The seed comes from surveying the app.
//! 2. **`reset` nilled the singleton.** `@__instance`'s type is the
//!    union of what the class assigns it, so one `= nil` made
//!    `self.instance` answer `Current | Nil` and every class-level
//!    forwarder register `Untyped`. `reset` now REPLACES the instance,
//!    which is what resetting a CurrentAttributes MEANS.
//! 3. **The forwarder's body cannot type itself.** `Ty::Class { Current }`
//!    is both the class object and an instance here, and the
//!    class-method table is consulted first, so `Current.instance.user`
//!    looks up the forwarder it is in the middle of computing. The
//!    answer is copied from the instance twin instead.
//!
//! And the Nil arm STAYS. That is the fourth test: `signed_in?` is
//! `Current.user.present?`, and against a non-nilable type it folds to
//! `true` — a correct fold of an incorrect type. Stripping nil here (as
//! the controller-wide ivar seed does) signed everyone in.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\n  \
    create_table :rooms do |t|\n    t.integer :user_id\n    t.string :name\n  end\nend\n";

const CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :user
end
"#;

const USER: &str = r#"
class User < ApplicationRecord
  has_many :rooms
end
"#;

const ROOM: &str = "class Room < ApplicationRecord\n  belongs_to :user\nend\n";

const CONTROLLER: &str = r#"
class RoomsController < ApplicationController
  def index
    Current.user = User.first
    @rooms = Current.user.rooms
  end

  def show
    head :forbidden unless signed_in?
  end

  private
    def signed_in?
      Current.user.present?
    end
end
"#;

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
        ("app/models/current.rb", CURRENT),
        ("app/models/user.rb", USER),
        ("app/models/room.rb", ROOM),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn errors(needle: &str) -> Vec<String> {
    diagnose(&app())
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains(needle))
        .collect()
}

fn emitted(stem: &str) -> String {
    let app = app();
    // `emit_spinel` emits the lowered models/controllers/views;
    // `Current` is a LIBRARY class and rides the other half.
    let mut files = ruby::emit_spinel(&app);
    files.extend(ruby::emit_library(&app));
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(stem))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ends with {stem}; got {:?}",
                files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()
            )
        })
}

/// The whole point: a read through the class-level forwarder answers a
/// model, so the hop after it dispatches.
#[test]
fn a_read_through_the_forwarder_answers_the_model() {
    let o = errors("rooms");
    assert!(
        o.is_empty(),
        "`Current.user.rooms` must dispatch — the write site says `User`: {o:?}"
    );
}

/// Ablation for reason 2: `reset` must not nil the slot, or
/// `self.instance` answers `Current | Nil` and nothing downstream types.
#[test]
fn reset_replaces_the_instance_rather_than_nilling_it() {
    let src = emitted("app/models/current.rb");
    assert!(
        src.contains("Thread.current[:__current_attrs_Current] = Current.new")
            && !src.contains("Thread.current[:__current_attrs_Current] = nil"),
        "resetting a CurrentAttributes means a fresh instance:\n{src}"
    );
}

/// Ablation for the Nil arm: `present?` on a nilable model folds to a
/// nil CHECK. Folding it to `true` is what signing everyone in looks
/// like, so assert the constant is not there.
#[test]
fn the_nil_arm_survives_so_presence_is_still_asked() {
    let src = emitted("app/controllers/rooms_controller.rb");
    assert!(
        src.contains("Current.user.nil?"),
        "`Current.user.present?` must still ask; it must not fold to true:\n{src}"
    );
}

/// Ablation for the thread-local read: `Thread#[]` answers untyped, and
/// a reader that returned it on a mere `nil?` test signed its type as
/// `untyped` — spinel then dispatched every forwarder on a boxed value.
#[test]
fn the_instance_reader_narrows_the_thread_local_to_the_class() {
    let sig = emitted("current.rbs");
    assert!(sig.contains("def self.instance: () -> Current"), "{sig}");
}

/// Analyze BEFORE lowering: a scoped call needs truthful source types
/// before a monomorphic emitted block caller can inherit them.
#[test]
fn literal_current_set_binds_and_returns_its_owned_block_values() {
    use roundhouse::{Expr, ExprNode, Ty};
    let source = r#"
class ScopeProbe
  def self.seed
    Current.user = 47
  end

  def self.result
    Current.set(user: 83) { 31 }
  end

  def self.shadow
    context = 7
    Current.set(user: 83) { |context| context.user }
  end

  def self.two_params
    extra = "outer"
    Current.set(user: 83) { |context, extra| extra }
  end

  def self.next_result
    Current.set(user: 83) { next 41 }
  end

  def self.break_result
    Current.set(user: 83) { break 59 }
  end

  def self.downstream
    Current.set(user: 83) { 29 }.positive?
  end
end
"#;
    let tree = [
        ("app/models/current.rb", CURRENT),
        ("app/lib/scope_probe.rb", source),
    ].into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("synthetic ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let probe = app.library_classes.iter().find(|c| c.name.0.as_str() == "ScopeProbe").unwrap();
    let mut failures = Vec::new();
    for (name, expected) in [
        ("result", Ty::Int),
        ("two_params", Ty::Nil),
        ("next_result", Ty::Int),
        ("break_result", Ty::Int),
        ("downstream", Ty::Bool),
    ] {
        let method = probe.methods.iter().find(|m| m.name.as_str() == name).unwrap();
        println!("{name}: body={:?}; signature={:?}", method.body.ty, method.signature);
        if method.body.ty.as_ref() != Some(&expected) {
            failures.push(format!("{name}: expected {expected:?}, got {:?}", method.body.ty));
        }
    }
    fn collect_context_reads(e: &Expr, out: &mut Vec<Ty>) {
        if let ExprNode::Var { name, .. } = &*e.node {
            if name.as_str() == "context" {
                if let Some(ty) = &e.ty { out.push(ty.clone()); }
            }
        }
        e.node.for_each_child(&mut |c| collect_context_reads(c, out));
    }
    let shadow = probe.methods.iter().find(|m| m.name.as_str() == "shadow").unwrap();
    let mut bindings = Vec::new();
    collect_context_reads(&shadow.body, &mut bindings);
    println!("shadow block context reads: {bindings:?}");
    if bindings != vec![Ty::Class { id: roundhouse::ident::ClassId("Current".into()), args: vec![] }] {
        failures.push(format!("shadow block should bind Current, not outer Int: {bindings:?}"));
    }
    let errors: Vec<_> = diagnose(&app).into_iter().filter(|d| d.severity == roundhouse::diagnostic::Severity::Error).map(|d| d.to_string()).collect();
    if !errors.is_empty() { failures.push(format!("source errors: {errors:?}")); }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Controls that a type-only fix can easily get wrong: a set-only
/// writer must not still type its reader as always nil, and exits
/// belong to their actual loop/block rather than the outer scope.
#[test]
fn literal_current_set_writer_and_exit_ownership_controls() {
    use roundhouse::Ty;
    let source = r#"
class ScopeProbe
  def self.scoped_writer
    Current.set(account: "scoped") { |context| context.account }
  end

  def self.nested_block
    Current.set(user: 83) { [1].each { break "inner" }; 31 }
  end

  def self.loop_tail
    Current.set(user: 83) { while true; break "loop"; end }
  end

  def self.nested_call_tail
    Current.set(user: 83) { [1].each { break "inner" } }
  end

  def self.loop_assignment
    Current.set(user: 83) { result = while true; break "loop"; end; result }
  end

  def self.nested_call_argument
    Current.set(user: 83) { [([1].each { break "inner" })] }
  end

  def self.forwarded_lambda
    Current.set(user: 83, &:to_s)
    "method"
  end

  def self.inner_forwarded_lambda
    Current.set(user: 83) { [1].each(&:to_s); "scope" }
    "method"
  end

  def self.exit_operand
    Current.set(user: 83) { next(if true; next "actual"; else; 31; end) }
  end

  def self.nonlocal_return
    Current.set(user: 83) { return "method" }
    raise "unreachable"
  end
end
class OtherCurrent < ActiveSupport::CurrentAttributes
  attribute :user
  def self.set(values)
    "override"
  end
end
class OverrideProbe
  def self.result
    OtherCurrent.set(user: 47) { 83 }
  end

  def self.included
    IncludedCurrent.set(user: 47) { 83 }
  end

  def self.transitive
    TransitiveCurrent.set(user: 47) { 83 }
  end

  def self.unrelated
    OtherSet.set(user: 47) { 83 }
  end
end
module ScopeOverride
  def set(values)
    "override"
  end
end
module IndirectOverride
  include ScopeOverride
end
class IncludedCurrent < ActiveSupport::CurrentAttributes
  include ScopeOverride
  attribute :user
end
class TransitiveCurrent < ActiveSupport::CurrentAttributes
  include IndirectOverride
  attribute :user
end
class OtherSet
  def self.set(values)
    "unrelated"
  end
end
"#;
    let tree = [
        ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n attribute :user, :account\nend"),
        ("app/lib/scope_probe.rb", source),
    ].into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("synthetic ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let mut failures = Vec::new();
    for (owner, name, required) in [
        ("ScopeProbe", "scoped_writer", Ty::Str),
        ("ScopeProbe", "exit_operand", Ty::Str),
        ("OverrideProbe", "result", Ty::Str),
        ("OverrideProbe", "included", Ty::Str),
        ("OverrideProbe", "unrelated", Ty::Str),
    ] {
        let class = app.library_classes.iter().find(|c| c.name.0.as_str() == owner).unwrap();
        let method = class.methods.iter().find(|m| m.name.as_str() == name).unwrap();
        println!("{owner}.{name}: body={:?}; signature={:?}", method.body.ty, method.signature);
        // A conservative nil arm is allowed; excluding the native value
        // from a closed type is not truthful inference.
        let contains_required = match &method.body.ty {
            Some(ty) if *ty == required => true,
            Some(Ty::Union { variants }) => variants.contains(&required),
            _ => false,
        };
        if !contains_required {
            failures.push(format!("{owner}.{name}: native value requires {required:?}, got {:?}", method.body.ty));
        }
    }
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ScopeProbe").unwrap();
    let refusals = roundhouse::lower::current_set::source_refusals(&app);
    for (name, reason) in [
        ("loop_tail", "loop result ownership"),
        ("loop_assignment", "loop result ownership"),
        ("nested_call_tail", "nested block-call result ownership"),
        ("nested_call_argument", "nested block-call result ownership"),
        ("nested_block", "nested block-call result ownership"),
        ("forwarded_lambda", "converted or forwarded block operands"),
        ("inner_forwarded_lambda", "converted or forwarded inner blocks"),
    ] {
        let method = class.methods.iter().find(|m| m.name.as_str() == name).unwrap();
        // These frozen native String-result failures are now explicit
        // unsupported controls, including propagation through non-tail
        // assignments and call arguments. Do not infer an invented
        // framework result or borrow a nested break to make them pass.
        assert!(refusals.iter().any(|d|
            !d.span.is_synthetic() && d.span.file == method.body.span.file
                && d.span.start >= method.body.span.start && d.span.end <= method.body.span.end
                && d.message.contains(reason)
                && d.severity == roundhouse::diagnostic::Severity::Error
        ), "{name}: expected source-located refusal: {refusals:?}");
    }
    // The discarded inner result still types its enclosing block as
    // Int, not String. Sound narrowing rejects the shape; it does not
    // misattribute the inner break to Current.set.
    let nested = class.methods.iter().find(|m| m.name.as_str() == "nested_block").unwrap();
    let roundhouse::ExprNode::Send { block: Some(block), .. } = &*nested.body.node else { panic!("scope call") };
    let roundhouse::ExprNode::Lambda { body, .. } = &*block.node else { panic!("scope block") };
    assert_eq!(body.ty, Some(Ty::Int));
    let method = class.methods.iter().find(|m| m.name.as_str() == "nonlocal_return").unwrap();
    if !matches!(&method.signature, Some(Ty::Fn { ret, .. }) if **ret == Ty::Str) {
        failures.push(format!("nonlocal return must belong to method: {:?}", method.signature));
    }
    // Include-only modules are absent from LibraryClass today. Do not
    // fabricate framework semantics across that missing ancestry. The
    // native override returns String, but this probe does not claim the
    // separate include-only module inference/emission gap is supported.
    let transitive = app.library_classes.iter().find(|c| c.name.0.as_str() == "OverrideProbe").unwrap()
        .methods.iter().find(|m| m.name.as_str() == "transitive").unwrap();
    println!("unknown-ancestry override type: {:?}", transitive.body.ty);
    if transitive.body.ty == Some(Ty::Int) {
        failures.push("unknown-ancestry override was incorrectly typed from its block".to_string());
    }
    let before = ruby::emit_expr(&transitive.body);
    roundhouse::lower::current_set::apply_current_set_lowering(&mut app);
    let after = app.library_classes.iter().find(|c| c.name.0.as_str() == "OverrideProbe").unwrap()
        .methods.iter().find(|m| m.name.as_str() == "transitive").unwrap();
    if ruby::emit_expr(&after.body) != before {
        failures.push("unknown-ancestry override must remain its original call".to_string());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
