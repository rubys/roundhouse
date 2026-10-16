//! `lower::model_to_library::resolve_model_aliases`: snapshot
//! semantics and the refusal shapes. Each refusal is checked twice —
//! once as the `Error`-severity diagnostic strict mode gates on, once
//! after `attribute_analysis_gaps`'s survey-mode demotion, which is
//! exactly how `roundhouse`/`roundhouse-check` tell the two modes
//! apart (see `ModelBodyItem::Alias` and `resolve_model_aliases`'s own
//! docs for the full design).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::attribution::attribute_analysis_gaps;
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "gadgets", force: :cascade do |t|
    t.integer "widget_id"
  end
end
"#;

/// Build the app, lower `Widget`, and return (resolved library class,
/// the diagnostics from lowering, a fresh gaps vec after survey-mode
/// attribution runs over them).
fn lower_widget(source: &str) -> (roundhouse::App, roundhouse::dialect::LibraryClass, Vec<roundhouse::diagnostic::Diagnostic>) {
    lower_model("Widget", &[("app/models/widget.rb", source)])
}

/// Same as `lower_widget`, but for any named model — the snapshot
/// test needs `Gadget` (a `belongs_to`, unlike `Widget`'s plain
/// columns).
fn lower_model(
    name: &str,
    files: &[(&str, &str)],
) -> (roundhouse::App, roundhouse::dialect::LibraryClass, Vec<roundhouse::diagnostic::Diagnostic>) {
    let mut all = vec![("db/schema.rb", SCHEMA)];
    all.extend_from_slice(files);
    let app = ingest_app_from_tree(tree(&all))
        .expect("ingest must succeed — the alias is deferred to lowering now");
    let model = app.models.iter().find(|m| m.name.0.as_str() == name).unwrap_or_else(|| panic!("missing {name}")).clone();
    let (lc, diags) =
        roundhouse::emit::diagnostics::scope(|| lower_model_to_library_class(&model, &app.schema));
    (app, lc, diags)
}

fn alias_unresolved_diags(diags: &[roundhouse::diagnostic::Diagnostic]) -> Vec<&roundhouse::diagnostic::Diagnostic> {
    diags
        .iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "alias_target_unresolved"))
        .collect()
}

/// **Snapshot test**: an alias copies the target's body AS DEFINED AT
/// THAT LINE. `widget` starts as a `belongs_to` reader (a source only
/// this PR can resolve — the ingest-time walk could never see it, so
/// the whole scenario fails before this PR, not just the "after"
/// half), then a hand-written `def` overrides it; one alias brackets
/// each side of that redefinition.
#[test]
fn an_alias_copies_the_targets_body_as_of_its_own_line() {
    let (_app, lc, diags) = lower_model(
        "Gadget",
        &[(
            "app/models/gadget.rb",
            "class Gadget < ApplicationRecord\n  belongs_to :widget, optional: true\n  alias owner_before widget\n  def widget\n    \"overridden\"\n  end\n  alias owner_after widget\nend\n",
        )],
    );
    assert!(alias_unresolved_diags(&diags).is_empty(), "must resolve: {diags:?}");
    let find = |name: &str| {
        lc.methods.iter().find(|m| m.name.as_str() == name).unwrap_or_else(|| panic!("missing {name}"))
    };
    // Before the redefinition: the belongs_to's own synthesized
    // reader — whatever shape that is, it is NOT the literal override.
    assert!(
        !matches!(&*find("owner_before").body.node,
            roundhouse::expr::ExprNode::Lit { value: roundhouse::expr::Literal::Str { value } } if value == "overridden"),
        "alias BEFORE the redefinition must keep the belongs_to's own body, not the override: {:?}",
        find("owner_before").body
    );
    // After the redefinition: the hand-written override.
    assert!(
        matches!(&*find("owner_after").body.node,
            roundhouse::expr::ExprNode::Lit { value: roundhouse::expr::Literal::Str { value } } if value == "overridden"),
        "alias AFTER the redefinition must see the new body: {:?}",
        find("owner_after").body
    );
}

/// **Refusal shape**: the target is declared only AFTER the alias
/// line — Ruby itself would raise NameError here (the name doesn't
/// exist yet when `alias` runs), so this must refuse, not resolve to
/// the later def.
#[test]
fn target_declared_after_the_alias_refuses() {
    let (app, lc, mut diags) = lower_widget(
        "class Widget < ApplicationRecord\n  alias greet greeting\n  def greeting\n    \"hello\"\n  end\nend\n",
    );
    let unresolved = alias_unresolved_diags(&diags);
    assert_eq!(unresolved.len(), 1, "exactly one gap: {diags:?}");
    assert_eq!(unresolved[0].severity, Severity::Error, "strict mode: an error");
    assert!(!lc.methods.iter().any(|m| m.name.as_str() == "greet"), "must not synthesize a partial alias");

    let mut gaps = Vec::new();
    attribute_analysis_gaps(&mut diags, &app, &mut gaps);
    assert_eq!(diags.iter().filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "alias_target_unresolved")).count(), 1);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "survey mode: demoted, not fatal");
    assert_eq!(gaps.len(), 1, "survey mode: recorded as a gap: {gaps:?}");
}

/// **Refusal shape**: a plain AR column reader. Rails itself raises
/// `NameError` here (attribute methods are generated lazily — that's
/// why `alias_attribute` exists), so roundhouse must refuse too,
/// naming the real fix in the message.
#[test]
fn a_column_alias_refuses_and_names_alias_attribute() {
    let (app, lc, mut diags) = lower_widget("class Widget < ApplicationRecord\n  alias headline name\nend\n");
    let unresolved = alias_unresolved_diags(&diags);
    assert_eq!(unresolved.len(), 1, "exactly one gap: {diags:?}");
    assert_eq!(unresolved[0].severity, Severity::Error, "strict mode: an error");
    assert!(unresolved[0].message.contains("alias_attribute"), "{}", unresolved[0].message);
    assert!(!lc.methods.iter().any(|m| m.name.as_str() == "headline"));

    let mut gaps = Vec::new();
    attribute_analysis_gaps(&mut diags, &app, &mut gaps);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "survey mode: demoted, not fatal");
    assert_eq!(gaps.len(), 1, "survey mode: recorded as a gap: {gaps:?}");
}

/// **Refusal shape**: an inherited AR target the model redefines
/// AFTER the alias. A forwarder would call whatever `destroy!` means
/// at RUN time, not at alias time — wrong for snapshot semantics — so
/// this refuses rather than guessing.
#[test]
fn an_inherited_target_redefined_after_the_alias_refuses() {
    let (app, lc, mut diags) = lower_widget(
        "class Widget < ApplicationRecord\n  alias reject! destroy!\n  def destroy!\n    \"local\"\n  end\nend\n",
    );
    let unresolved = alias_unresolved_diags(&diags);
    assert_eq!(unresolved.len(), 1, "exactly one gap: {diags:?}");
    assert_eq!(unresolved[0].severity, Severity::Error, "strict mode: an error");
    assert!(!lc.methods.iter().any(|m| m.name.as_str() == "reject!"), "must not synthesize a partial forwarder");

    let mut gaps = Vec::new();
    attribute_analysis_gaps(&mut diags, &app, &mut gaps);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "survey mode: demoted, not fatal");
    assert_eq!(gaps.len(), 1, "survey mode: recorded as a gap: {gaps:?}");
}

/// **Refusal shape**: a name defined nowhere at all — no hand-written
/// def, no enum/association/attr_accessor, no column, no known
/// ActiveRecord::Base method.
#[test]
fn a_name_defined_nowhere_refuses() {
    let (app, lc, mut diags) =
        lower_widget("class Widget < ApplicationRecord\n  alias bogus not_a_real_method_anywhere\nend\n");
    let unresolved = alias_unresolved_diags(&diags);
    assert_eq!(unresolved.len(), 1, "exactly one gap: {diags:?}");
    assert_eq!(unresolved[0].severity, Severity::Error, "strict mode: an error");
    assert!(!lc.methods.iter().any(|m| m.name.as_str() == "bogus"));

    let mut gaps = Vec::new();
    attribute_analysis_gaps(&mut diags, &app, &mut gaps);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "survey mode: demoted, not fatal");
    assert_eq!(gaps.len(), 1, "survey mode: recorded as a gap: {gaps:?}");
}
