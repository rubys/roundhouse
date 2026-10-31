use crate::dialect::{MethodDef, MethodReceiver, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

use super::RuntimeRelation;

const KEEP: &str = "_keep_selected_aliases";

fn base_instance_methods() -> &'static crate::det::HashSet<Symbol> {
    static NAMES: std::sync::OnceLock<crate::det::HashSet<Symbol>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = crate::det::HashSet::default();
        for source in [
            include_str!("../../../runtime/ruby/active_record/base.rb"),
            include_str!("../../../runtime/ruby/active_record/connection.rb"),
        ] {
            for method in crate::runtime_src::parse_methods(source).expect("framework Base method bodies must ingest") {
                if method.receiver == MethodReceiver::Instance {
                    names.insert(method.name);
                }
            }
        }
        names
    })
}

fn declared_elsewhere(model: &Model, name: &Symbol) -> bool {
    model.attributes.fields.contains_key(name)
        || super::associations::model_defines_instance_method(model, name)
        || model.associations().any(|a| a.name() == name)
        || model.body.iter().any(|item| match item {
            ModelBodyItem::Unknown { expr, .. } => {
                super::attr_accessor_arg_names(expr).contains(name) || super::attribute_api_arg_name(expr).as_ref() == Some(name)
            }
            _ => false,
        })
        || base_instance_methods().contains(name)
        || model.select_aliases.iter().any(|a| &a.name == name && a.source_method)
}

fn unique_aliases(model: &Model) -> Vec<Symbol> {
    let mut out: Vec<Symbol> = Vec::new();
    for alias in &model.select_aliases {
        if !out.contains(&alias.name) {
            out.push(alias.name.clone());
        }
    }
    out
}

pub fn reader_names(model: &Model) -> Vec<Symbol> {
    unique_aliases(model).into_iter().filter(|name| !declared_elsewhere(model, name)).collect()
}

// Through the writer rather than the ivar: Rails casts a selected value by the attribute's type.
fn hydrated_attributes(model: &Model) -> Vec<Symbol> {
    let declared: Vec<Symbol> = super::markers::attribute_api_decls(&model.body).into_iter().map(|(n, _)| n).collect();
    unique_aliases(model)
        .into_iter()
        .filter(|name| {
            let setter = Symbol::from(format!("{}=", name.as_str()));
            declared.contains(name)
                && !model.attributes.fields.contains_key(name)
                && !super::associations::model_defines_instance_method(model, &setter)
        })
        .collect()
}

pub(super) fn push_select_alias_methods(methods: &mut Vec<MethodDef>, model: &Model, runtime_relation: RuntimeRelation) {
    if runtime_relation != RuntimeRelation::Present || model.select_aliases.is_empty() {
        return;
    }
    let defined = |methods: &[MethodDef], name: &Symbol| {
        methods.iter().any(|m| &m.name == name && m.receiver == MethodReceiver::Instance)
    };
    let readers: Vec<Symbol> = reader_names(model).into_iter().filter(|n| !defined(methods, n)).collect();
    let hydrated: Vec<Symbol> = hydrated_attributes(model)
        .into_iter()
        .filter(|n| defined(methods, &Symbol::from(format!("{}=", n.as_str()))))
        .collect();
    if readers.is_empty() && hydrated.is_empty() {
        return;
    }
    let Some(instantiate) = methods
        .iter_mut()
        .find(|m| m.name.as_str() == "instantiate" && m.receiver == MethodReceiver::Class)
    else {
        return;
    };
    let ExprNode::Seq { exprs } = &mut *instantiate.body.node else { return };
    let keep = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from("instance") })),
            method: Symbol::from(KEEP),
            args: vec![Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from("row") })],
            block: None,
            parenthesized: true,
        },
    );
    // Not at the end: Rails runs after_initialize on attributes already loaded.
    let at = exprs
        .iter()
        .position(|e| matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "mark_persisted!"))
        .map_or(0, |i| i + 1);
    exprs.insert(at, keep);

    let class = model.name.0.as_str();
    let mut src = format!("class {class}\n  def {KEEP}(row)\n");
    if !readers.is_empty() {
        let any_key = readers.iter().map(|n| format!("row.key?({:?})", n.as_str())).collect::<Vec<_>>().join(" || ");
        src.push_str(&format!("    @__selected_aliases = row if {any_key}\n"));
    }
    for name in &hydrated {
        let n = name.as_str();
        src.push_str(&format!("    self.{n} = row[{n:?}] if row.key?({n:?})\n"));
    }
    src.push_str("    nil\n  end\n");
    for name in &readers {
        let n = name.as_str();
        src.push_str(&format!(
            "  def {n}\n    selected = @__selected_aliases\n    raise NoMethodError, \"undefined method '{n}' for an instance of {class}\" if selected.nil? || !selected.key?({n:?})\n    selected[{n:?}]\n  end\n"
        ));
    }
    src.push_str("end\n");
    let parsed = crate::runtime_src::parse_methods(&src).expect("select alias methods must ingest");
    let span = model.select_aliases.first().map_or(model.span, |a| a.span);
    for mut method in parsed {
        method.enclosing_class = Some(model.name.0.clone());
        method.body.inherit_span(span);
        if method.name.as_str() == KEEP {
            let row_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Untyped) };
            method.signature = Some(super::fn_sig(vec![(Symbol::from("row"), row_ty)], Ty::Nil));
            method.mutates_self = true;
        }
        methods.push(method);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn select_alias_gaps_for_target(
    app: &crate::App,
    target: crate::build_target::BuildTarget,
) -> Vec<crate::diagnostic::Diagnostic> {
    if target.has_runtime_relation() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for model in &app.models {
        let mut names = reader_names(model);
        names.extend(hydrated_attributes(model));
        for alias in model.select_aliases.iter().filter(|a| names.contains(&a.name)) {
            out.push(crate::diagnostic::Diagnostic::unsupported(
                alias.span,
                Some(Symbol::from(target.as_str())),
                format!("select alias `{}`", alias.name.as_str()),
                format!(
                    "{} records keep only their schema columns on this target, so `{}` would not answer the selected value",
                    model.name.0.as_str(),
                    alias.name.as_str()
                ),
            ));
        }
    }
    out
}
