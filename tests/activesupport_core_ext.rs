//! Small ActiveSupport surfaces that ordinary apps call. The return type
//! stays the existing duration or hash type. This does not add Date.

use roundhouse::analyze::Analyzer;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::ty::Ty;
use roundhouse::App;

fn body(source: &str) -> roundhouse::Expr {
    let classes = ingest_library_classes(source.as_bytes(), "ext.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes = classes;
    Analyzer::new(&app).analyze(&mut app);
    app.library_classes[0].methods[0].body.clone()
}

#[test]
fn index_with_returns_a_hash_and_durations_stay_typed() {
    let typed = body("class Probe\n  def values(items)\n    items.index_with { |item| item }\n    2.days\n    3.hours\n    4.minutes\n    1.to_d\n  end\nend\n");
    let debug = format!("{typed:?}");
    assert!(debug.contains("index_with"), "{debug}");
    assert!(matches!(typed.ty, Some(Ty::Class { .. }) | Some(Ty::Untyped) | Some(Ty::Int)), "{:?}", typed.ty);
    fn find<'a>(expr: &'a roundhouse::Expr, name: &str) -> Option<&'a roundhouse::Expr> {
        if let ExprNode::Send { method, .. } = &*expr.node {
            if method.as_str() == name {
                return Some(expr);
            }
        }
        let mut found = None;
        expr.node.for_each_child(&mut |child| {
            if found.is_none() {
                found = find(child, name);
            }
        });
        found
    }
    let indexed = body("class Probe\n  def values\n    [\"a\"].index_with { |item| item.length }\n  end\nend\n");
    let indexed_call = find(&indexed, "index_with").expect("index_with");
    assert!(matches!(indexed_call.ty, Some(Ty::Hash { .. })), "index_with: {:?}", indexed_call.ty);
    for name in ["days", "hours", "minutes"] {
        let call = find(&typed, name).expect(name);
        assert!(call.ty.is_some(), "{name} has no type");
    }
    let decimal = find(&typed, "to_d").expect("to_d");
    assert!(matches!(&decimal.ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "BigDecimal"), "{:?}", decimal.ty);
}
