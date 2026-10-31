use roundhouse::expr::{Expr, ExprNode, NATIVE_KEYWORD_ARGUMENTS};
use roundhouse::ingest::ingest_app_from_tree;

fn helper_calls(expr: &Expr, decisions: &mut Vec<bool>) {
    if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "label_for") {
        decisions.push(expr.decisions & NATIVE_KEYWORD_ARGUMENTS != 0);
    }
    expr.node.for_each_child(&mut |child| helper_calls(child, decisions));
}

#[test]
fn test_setup_and_bodies_do_not_borrow_an_app_helpers_keyword_contract() {
    let files = [
        ("app/helpers/labels_helper.rb", "module LabelsHelper\n  def label_for(style: :app)\n    style\n  end\nend\n"),
        ("test/models/label_test.rb", "require 'test_helper'\nclass LabelTest < ActiveSupport::TestCase\n  def label_for(style: :test)\n    style\n  end\n  setup do\n    label_for(style: :setup)\n  end\n  test 'local helper' do\n    label_for(style: :body)\n  end\nend\n"),
    ].into_iter().map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(files).unwrap();
    roundhouse::session::analyze_and_lower_for(&mut app, roundhouse::project::BuildTarget::Ruby);
    let module = app.test_modules.iter().find(|module| module.name.0.as_str() == "LabelTest").unwrap();
    let mut calls = Vec::new();
    helper_calls(module.setup.as_ref().expect("setup"), &mut calls);
    for test in &module.tests {
        helper_calls(&test.body, &mut calls);
    }
    assert_eq!(calls, [false, false]);
}
