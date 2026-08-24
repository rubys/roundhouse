//! `Random.uuid` → `SecureRandom.uuid`, and the rest of the
//! `Random::Formatter` surface with it.
//!
//! These are not two methods that happen to agree. `securerandom.rb`
//! defines ONE module, `Random::Formatter`, and extends BOTH `Random`
//! and `SecureRandom` with it; `uuid` is the same code either way, and
//! only the byte source underneath differs — `Random`'s default PRNG
//! (Mersenne Twister) versus the OS CSPRNG. Which is also why
//! `Random.uuid` is undefined until something requires securerandom:
//! campfire's `Message#client_message_id ||= Random.uuid` works only
//! because Rails has already required it.
//!
//! Nothing on any target defines it. `SecureRandom` is the name the
//! emitted tree already carries (campfire reaches it four other ways,
//! `SecureRandom.alphanumeric` among them), so grounding here spends no
//! new runtime surface — the same reasoning as `time_current`.
//!
//! **Divergence, deliberately in the safe direction:** the rewritten
//! call reads the CSPRNG where Rails read the PRNG. The value is a v4
//! UUID string either way and every consumer treats it as an opaque id;
//! an app that wanted a *reproducible* uuid from a seeded `Random`
//! would not get one, and no corpus app asks for that. Recorded in
//! docs/pipeline/runtime.md.
//!
//! Only the names `Random` cannot answer on its own are rewritten.
//! `Random.rand`, `Random.bytes`, `Random.new_seed`, `Random.srand`
//! and `Random.random_number` are real methods on the PRNG class with
//! their own meaning, and redirecting those WOULD change behavior.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// The `Random::Formatter` names that reach `Random` ONLY through
/// securerandom's extend. `rand` / `random_number` are excluded: both
/// exist on the PRNG class in their own right.
const FORMATTER_ONLY: &[&str] = &[
    "uuid", "uuid_v4", "uuid_v7", "hex", "base64", "urlsafe_base64", "alphanumeric",
    "random_bytes",
];

pub fn apply_random_formatter_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: Some(recv), method, .. } = &mut *expr.node else { return };
    if !FORMATTER_ONLY.contains(&method.as_str()) {
        return;
    }
    let ExprNode::Const { path } = &*recv.node else { return };
    if path.len() != 1 || path[0].as_str() != "Random" {
        return;
    }
    let span = recv.span;
    let mut secure = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("SecureRandom")] });
    secure.ty = Some(Ty::Class { id: ClassId(Symbol::from("SecureRandom")), args: vec![] });
    *recv = secure;
    // Analyze never saw this send resolve, so the stamp is written
    // here — every name in the list answers a String, which is what
    // `analyze::registry::stdlib` already says for the SecureRandom
    // twin of each.
    expr.ty = Some(Ty::Str);
}
