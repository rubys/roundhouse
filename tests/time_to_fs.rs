//! `Time#to_fs(:format)` → what the format is defined as
//! (`lower::time_current`).
//!
//! A hole we opened ourselves: analyze TYPES `to_fs` and the `direct`
//! URL-helper lowering EMITS it, while no target implements it. Rails
//! resolves it through `Time::DATE_FORMATS` — part built-in strftime
//! strings, part app-defined lambdas from an initializer — and both
//! halves are compile-time knowledge.
//!
//! The built-in strings here were MEASURED against Rails 8.1
//! (`Time.utc(2026,8,14,9,5,3).to_fs(:number)` → `"20260814090503"`),
//! not transcribed.

use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

const EPOCH_INITIALIZER: &str =
    "Time::DATE_FORMATS[:epoch] = ->(time) { (time.to_f * 1000).to_i }\n";

/// A model whose one method body is `<expr>`, plus whatever extra files
/// the case needs.
fn app_with(body: &str, extra: Vec<(&str, &str)>) -> App {
    let mut tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = vec![(
        std::path::PathBuf::from("app/models/thing.rb"),
        format!("class Thing < ApplicationRecord\n  def stamp\n    {body}\n  end\nend\n")
            .as_bytes()
            .to_vec(),
    )]
    .into_iter()
    .collect();
    for (path, source) in extra {
        tree.insert(std::path::PathBuf::from(path), source.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

/// The lowered body of `Thing#stamp`, rendered as source-ish text.
fn stamp(body: &str, extra: Vec<(&str, &str)>) -> String {
    let app = app_with(body, extra);
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Thing").expect("Thing");
    for item in &model.body {
        if let roundhouse::dialect::ModelBodyItem::Method { method, .. } = item {
            if method.name.as_str() == "stamp" {
                return render(&method.body);
            }
        }
    }
    panic!("no stamp method");
}

fn render(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Send { recv, method, args, .. } => {
            let recv = recv.as_ref().map(|r| format!("{}.", render(r))).unwrap_or_default();
            let args: Vec<String> = args.iter().map(render).collect();
            if args.is_empty() {
                format!("{recv}{}", method.as_str())
            } else {
                format!("{recv}{}({})", method.as_str(), args.join(", "))
            }
        }
        ExprNode::Lit { value } => match value {
            roundhouse::expr::Literal::Str { value } => format!("{value:?}"),
            roundhouse::expr::Literal::Sym { value } => format!(":{}", value.as_str()),
            other => format!("{other:?}"),
        },
        ExprNode::Ivar { name } => format!("@{}", name.as_str()),
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        ExprNode::SelfRef => "self".to_string(),
        other => format!("{other:?}"),
    }
}

/// Rails' `:number` is `%Y%m%d%H%M%S` — the format campfire's
/// `direct :fresh_account_logo` cache-buster is built from.
#[test]
fn a_builtin_format_becomes_the_strftime_it_is_defined_as() {
    assert_eq!(
        stamp("created_at.to_fs(:number)", vec![]),
        "created_at.strftime(\"%Y%m%d%H%M%S\")"
    );
    assert_eq!(
        stamp("created_at.to_fs(:db)", vec![]),
        "created_at.strftime(\"%Y-%m-%d %H:%M:%S\")"
    );
}

/// `to_formatted_s` is `to_fs`' older spelling, and Rails still ships
/// both names for the same method.
#[test]
fn the_older_spelling_lowers_identically() {
    assert_eq!(
        stamp("created_at.to_formatted_s(:number)", vec![]),
        "created_at.strftime(\"%Y%m%d%H%M%S\")"
    );
}

/// An app-defined format is the lambda from its initializer, inlined
/// with the receiver substituted for the parameter — campfire's
/// `:epoch`, the millisecond stamp its message list sorts by.
#[test]
fn an_app_defined_format_inlines_its_initializer_lambda() {
    assert_eq!(
        stamp(
            "created_at.to_fs(:epoch)",
            vec![("config/initializers/time_formats.rb", EPOCH_INITIALIZER)]
        ),
        "created_at.to_f.*(Int { value: 1000 }).to_i.to_s"
    );
}

/// The app's definition wins over a built-in of the same name, as it
/// does in Rails — the initializer assigns into that very hash.
#[test]
fn an_app_definition_overrides_a_builtin_of_the_same_name() {
    assert_eq!(
        stamp(
            "created_at.to_fs(:number)",
            vec![(
                "config/initializers/time_formats.rb",
                "Time::DATE_FORMATS[:number] = ->(t) { t.to_i }\n"
            )]
        ),
        "created_at.to_i.to_s"
    );
}

/// An unmeasured format is LEFT ALONE, loudly. Rails' own fallback is
/// `to_s`, and emitting that would render a readable date where the app
/// asked for a format — "we do not know this format" is not the same
/// fact as "Rails does not know it".
#[test]
fn an_unknown_format_declines_rather_than_falling_back_to_to_s() {
    assert_eq!(
        stamp("created_at.to_fs(:rfc822)", vec![]),
        "created_at.to_fs(:rfc822)"
    );
}

/// Bare `to_fs` IS `to_s`, which every target answers — nothing to do.
#[test]
fn the_no_argument_form_is_left_alone() {
    assert_eq!(stamp("created_at.to_fs", vec![]), "created_at.to_fs");
}

/// The one place `to_fs` is emitted BY US rather than written by the
/// app: a `direct` helper's query value. Those bodies are synthesized
/// during route lowering, after the post-analyze hook has run, so they
/// take the rewrite on their own way out.
#[test]
fn a_direct_helpers_body_is_lowered_too() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = vec![
        (
            std::path::PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :things, only: :show\n  \
              direct :fresh_thing do |thing, options|\n    \
              route_for :thing, thing, v: thing.updated_at.to_fs(:number)\n  end\nend\n"
                .to_vec(),
        ),
        (
            std::path::PathBuf::from("app/models/thing.rb"),
            b"class Thing < ApplicationRecord\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let funcs = roundhouse::lower::routes_to_library::lower_routes_to_library_functions(&app);
    let f = funcs
        .iter()
        .find(|f| f.name.as_str() == "fresh_thing_path")
        .expect("fresh_thing_path");
    let body = format!("{:?}", f.body);
    assert!(body.contains("strftime"), "expected the format expanded, got: {body}");
    assert!(!body.contains("to_fs"), "expected no surviving to_fs, got: {body}");
}
