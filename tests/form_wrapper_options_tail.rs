//! A form-wrapper helper whose tail is an OPTIONS HASH.
//!
//! `form_wrapper_helpers` splices a helper that is nothing but a
//! `form_with` with the block forwarded through, so the form-builder
//! macro-inline sees both halves at once. It declined campfire's
//! `Users::ProfilesHelper#profile_form_with(model, **params, &)` three
//! ways, and the helper emitted a bare `form_with` no module defines:
//!
//! * `**options` ingests as a trailing POSITIONAL defaulting to `{}`,
//!   so `profile_form_with @user` supplies one argument to a
//!   two-parameter wrapper and the arity check rejected it. The same
//!   template also writes `profile_form_with @user, class: "…"`, so
//!   both spellings have to work.
//! * A literal options hash is not a `Var`/`Ivar`/`Lit`, so the
//!   pure-read check rejected the call that DID have full arity. A hash
//!   literal of pure reads is built fresh at the call site —
//!   substituting it re-allocates rather than re-observes, which is the
//!   property that predicate is about.
//! * And once spliced, `lower::kwsplat` has already desugared the
//!   wrapper's body to `{…}.merge(params)`, where `form_with` wants ONE
//!   literal hash to walk. Given the chain it DECLINES — and declining
//!   after the splice is worse than declining before it: the call is
//!   gone and nothing replaces it, so the form silently vanishes from
//!   the page instead of failing loudly. Measured: three
//!   `profile_form_with` calls became three `io << ""`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::App;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// The FULL pipeline, not just `lower_view_to_library_class`: the
/// `**params` → `.merge` desugar is a post-analyze pass, so a harness
/// that skips those never sees the shape this fix is about.
fn app_with(call_tail: &str) -> App {
    app_with_helper(call_tail, r#"module ThingsHelper
  def thing_form_with(model, **params, &)
    form_with model: @thing, url: thing_path(model), method: :patch, **params, &
  end
end
"#)
}

fn app_with_helper(call_tail: &str, helper: &str) -> App {
    let template = format!(
        "<%= thing_form_with @thing{call_tail} do |form| %>\n  <p>hi</p>\n<% end %>\n"
    );
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :things\nend\n",
        ),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n"),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  def show\n    @thing = Thing.first\n  end\nend\n",
        ),
        ("app/helpers/things_helper.rb", helper),
        ("app/views/things/show.html.erb", Box::leak(template.into_boxed_str())),
    ]))
    .expect("ingest wrapper app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn show_view(call_tail: &str) -> String {
    let app = app_with(call_tail);
    let files = roundhouse::emit::ruby::emit_lowered_views(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("things/show.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no show view; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

/// Bare call — the options parameter binds its own `{}` default.
#[test]
fn a_call_with_no_options_binds_the_default() {
    let src = show_view("");
    assert!(src.contains("<form"), "the wrapper must splice to a real form:\n{src}");
    assert!(
        !src.contains("thing_form_with"),
        "no call to the un-spliced wrapper may survive:\n{src}"
    );
}

/// …and with a literal options hash, whose keys reach the form and
/// whose collisions WIN, which is Ruby's `merge` and the point of a
/// wrapper.
#[test]
fn a_call_with_an_options_hash_merges_and_overrides() {
    let src = show_view(", class: \"txt-medium\", method: :post");
    assert!(src.contains("<form"), "{src}");
    assert!(!src.contains("thing_form_with"), "{src}");
    assert!(src.contains("txt-medium"), "the caller's options reach the form:\n{src}");
    assert!(
        src.contains("form_method = :post"),
        "the caller's `method:` overrides the helper's `:patch`:\n{src}"
    );
}

/// The helper's body is read in the VIEW's context. campfire's
/// `profile_form_with(model, …)` ignores its own parameter and names
/// `@user`, which is legal because a Rails view helper runs against the
/// view — and the emitted view is a module function whose ivars became
/// parameters before the walker ran. Unrewritten, the spliced `@user`
/// stayed an ivar on a module function (nil), and every field in the
/// form died on `undefined method '[]' for nil`.
#[test]
fn the_helpers_own_ivars_read_as_the_views_locals() {
    let app = app_with("");
    let files = roundhouse::emit::ruby::emit_lowered_views(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("things/show.rb"))
        .map(|f| f.content.clone())
        .expect("show view");
    assert!(
        !src.contains("@thing"),
        "no ivar may survive into a view module function:\n{src}"
    );
}

/// The failure this fix is really about: never an EMPTY append where a
/// form belongs.
#[test]
fn the_form_is_never_silently_dropped() {
    for tail in ["", ", class: \"x\""] {
        let src = show_view(tail);
        assert!(
            !src.contains("io << \"\"\n      io\n"),
            "the whole tag vanished for `{tail}`:\n{src}"
        );
    }
}

#[test]
fn owner_bridges_keep_required_typed_parameters_and_source_spans() {
    use roundhouse::dialect::{MethodReceiver, MethodVisibility};
    use roundhouse::ty::{ParamKind, Ty};
    let mut app = app_with_helper("", r#"module ThingsHelper
  def thing_form_with(model, suffix = "!", &)
    form_with model: model, url: thing_path(model), data: { label: own_data(model, suffix), count: Thing.count }, &
  end
  private
  def own_data(model, suffix); model.name + suffix; end
end
"#);
    let helper = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "ThingsHelper").unwrap();
    let bridge = helper.methods.iter().find(|m| m.name.as_str().starts_with("__rh_form_")).unwrap();
    assert_eq!(bridge.receiver, MethodReceiver::Class);
    assert_eq!(bridge.visibility, MethodVisibility::Public);
    assert_eq!(bridge.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["model", "suffix"]);
    assert!(bridge.params.iter().all(|p| p.default.is_none()));
    assert!(bridge.block_param.is_none());
    assert!(!bridge.body.span.is_synthetic());
    let Some(Ty::Fn { params, block, ret, .. }) = &bridge.signature else { panic!("typed bridge") };
    assert!(params.iter().all(|p| p.kind == ParamKind::Required));
    assert_eq!(params[1].ty, Ty::Str);
    assert_eq!(bridge.body.ty.as_ref(), Some(ret.as_ref()));
    assert!(block.is_none());
    assert!(!app.helper_method_index.contains_key(&bridge.name));
    let wrapper = helper.methods.iter().find(|m| m.name.as_str() == "thing_form_with").unwrap();
    let body = match &*wrapper.body.node {
        roundhouse::expr::ExprNode::Seq { exprs } => &exprs[0],
        _ => &wrapper.body,
    };
    let roundhouse::expr::ExprNode::Send { args, .. } = &*body.node else { panic!("form call") };
    let roundhouse::expr::ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("kwargs") };
    let value = &entries.iter().find(|(k, _)| matches!(&*k.node,
        roundhouse::expr::ExprNode::Lit { value: roundhouse::expr::Literal::Sym { value } } if value.as_str() == "data"
    )).unwrap().1;
    assert!(!bridge.effects.is_pure(), "the nested DB read is effectful");
    assert_eq!(value.effects, bridge.effects, "the call must retain nested effects");
    let roundhouse::expr::ExprNode::Send { args, .. } = &*value.node else { panic!("bridge call") };
    assert_eq!(args.iter().map(|a| a.ty.as_ref()).collect::<Vec<_>>(), params.iter().map(|p| Some(&p.ty)).collect::<Vec<_>>());
    let before = app.library_classes.clone();
    roundhouse::session::analyze_and_lower(&mut app);
    let after = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "ThingsHelper").unwrap();
    let before = before.iter().find(|lc| lc.name == after.name).unwrap();
    assert_eq!(after.methods.iter().filter(|m| m.name.as_str().starts_with("__rh_form_")).count(), 1);
    assert_eq!(before.methods.len(), after.methods.len(), "do not synthesize bridges twice");
}

#[test]
fn owner_extraction_does_not_hide_model_syntax_or_executable_defaults() {
    for helper in [
        r#"module ThingsHelper
  def thing_form_with(model, &)
    form_with model: [:admin, thing], url: thing_path(model), &
  end
  def thing; @thing; end
end
"#,
        r#"module ThingsHelper
  def thing_form_with(model, suffix = own_suffix, &)
    form_with model: model, data: own_data(model, suffix), &
  end
  def own_suffix; "!"; end
  def own_data(model, suffix); { label: model.name + suffix }; end
end
"#,
        r#"module ThingsHelper
  def thing_form_with(model, &)
    form_with model: model, id: "form-#{own_label}", class: ["base", own_label], &
  end
  def own_label; "owner"; end
end
"#,
        r#"module ThingsHelper
  def thing_form_with(model, &)
    form_with model: model, url: thing_path(tmp = model), data: { controller: own_label, label: tmp.name }, &
  end
  def own_label; "owner"; end
end
"#,
        r#"module ThingsHelper
  def thing_form_with(model, &)
    form_with model: model, data: { controller: own_label, label: @thing.name }, &
  end
  def own_label; "owner"; end
end
"#,
    ] {
        let app = app_with_helper("", helper);
        assert!(app.library_classes.iter().flat_map(|lc| &lc.methods)
            .all(|m| !m.name.as_str().starts_with("__rh_form_")));
    }
}

#[test]
fn a_same_named_non_helper_wrapper_cannot_claim_a_view_call() {
    let mut app = app_with("");
    let mut decoy = app.library_classes.iter()
        .find(|lc| lc.name.0.as_str() == "ThingsHelper").unwrap().clone();
    decoy.name = roundhouse::ident::ClassId(roundhouse::ident::Symbol::from("UnrelatedLibrary"));
    let wrapper = decoy.methods.iter_mut().find(|m| m.name.as_str() == "thing_form_with").unwrap();
    // It looks like a wrapper, but has no binding for the view's argument.
    wrapper.params.clear();
    app.library_classes.push(decoy);
    let files = roundhouse::emit::ruby::emit_lowered_views(&app);
    let show = files.iter().find(|f| f.path.to_string_lossy().ends_with("things/show.rb")).unwrap();
    assert!(show.content.contains("<form"), "{}", show.content);
    assert!(!show.content.contains("UnrelatedLibrary"), "{}", show.content);
}
