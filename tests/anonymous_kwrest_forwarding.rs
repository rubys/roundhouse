//! `def f(**); g(**); end` — anonymous keyword-rest forwarding. Prism
//! reports the parameter as a `KeywordRestParameterNode` with an empty
//! `name()`, and the declaration side used to silently drop it, while
//! the call-site bare `**` (an `AssocSplatNode` with no value) was
//! ledgered as "anonymous `**` keyword forwarding not yet supported".
//! Both sides now synthesize the same `__fwd_kwargs` binding name
//! `pr/argument-forwarding`'s `...` desugar uses, so the two ends
//! agree without a dedicated "anonymous kwrest" IR shape.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::Param;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

#[test]
fn anonymous_kwrest_param_and_forward_share_the_synthesized_binding() {
    let app = ingest_app_from_tree(tree(&[(
        "app/services/widget.rb",
        "class Widget\n  def build\n    other\n  end\n\n  def other(**opts)\n    opts\n  end\nend\n",
    )]))
    .expect("ingest");
    // Sanity: the fixture ingests at all (a canary the anonymous form
    // alone wouldn't need, but keeps this test from silently no-op'ing
    // if `Widget` stops resolving under a future refactor).
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "Widget"));

    let app = ingest_app_from_tree(tree(&[(
        "app/services/anon_widget.rb",
        "class AnonWidget\n  def build(**)\n    other(**)\n  end\nend\n",
    )]))
    .expect("ingest anonymous forwarding");
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "AnonWidget")
        .expect("AnonWidget class");
    let build = class.methods.iter().find(|m| m.name.as_str() == "build").expect("build method");

    // Declaration side: the anonymous `**` synthesizes a `__fwd_kwargs`
    // parameter (flattened to a defaulted positional, `from_kwrest`,
    // the same shape a NAMED `**opts` gets when nothing else in the
    // def keeps the keyword group — see the `keeps_keywords` gate in
    // `ingest_library_method`) rather than being dropped.
    let kwrest = build
        .params
        .iter()
        .find(|p: &&Param| p.from_kwrest)
        .unwrap_or_else(|| panic!("expected a synthesized kwrest param, got {:?}", build.params));
    assert_eq!(kwrest.name.as_str(), "__fwd_kwargs");

    // Call side: the bare `**` forward reads that same binding rather
    // than failing ingest.
    match &*build.body.node {
        ExprNode::Send { args, .. } => {
            assert_eq!(args.len(), 1);
            match &*args[0].node {
                ExprNode::Var { name, .. } => assert_eq!(name.as_str(), "__fwd_kwargs"),
                other => panic!("expected a __fwd_kwargs Var, got {other:?}"),
            }
        }
        other => panic!("expected a Send body, got {other:?}"),
    }
}
