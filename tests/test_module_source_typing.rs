//! Source test bodies must be typed before any emission-only cloning.
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::{App, Expr, ExprNode, Ty};
use std::path::PathBuf;

fn ingest(source: &str) -> App {
    ingest_app_from_tree(
        [(
            PathBuf::from("test/models/source_probe_test.rb"),
            source.as_bytes().to_vec(),
        )]
        .into_iter()
        .collect(),
    )
    .expect("source test module")
}

fn each_receivers(body: &Expr, out: &mut Vec<Option<Ty>>) {
    if let ExprNode::Send {
        recv: Some(recv),
        method,
        ..
    } = &*body.node
    {
        if method.as_str() == "each" {
            out.push(recv.ty.clone());
        }
    }
    body.node
        .for_each_child(&mut |child| each_receivers(child, out));
}

#[test]
fn original_test_helpers_converge_without_rewriting_keyword_forwarding() {
    // Caller before callees: one pass or declaration-order inference cannot
    // establish this chain. Array shape does not require guessing its values.
    let mut app = ingest(
        r#"
class SourceProbeTest < ActiveSupport::TestCase
  test "nested original assertions" do
    ["first", "second"].each do |value|
      values_from(value: value).each do |inner|
        assert_equal value, inner
      end
    end
    values_from(value: "last").each { |inner| assert_equal "last", inner }
  end
  def values_from(**details)
    values_for(**details).map(&:to_s)
  end
  def values_for(**details)
    [value_for(**details), value_for(**details)]
  end
  def value_for(value:)
    value
  end
end
"#,
    );
    let before = app.test_modules[0].clone();
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let module = &app.test_modules[0];
    let mut receivers = Vec::new();
    each_receivers(&module.tests[0].body, &mut receivers);
    assert_eq!(receivers.len(), 3);
    assert!(
        receivers
            .iter()
            .all(|ty| matches!(ty, Some(Ty::Array { .. }))),
        "original test each receivers, not lowered clones: {receivers:?}"
    );
    assert_eq!(module.tests[0].body.span, before.tests[0].body.span);
    let shapes = |params: &[roundhouse::dialect::Param]| {
        params
            .iter()
            .map(|p| (p.name.clone(), p.ty_kind(), p.from_kwrest, p.from_keyword))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shapes(&module.helpers[0].params),
        shapes(&before.helpers[0].params)
    );
    // The kwsplat repair owns receiverless source calls; annotating self here
    // would prevent it from recognizing the very forwarding we just typed.
    let mut bare_forwarding = false;
    fn find_bare(expr: &Expr, found: &mut bool) {
        if let ExprNode::Send {
            recv: None, method, ..
        } = &*expr.node
        {
            *found |= method.as_str() == "values_for";
        }
        expr.node
            .for_each_child(&mut |child| find_bare(child, found));
    }
    find_bare(&module.helpers[0].body, &mut bare_forwarding);
    assert!(
        bare_forwarding,
        "source calls must not acquire synthetic self"
    );
    assert!(
        matches!(module.helpers[0].signature, Some(Ty::Fn { ref ret, .. })
        if matches!(&**ret, Ty::Array { .. })),
        "converged helper signature"
    );
}

#[test]
fn setup_ivars_flow_to_tests_but_setup_locals_do_not() {
    let mut app = ingest(
        r#"
class SourceProbeTest < ActiveSupport::TestCase
  setup do
    @values = [3, 7]
    setup_only = [11, 19]
  end
  test "setup scope" do
    @values.each { |value| value + 2 }
    setup_only.each { |value| value }
  end
  def independent(value = "default")
    value
  end
end
"#,
    );
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let module = &app.test_modules[0];
    assert!(module.setup.as_ref().unwrap().ty.is_some());
    let mut receivers = Vec::new();
    each_receivers(&module.tests[0].body, &mut receivers);
    assert_eq!(
        receivers[0],
        Some(Ty::Array {
            elem: Box::new(Ty::Int)
        })
    );
    assert!(
        !matches!(receivers[1], Some(Ty::Array { .. })),
        "setup locals are another Ruby scope"
    );
    assert_eq!(
        module.helpers[0].body.ty,
        Some(Ty::Str),
        "real parameter default seeds its own method"
    );
}

#[test]
fn fixture_accessors_use_loader_identity_and_preserve_helper_shadowing() {
    let mut app = ingest_app_from_tree(
        [
            (
                "app/models/article.rb",
                "class Article < ApplicationRecord; end",
            ),
            (
                "app/models/push/subscription.rb",
                "class Push::Subscription < ApplicationRecord; end",
            ),
            // A flattened-name guess would resolve, but to the WRONG class.
            (
                "app/models/push_subscription.rb",
                "class PushSubscription < ApplicationRecord; end",
            ),
            ("test/fixtures/push/subscriptions.yml", "one:\n  id: 1\n"),
            (
                "test/fixtures/overrides.yml",
                "_fixture:\n  model_class: ::Article\none:\n  id: 2\n",
            ),
            ("test/fixtures/articles.yml", "one:\n  id: 3\n"),
            ("test/fixtures/ghosts.yml", "one:\n  id: 4\n"),
            (
                "test/models/source_probe_test.rb",
                r#"
class SourceProbeTest < ActiveSupport::TestCase
  test "fixture identity" do
    push_subscriptions(:one)
    overrides(:one)
    articles(:one)
    ghosts(:one)
  end
  def articles(label)
    "own helper"
  end
end
"#,
            ),
        ]
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect(),
    )
    .unwrap();
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let ExprNode::Seq { exprs } = &*app.test_modules[0].tests[0].body.node else {
        panic!("original test statements");
    };
    let class_ty = |name: &str| {
        Some(Ty::Class {
            id: roundhouse::ident::ClassId(name.into()),
            args: vec![],
        })
    };
    assert_eq!(exprs[0].ty, class_ty("Push::Subscription"));
    assert_eq!(exprs[1].ty, class_ty("Article"));
    assert_eq!(
        exprs[2].ty,
        Some(Ty::Str),
        "real helper overrides fixture accessor"
    );
    assert_eq!(
        exprs[3].ty,
        Some(Ty::Untyped),
        "unknown model stays gradual"
    );
    let lowered = roundhouse::lower::fixtures::lower_fixtures(&app);
    for (name, expected) in [
        ("push_subscriptions", "Push::Subscription"),
        ("overrides", "Article"),
    ] {
        let fixture = lowered
            .fixtures
            .iter()
            .find(|fixture| fixture.name.as_str() == name)
            .unwrap();
        assert_eq!(
            fixture.class.0.as_str(),
            expected,
            "loader identity for {name}"
        );
    }
}

#[test]
fn inherited_helpers_and_declared_parameter_types_use_the_source_registry() {
    let mut app = ingest(
        r#"
class BaseProbeTest < ActiveSupport::TestCase
  def values
    [5, 11]
  end
end
class SourceProbeTest < BaseProbeTest
  test "inherited source helper" do
    self.values.each { |value| declared(value) }
  end
  def declared(value)
    value + 1
  end
end
"#,
    );
    let child = app
        .test_modules
        .iter_mut()
        .find(|module| module.name.0.as_str() == "SourceProbeTest")
        .unwrap();
    child.helpers[0].signature = Some(Ty::Fn {
        params: vec![roundhouse::ty::Param {
            name: "value".into(),
            ty: Ty::Int,
            kind: roundhouse::ty::ParamKind::Required,
        }],
        block: None,
        ret: Box::new(Ty::Int),
        effects: roundhouse::effect::EffectSet::pure(),
    });
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let child = app
        .test_modules
        .iter()
        .find(|module| module.name.0.as_str() == "SourceProbeTest")
        .unwrap();
    let mut receivers = Vec::new();
    each_receivers(&child.tests[0].body, &mut receivers);
    assert_eq!(
        receivers,
        vec![Some(Ty::Array {
            elem: Box::new(Ty::Int)
        })]
    );
    assert_eq!(child.helpers[0].body.ty, Some(Ty::Int));
    assert!(matches!(child.helpers[0].signature, Some(Ty::Fn { ref ret, .. }) if **ret == Ty::Int));
}

#[test]
fn helper_defaults_see_preceding_parameter_bindings() {
    let mut app = ingest(
        r#"
class SourceProbeTest < ActiveSupport::TestCase
  test "dependent default" do
    normalized("first")
  end
  def normalized(value, upper = upper_for(value), values = [upper])
    values
  end
  def upper_for(input)
    input.upcase
  end
end
"#,
    );
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let helper = &app.test_modules[0].helpers[0];
    assert_eq!(helper.params[1].default.as_ref().unwrap().ty, Some(Ty::Str));
    let strings = Ty::Array {
        elem: Box::new(Ty::Str),
    };
    assert_eq!(
        helper.params[2].default.as_ref().unwrap().ty,
        Some(strings.clone())
    );
    assert_eq!(helper.body.ty, Some(strings));
    assert_eq!(
        app.test_modules[0].helpers[1].body.ty,
        Some(Ty::Str),
        "default call sites feed the real helper's parameter inference"
    );
}

#[test]
fn forwarded_keyword_hash_does_not_replace_positional_or_named_observations() {
    let mut app = ingest(
        r#"
class SourceProbeTest < ActiveSupport::TestCase
  test "mixed forwarding" do
    forwarding(13, value: "forwarded")
    target(17, value: "direct")
  end
  def forwarding(number, **details)
    target(number, **details)
  end
  def target(number, value:)
    [number, value]
  end
end
"#,
    );
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let helper = &app.test_modules[0].helpers[1];
    let Some(Ty::Fn { params, .. }) = &helper.signature else {
        panic!("target helper signature: {:?}", helper.signature);
    };
    assert_eq!(
        params[0].ty,
        Ty::Int,
        "real positional sites survive forwarding"
    );
    assert_eq!(
        params[1].ty,
        Ty::Str,
        "keyword hash is not the keyword value"
    );
}

#[test]
fn forwarded_keyword_hash_still_binds_to_a_positional_hash_parameter() {
    let mut app = ingest(
        r#"
class SourceProbeTest < ActiveSupport::TestCase
  test "positional hash control" do
    forwarding({ value: "kept" })
  end
  def forwarding(details)
    target(**details)
  end
  def target(payload)
    payload
  end
end
"#,
    );
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let helper = &app.test_modules[0].helpers[1];
    let hash = Ty::Hash {
        key: Box::new(Ty::Sym),
        value: Box::new(Ty::Str),
    };
    assert_eq!(helper.body.ty, Some(hash.clone()));
    let Some(Ty::Fn { params, .. }) = &helper.signature else {
        panic!("positional Hash helper signature: {:?}", helper.signature);
    };
    assert_eq!(params[0].ty, hash);
}

#[test]
fn keyword_rest_binds_the_whole_hash_beside_positional_rest() {
    let mut app = ingest(
        r#"
class SourceProbeTest < ActiveSupport::TestCase
  test "keyword rest control" do
    details = { value: "kept" }
    target(13, **details)
  end
  def target(*numbers, **details)
    details
  end
end
"#,
    );
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let helper = &app.test_modules[0].helpers[0];
    assert_eq!(helper.params[0].ty_kind(), roundhouse::ty::ParamKind::Rest);
    assert_eq!(
        helper.params[1].ty_kind(),
        roundhouse::ty::ParamKind::KeywordRest
    );
    assert_eq!(
        helper.body.ty,
        Some(Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Str),
        }),
        "keyword-rest is a real Hash binding, not an individual named keyword"
    );
}

#[test]
fn inherited_helper_call_sites_belong_to_the_defining_test_class() {
    let mut app = ingest(
        r#"
class BaseProbeTest < ActiveSupport::TestCase
  def values(value:)
    [value.upcase]
  end
end
class SourceProbeTest < BaseProbeTest
  test "inherited parameter" do
    values(value: "first").each { |value| assert_equal "FIRST", value }
  end
end
"#,
    );
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let base = app
        .test_modules
        .iter()
        .find(|module| module.name.0.as_str() == "BaseProbeTest")
        .unwrap();
    assert_eq!(
        base.helpers[0].body.ty,
        Some(Ty::Array {
            elem: Box::new(Ty::Str)
        })
    );
    let key = (base.name.clone(), "values".into());
    assert_eq!(app.inferred_method_params.get(&key), Some(&vec![Ty::Str]));
    let child = app
        .test_modules
        .iter()
        .find(|module| module.name.0.as_str() == "SourceProbeTest")
        .unwrap();
    assert!(
        !app.inferred_method_params
            .contains_key(&(child.name.clone(), "values".into()))
    );
}

#[test]
fn test_helper_observations_do_not_flow_into_production_concerns() {
    // Own def wins over the included concern; moving the include to the
    // child instead makes the concern shadow the inherited helper.
    for include_on_child in [false, true] {
        let source = format!(
            r#"
class BaseProbeTest < ActiveSupport::TestCase
  {}
  def rows(records)
    records
  end
end
class SourceProbeTest < BaseProbeTest
  {}
  test "ownership boundary" do
    rows(["test-only"])
  end
end
"#,
            if include_on_child {
                ""
            } else {
                "include ProductionConcern"
            },
            if include_on_child {
                "include ProductionConcern"
            } else {
                ""
            }
        );
        let mut app = ingest_app_from_tree([
            ("app/models/concerns/production_concern.rb", "module ProductionConcern\n  def rows(records)\n    records\n  end\nend".to_string()),
            ("app/models/user.rb", "class User < ApplicationRecord; end".to_string()),
            ("app/models/reader.rb", "class Reader < ApplicationRecord\n  include ProductionConcern\n  def read\n    rows(User.where(id: [3, 7]))\n  end\nend".to_string()),
            ("test/models/source_probe_test.rb", source),
        ].into_iter().map(|(path, source)| (PathBuf::from(path), source.into_bytes())).collect()).unwrap();
        roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
        let key = (
            roundhouse::ident::ClassId("ProductionConcern".into()),
            "rows".into(),
        );
        assert_eq!(
            app.inferred_method_params.get(&key),
            Some(&vec![Ty::Relation {
                of: roundhouse::ident::ClassId("User".into())
            }]),
            "production concern shape, include_on_child={include_on_child}"
        );
        let key = (
            roundhouse::ident::ClassId("BaseProbeTest".into()),
            "rows".into(),
        );
        if include_on_child {
            assert!(
                !app.inferred_method_params.contains_key(&key),
                "nearer included production method shadows ancestor test helper"
            );
        } else {
            assert_eq!(
                app.inferred_method_params.get(&key),
                Some(&vec![Ty::Array {
                    elem: Box::new(Ty::Str)
                }])
            );
        }
    }
}
