// Not `Integer#to_d` / `Float#to_d` / `BigDecimal#floor` as written: `to_d` is a `bigdecimal/util` reopen no emitted tree loads and spinel cannot dispatch, and spinel's BigDecimal rounds only through `round`.
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

fn bigdecimal() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("BigDecimal")), args: vec![].into() }
}

fn send(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>, ty: Ty) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
    );
    e.ty = Some(ty);
    e
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let span = expr.span;
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    let is_decimal = |t: &Ty| matches!(t, Ty::Class { id, .. } if id.0.as_str() == "BigDecimal");
    let replacement = match (method.as_str(), r.ty.as_ref(), args.as_slice()) {
        ("to_d", Some(t), []) if is_decimal(t) => r.clone(),
        ("to_d", Some(Ty::Int), []) => send(span, None, "BigDecimal", vec![r.clone()], bigdecimal()),
        // Not `BigDecimal(float)`: it needs a precision, and `Float#to_d`'s own digits are what the runtime helper spells.
        ("to_d", Some(Ty::Float), []) => {
            let mut support = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
            support.ty = Some(Ty::Class { id: ClassId(Symbol::from("ActiveSupport")), args: vec![].into() });
            let text = send(span, Some(support), "float_decimal_text", vec![r.clone()], Ty::Str);
            send(span, None, "BigDecimal", vec![text], bigdecimal())
        }
        ("floor" | "ceil", Some(t), _) if is_decimal(t) && args.len() <= 1 => {
            let mode = if method.as_str() == "floor" { "floor" } else { "ceiling" };
            let mut sym = Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(mode) } });
            sym.ty = Some(Ty::Sym);
            match args.first() {
                Some(digits) => send(span, Some(r.clone()), "round", vec![digits.clone(), sym], bigdecimal()),
                None => {
                    let mut zero = Expr::new(span, ExprNode::Lit { value: Literal::Int { value: 0 } });
                    zero.ty = Some(Ty::Int);
                    let rounded = send(span, Some(r.clone()), "round", vec![zero, sym], bigdecimal());
                    send(span, Some(rounded), "to_i", vec![], Ty::Int)
                }
            }
        }
        _ => return,
    };
    *expr = replacement;
}
