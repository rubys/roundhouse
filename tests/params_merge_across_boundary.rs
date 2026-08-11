//! `<params>.merge(k: v)` written a method and a class away from the
//! permit chain — campfire's first-run signup.
//!
//! `FirstRunsController#create` hands its `user_params` to
//! `FirstRun.create!`, whose body does `User.new(user_params.merge(role:
//! :administrator))`. The syntactic merge recognition only reaches a
//! merge written directly on the permit chain, so `role` never joined
//! the field set and the emit called `UserParams#merge`, which doesn't
//! exist.
//!
//! Widening the params class is NOT the fix: `UserParams` is shared with
//! the plain public signup controller, so putting `role` in its field
//! list would let a signup form POST `user[role]=administrator`. The
//! merge stays call-site local — assigned on the model after
//! construction.

use roundhouse::app::App;
use roundhouse::expr::{Expr, ExprNode};

fn app_from(files: Vec<(&str, &str)>) -> App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n    t.string :email_address\n    \
    t.integer :role\n  end\nend\n";

/// campfire's shape: a controller helper, a plain (non-model) class that
/// receives it, and the merge inside that class.
fn first_run_app(merge_key: &str) -> App {
    let builder = format!(
        r#"class FirstRun
  def self.create!(user_params)
    room = Room.new
    administrator = room.creator = User.new(user_params.merge({merge_key}: 1))
    administrator
  end
end
"#
    );
    app_from(vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/models/first_run.rb", Box::leak(builder.into_boxed_str())),
        (
            "app/controllers/first_runs_controller.rb",
            r#"class FirstRunsController < ApplicationController
  def create
    user = FirstRun.create!(user_params)
    user
  end

  private
    def user_params
      params.require(:user).permit(:name, :email_address)
    end
end
"#,
        ),
    ])
}

/// The `FirstRun.create!` body, as a list of statement renderings.
fn create_bang_statements(app: &App) -> Vec<String> {
    let lc = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "FirstRun")
        .expect("FirstRun ingested as a library class");
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "create!")
        .expect("FirstRun.create!");
    match &*m.body.node {
        ExprNode::Seq { exprs } => exprs.iter().map(render).collect(),
        _ => vec![render(&m.body)],
    }
}

/// Enough of an expression's shape to assert on, without depending on
/// any emitter.
fn render(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Assign { target, value } => {
            format!("{} = {}", render_lvalue(target), render(value))
        }
        ExprNode::Send { recv, method, args, .. } => {
            let recv = recv.as_ref().map(|r| format!("{}.", render(r))).unwrap_or_default();
            let args: Vec<String> = args.iter().map(render).collect();
            format!("{recv}{}({})", method.as_str(), args.join(", "))
        }
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Lit { value } => format!("{value:?}"),
        ExprNode::Seq { exprs } => {
            format!("SEQ[{}]", exprs.iter().map(render).collect::<Vec<_>>().join("; "))
        }
        other => format!("{other:?}"),
    }
}

fn render_lvalue(l: &roundhouse::expr::LValue) -> String {
    use roundhouse::expr::LValue;
    match l {
        LValue::Var { name, .. } => name.as_str().to_string(),
        LValue::Ivar { name } => format!("@{}", name.as_str()),
        LValue::Attr { recv, name } => format!("{}.{}", render(recv), name.as_str()),
        other => format!("{other:?}"),
    }
}

#[test]
fn the_merge_becomes_a_factory_call_plus_setters_hoisted_above_the_statement() {
    let mut app = first_run_app("role");
    roundhouse::session::analyze_and_lower(&mut app);
    let stmts = create_bang_statements(&app);

    // The construction and the merged-key setter are their own
    // statements, ABOVE the statement that consumed the value. A `Seq`
    // left in the expression slot would render as newline-joined lines
    // and bind `administrator` to the pre-setter value.
    assert!(
        stmts.contains(&"_pm0 = User.from_params(user_params)".to_string()),
        "expected a hoisted factory call, got {stmts:#?}"
    );
    assert!(
        stmts.iter().any(|s| s.starts_with("_pm0.role = ")),
        "expected a hoisted `role` setter, got {stmts:#?}"
    );
    assert!(
        stmts.contains(&"administrator = room.creator=(_pm0)".to_string()),
        "the original statement should now read the temp, got {stmts:#?}"
    );
    assert!(
        !stmts.iter().any(|s| s.contains("SEQ[")),
        "no statement list may be left in expression position: {stmts:#?}"
    );
    assert!(
        !stmts.iter().any(|s| s.contains("merge")),
        "the `merge` send must be gone — the params class has no such method: {stmts:#?}"
    );
}

#[test]
fn the_proven_parameter_type_is_stamped_on_the_signature() {
    let mut app = first_run_app("role");
    roundhouse::session::analyze_and_lower(&mut app);
    let lc = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "FirstRun")
        .expect("FirstRun");
    let m = lc.methods.iter().find(|m| m.name.as_str() == "create!").expect("create!");
    // Required, not decorative: the rewritten body passes this parameter
    // to a `UserParams`-typed factory, which an `untyped` can't satisfy
    // on a strict target.
    let sig = format!("{:?}", m.signature.as_ref().expect("signature synthesized"));
    assert!(sig.contains("UserParams"), "expected UserParams in {sig}");
}

#[test]
fn the_merged_key_does_not_join_the_permitted_field_set() {
    let mut app = first_run_app("role");
    roundhouse::session::analyze_and_lower(&mut app);
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    let spec = specs
        .canonical(&roundhouse::Symbol::from("user"))
        .expect("a :user spec");
    let fields: Vec<&str> = spec.fields.iter().map(|f| f.as_str()).collect();
    // `UserParams.from_raw` reads exactly these keys off the request.
    // `role` joining them is privilege escalation, not a convenience.
    assert_eq!(fields, vec!["name", "email_address"]);
}

#[test]
fn a_merged_key_with_no_writer_declines_and_is_ledgered() {
    // `nickname` is not a column and the model defines no `nickname=`.
    let mut app = first_run_app("nickname");
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let stmts = create_bang_statements(&app);
    assert!(
        stmts.iter().any(|s| s.contains("merge")),
        "source shape should be left in place, got {stmts:#?}"
    );
    assert!(
        diags.iter().any(|d| d.message.contains("params class has no `merge`")),
        "expected a residue diagnostic naming the gap, got {:#?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn a_disagreeing_call_site_poisons_the_binding() {
    // Two controllers call `FirstRun.create!` at the same position, one
    // with a params helper and one with a plain Hash. The parameter
    // isn't uniformly a params object, so nothing is rewritten.
    let mut app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/models/first_run.rb",
            r#"class FirstRun
  def self.create!(user_params)
    User.new(user_params.merge(role: 1))
  end
end
"#,
        ),
        (
            "app/controllers/first_runs_controller.rb",
            r#"class FirstRunsController < ApplicationController
  def create
    user = FirstRun.create!(user_params)
    user
  end

  private
    def user_params
      params.require(:user).permit(:name, :email_address)
    end
end
"#,
        ),
        (
            "app/controllers/imports_controller.rb",
            r#"class ImportsController < ApplicationController
  def create
    user = FirstRun.create!({ name: "seed" })
    user
  end
end
"#,
        ),
    ]);
    roundhouse::session::analyze_and_lower(&mut app);
    let stmts = create_bang_statements(&app);
    assert!(
        stmts.iter().any(|s| s.contains("merge")),
        "a disagreeing call site must leave the source shape alone, got {stmts:#?}"
    );
}
