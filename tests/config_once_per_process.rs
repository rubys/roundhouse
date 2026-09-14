//! A config assignment is evaluated ONCE per process, and a test may
//! write the key back.
//!
//! An initializer runs at boot, so `config.x.web_push_pool =
//! WebPush::Pool.new(…)` builds one pool and every read afterwards
//! answers it. The lifted reader used to re-evaluate the expression per
//! call — right for `ENV.fetch`, wrong for an object — and campfire's
//! push suite, waiting on the task counter of the pool it could see,
//! never saw the pool the pusher had built. So ingest lifts three
//! methods per leaf (`<key>__build`, the memoizing `<key>`, and
//! `<key>=`), `lower::config_reader` reads the reader's type off the
//! build method and rewrites the write a test makes, and the app-wide
//! `setup` in `test/test_helper.rb` — where campfire makes that write,
//! through `Rails.configuration.tap do |config| … end` — is spliced
//! ahead of every test module's own.

use roundhouse::analyze::Analyzer;
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_config_reader_lowering;
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\nend\n";

fn app() -> App {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :users\nend\n"),
        (
            "config/application.rb",
            "module Sample\n  class Application < Rails::Application\n  end\nend\n",
        ),
        (
            "config/initializers/pool.rb",
            "Rails.application.configure do\n  \
               config.x.pool = Pool.new(size: 3)\nend\n",
        ),
        ("lib/pool.rb", "class Pool\n  def initialize(size:)\n    @size = size\n  end\n  def size\n    @size\n  end\nend\n"),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
               def pool_size\n    \
                 Rails.configuration.x.pool.size\n  \
               end\nend\n",
        ),
        (
            "test/test_helper.rb",
            "class ActiveSupport::TestCase\n  \
               setup do\n    \
                 Rails.configuration.tap do |config|\n      \
                   config.x.pool = Pool.new(size: config.x.pool.size + 1)\n    \
                 end\n  \
               end\nend\n",
        ),
        (
            "test/models/user_test.rb",
            "require \"test_helper\"\n\
             class UserTest < ActiveSupport::TestCase\n  \
               setup do\n    @user = User.new\n  end\n  \
               test \"pool\" do\n    assert_equal 4, Rails.configuration.x.pool.size\n  end\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    apply_config_reader_lowering(&mut app);
    app
}

fn sends(expr: &Expr, out: &mut Vec<String>) {
    if let ExprNode::Send { method, args, .. } = &*expr.node {
        out.push(format!("{}/{}", method.as_str(), args.len()));
    }
    expr.node.for_each_child(&mut |c| sends(c, out));
}

#[test]
fn a_leaf_lifts_a_build_a_memoizing_reader_and_a_writer() {
    let app = app();
    let lc = app.rails_application.as_ref().expect("reopen");
    let names: Vec<&str> = lc.methods.iter().map(|m| m.name.as_str()).collect();
    for expected in ["x_pool__build", "x_pool", "x_pool="] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    let constants: Vec<&str> = lc.constants.iter().map(|(n, _)| n.as_str()).collect();
    assert!(constants.contains(&"X_POOL_SLOT"), "{constants:?}");
    assert!(constants.contains(&"CONFIG_LOCK"), "{constants:?}");
    // The memo reads the slot, not the expression.
    let reader = lc.methods.iter().find(|m| m.name.as_str() == "x_pool").unwrap();
    let mut calls = Vec::new();
    sends(&reader.body, &mut calls);
    assert!(calls.contains(&"x_pool__build/0".to_string()), "{calls:?}");
    assert!(calls.contains(&"synchronize/0".to_string()), "{calls:?}");
}

#[test]
fn the_reader_keeps_the_type_of_the_expression_it_memoizes() {
    let app = app();
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User");
    let body = user
        .body
        .iter()
        .find_map(|item| match item {
            roundhouse::dialect::ModelBodyItem::Method { method, .. } if method.name.as_str() == "pool_size" => {
                Some(&method.body)
            }
            _ => None,
        })
        .expect("pool_size");
    // `Rails.configuration.x.pool.size` -> `Rails.application.x_pool.size`,
    // and the rewritten hop is typed as the Pool the build answers.
    let ExprNode::Send { recv: Some(pool), method, .. } = &*body.node else { panic!("{:?}", body.node) };
    assert_eq!(method.as_str(), "size");
    let ExprNode::Send { method: reader, .. } = &*pool.node else { panic!("{:?}", pool.node) };
    assert_eq!(reader.as_str(), "x_pool");
    assert!(
        matches!(&pool.ty, Some(roundhouse::ty::Ty::Class { id, .. }) if id.0.as_str() == "Pool"),
        "the memoizing reader must carry its build's type: {:?}",
        pool.ty
    );
}

#[test]
fn the_app_wide_setup_is_spliced_ahead_of_the_modules_own_and_its_tap_write_becomes_the_writer() {
    let app = app();
    let tm = app.test_modules.iter().find(|t| t.name.0.as_str() == "UserTest").expect("UserTest");
    let setup = tm.setup.as_ref().expect("setup");
    let ExprNode::Seq { exprs } = &*setup.node else { panic!("{:?}", setup.node) };
    // First the app's write — unwrapped from its `tap`, with the block's
    // `config` read as the configuration, and the assignment as the
    // writer — then the module's own ivar assignment.
    let mut calls = Vec::new();
    sends(&exprs[0], &mut calls);
    assert_eq!(calls[0], "x_pool=/1", "{calls:?}");
    assert!(calls.contains(&"x_pool/0".to_string()), "the value reads the key through the reader: {calls:?}");
    assert!(!calls.iter().any(|c| c.starts_with("tap/")), "the tap is unwrapped: {calls:?}");
    assert!(
        matches!(&*exprs.last().unwrap().node, ExprNode::Assign { .. }),
        "the module's own setup follows: {:?}",
        exprs.last().unwrap().node
    );
    // And the test body's read goes through the reader too.
    let mut body_calls = Vec::new();
    sends(&tm.tests[0].body, &mut body_calls);
    assert!(body_calls.contains(&"x_pool/0".to_string()), "{body_calls:?}");
}
