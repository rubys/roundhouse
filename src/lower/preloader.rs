// Not Rails' `ActiveRecord::Associations::Preloader`, which no target ships: its one public use, `new(records:, associations:).call`, is the model's own `preload_associations`.
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(crate) struct PreloaderCall<'a> {
    pub model: ClassId,
    pub records: &'a Expr,
    pub relation: bool,
    pub associations: &'a Expr,
}

/// `ActiveRecord::Associations::Preloader.new(records: r, associations: a)`
/// whose records are one model's.
pub(crate) fn preloader_new<'a>(new: &'a Expr, is_model: impl Fn(&ClassId) -> bool) -> Option<PreloaderCall<'a>> {
    let ExprNode::Send { recv: Some(konst), method, args, block: None, .. } = &*new.node else { return None };
    if method.as_str() != "new" || !is_preloader_const(konst) {
        return None;
    }
    let [kwargs] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, kwargs: true } = &*kwargs.node else { return None };
    let entry = |name: &str| {
        entries.iter().find_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name => Some(v),
            _ => None,
        })
    };
    let (records, associations) = (entry("records")?, entry("associations")?);
    if entries.len() != 2 {
        return None;
    }
    let (model, relation) = match records.ty.as_ref()? {
        Ty::Array { elem } => match elem.as_ref() {
            Ty::Class { id, .. } => (id.clone(), false),
            _ => return None,
        },
        Ty::Relation { of } => (of.clone(), true),
        _ => return None,
    };
    is_model(&model).then_some(PreloaderCall { model, records, relation, associations })
}

fn is_preloader_const(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Const { path }
        if matches!(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice(),
            ["ActiveRecord", "Associations", "Preloader"] | ["", "ActiveRecord", "Associations", "Preloader"]))
}

/// Drop the refusals typing left on `Preloader` and its `new`, once the call is known to lower.
pub(crate) fn clear_refusals(new: &mut Expr) {
    let preloader = Ty::Class { id: ClassId(Symbol::from("ActiveRecord::Associations::Preloader")), args: vec![] };
    new.diagnostic = None;
    new.ty = Some(preloader.clone());
    if let ExprNode::Send { recv: Some(konst), .. } = &mut *new.node {
        konst.diagnostic = None;
        konst.ty = Some(preloader);
    }
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(new), method, args, block: None, .. } = &*expr.node else { return };
    if method.as_str() != "call" || !args.is_empty() || expr.decisions & crate::expr::ADMITTED_PRELOADER_CALL == 0 {
        return;
    }
    let Some(call) = preloader_new(new, |_| true) else { return };
    let span = expr.span;
    let mut records = call.records.clone();
    if call.relation {
        let array = Ty::Array { elem: Box::new(Ty::Class { id: call.model.clone(), args: vec![] }) };
        records = Expr::new(span, ExprNode::Send { recv: Some(records), method: Symbol::from("to_a"), args: vec![], block: None, parenthesized: false });
        records.ty = Some(array);
    }
    let mut specs = Expr::new(span, ExprNode::Array { elements: vec![call.associations.clone()], style: Default::default() });
    specs.ty = Some(Ty::Array { elem: Box::new(Ty::Untyped) });
    let mut model = Expr::new(span, ExprNode::Const { path: call.model.0.as_str().split("::").map(Symbol::from).collect() });
    model.ty = Some(Ty::Class { id: call.model.clone(), args: vec![] });
    let mut lowered = Expr::new(
        span,
        ExprNode::Send { recv: Some(model), method: Symbol::from("preload_associations"), args: vec![records, specs], block: None, parenthesized: true },
    );
    lowered.ty = Some(Ty::Nil);
    *expr = lowered;
}
