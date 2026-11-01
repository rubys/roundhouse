//! DnsTestHelper::CONST must become bare CONST after splice and stay bare
//! through analyze + test-module lowering (campfire push-subscription floor).
use roundhouse::expr::ExprNode;

fn app() -> roundhouse::app::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "test/test_helper.rb",
            "class ActiveSupport::TestCase\n  include DnsTestHelper\nend\n",
        ),
        (
            "test/test_helpers/dns_test_helper.rb",
            "module DnsTestHelper\n  WEB_PUSH_PUBLIC_TEST_IP = \"1.2.3.4\"\n  def stub_x\n    WEB_PUSH_PUBLIC_TEST_IP\n  end\nend\n",
        ),
        (
            "test/models/push_test.rb",
            "class PushTest < ActiveSupport::TestCase\n  test \"ip\" do\n    DnsTestHelper::WEB_PUSH_PUBLIC_TEST_IP\n  end\nend\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

fn walk(e: &roundhouse::expr::Expr, out: &mut Vec<Vec<String>>) {
    if let ExprNode::Const { path } = &*e.node {
        out.push(path.iter().map(|s| s.as_str().to_string()).collect());
    }
    e.node.for_each_child(&mut |c| walk(c, out));
}

fn assert_bare_web_push(paths: &[Vec<String>], phase: &str) {
    assert!(
        !paths
            .iter()
            .any(|p| p.len() == 2 && p[0] == "DnsTestHelper"),
        "{phase} re-introduced DnsTestHelper qualification: {paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.as_slice() == ["WEB_PUSH_PUBLIC_TEST_IP"]),
        "{phase}: expected bare WEB_PUSH_PUBLIC_TEST_IP, got {paths:?}"
    );
}

#[test]
fn dns_helper_qualified_const_is_unqualified_on_the_test_body() {
    let app = app();
    let tm = app
        .test_modules
        .iter()
        .find(|t| t.name.0.as_str() == "PushTest")
        .expect("PushTest");
    assert!(
        tm.constants
            .iter()
            .any(|(n, _)| n.as_str() == "WEB_PUSH_PUBLIC_TEST_IP"),
        "expected spliced constant"
    );
    let mut paths = Vec::new();
    for t in &tm.tests {
        walk(&t.body, &mut paths);
    }
    assert_bare_web_push(&paths, "ingest");

    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
        &roundhouse::lower::routes::direct_helper_names(&app),
    );
    let mut lowered_paths = Vec::new();
    for m in lcs.iter().flat_map(|lc| lc.methods.iter()) {
        if m.name.as_str().starts_with("test_") {
            walk(&m.body, &mut lowered_paths);
        }
    }
    assert_bare_web_push(&lowered_paths, "lower");
}

/// Full analyze retypes Const via Rubydex; Value qualify must not undo
/// the splice when the test class already owns the bare constant.
#[test]
fn dns_helper_const_stays_bare_through_analyze() {
    let mut app = app();
    roundhouse::session::analyze_and_lower(&mut app);
    let tm = app
        .test_modules
        .iter()
        .find(|t| t.name.0.as_str() == "PushTest")
        .expect("PushTest");
    let mut paths = Vec::new();
    for t in &tm.tests {
        walk(&t.body, &mut paths);
    }
    for h in &tm.helpers {
        walk(&h.body, &mut paths);
    }
    assert_bare_web_push(&paths, "analyze");

    // Re-lower after analyze — emitter reads these method bodies.
    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
        &roundhouse::lower::routes::direct_helper_names(&app),
    );
    let mut lowered_paths = Vec::new();
    for m in lcs.iter().flat_map(|lc| lc.methods.iter()) {
        if m.name.as_str().starts_with("test_") {
            walk(&m.body, &mut lowered_paths);
        }
    }
    assert_bare_web_push(&lowered_paths, "post-analyze lower");
}
