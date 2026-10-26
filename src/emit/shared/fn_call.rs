//! `.call` and `[]` on a value typed as a function are invocations.
//!
//! Ruby and Crystal spell that `fn.call(x)` / `fn[x]` and that is the
//! invocation. Other targets have no `call` method on a closure: TypeScript
//! would bind the argument as `this` via `Function.prototype.call`.

use crate::expr::Expr;
use crate::ty::Ty;

/// The receiver when `method` invokes a function value. `None` for an
/// ordinary method, including `.call` on anything that is not a `Ty::Fn`.
pub fn function_receiver<'a>(
    recv: Option<&'a Expr>,
    method: &str,
    block: Option<&Expr>,
) -> Option<&'a Expr> {
    if block.is_some() || !matches!(method, "call" | "[]") {
        return None;
    }
    let recv = recv?;
    matches!(recv.ty.as_ref(), Some(Ty::Fn { .. })).then_some(recv)
}
