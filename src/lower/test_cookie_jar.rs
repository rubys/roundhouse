//! `ActionDispatch::TestRequest.create.cookie_jar` → `ActionController::CookieJar.new`
//! in test bodies.
//!
//! The Rails 8 authentication generator's `SessionTestHelper#sign_in_as`
//! signs a cookie through a jar of its own, then copies the signed value
//! into the test's:
//!
//!   ActionDispatch::TestRequest.create.cookie_jar.tap do |cookie_jar|
//!     cookie_jar.signed[:session_id] = Current.session.id
//!     cookies["session_id"] = cookie_jar[:session_id]
//!   end
//!
//! A request built from Rails' default env has no cookies, so the jar it
//! answers is a fresh, empty one, and that is all the helper reads from
//! it. Rewritten at the call site rather than given a runtime home:
//! `TestRequest.create` takes its env with no default on purpose (see
//! runtime/ruby/action_dispatch/request.rb), and a Request never needed
//! a cookie jar of its own anywhere else.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_test_cookie_jar_lowering(app: &mut App) {
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite);
    if is_default_test_request_jar(e) {
        let jar = Expr::new(
            e.span,
            ExprNode::Const {
                path: vec![Symbol::from("ActionController"), Symbol::from("CookieJar")],
            },
        );
        *e.node = ExprNode::Send {
            recv: Some(jar),
            method: Symbol::from("new"),
            args: vec![],
            block: None,
            parenthesized: false,
        };
        e.ty = None;
    }
}

fn is_default_test_request_jar(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return false;
    };
    if method.as_str() != "cookie_jar" || !args.is_empty() {
        return false;
    }
    let ExprNode::Send { recv: Some(c), method, args, block: None, .. } = &*r.node else {
        return false;
    };
    method.as_str() == "create"
        && args.is_empty()
        && matches!(&*c.node, ExprNode::Const { path }
            if path.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["ActionDispatch", "TestRequest"])
}
