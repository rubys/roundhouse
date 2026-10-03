//! Native Ruby execution and output boundary for literal Current.set.
//!
//! Source admission is shared with inference in `crate::current_set`.
//! A generated class method receives RHS arguments before capturing the
//! current instance, protects sequential save/write, yields to the ORIGINAL
//! attached block, and restores saved keys in one insertion-order ensure.
//! Ruby owns next, break, captures and enclosing-method return; no Proc,
//! IIFE or overridable application primitive substitutes for that block.

use crate::app::App;
pub use crate::current_set::source_refusals;
use crate::current_set::{
    ScopedClasses, classify_site, for_each_body, keyword_pairs, ruby_family,
    scoped_classes, set_receiver_class, site_diagnostics, target_refusal,
};
use crate::diagnostic::{Diagnostic, Severity};
use crate::dialect::{
    AccessorKind, LibraryClass, LibraryClassOrigin, MethodDef, MethodReceiver, Param,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::{ParamKind, Ty};
use std::collections::{BTreeMap, BTreeSet};

/// Hard fail before output, deliberately independent of allow-unsupported.
/// Blog exports verbatim; Ruby-family targets share the generated helper.
pub fn guard_output(app: &App, target: &str) -> Result<(), String> {
    if target == "blog" {
        return Ok(());
    }
    // Reuse the analyzer's resolution on an isolated clone only when raw
    // bare candidates need it. Preserve ingest-shaped input for Roda.
    fn unresolved_site(expr: &Expr) -> bool {
        if let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: Some(block),
            ..
        } = &*expr.node
        {
            if method.as_str() == "set"
                && matches!(&*recv.node, ExprNode::Const { .. })
                && matches!(&*block.node, ExprNode::Lambda { .. })
                && keyword_pairs(args).is_some()
                && recv.ty.is_none()
            {
                return true;
            }
        }
        let mut found = false;
        expr.node
            .for_each_child(&mut |child| found |= unresolved_site(child));
        found
    }
    let classes = scoped_classes(app);
    let mut needs_resolution = false;
    if !classes.supported.is_empty() {
        for_each_body(app, &mut |body, _| {
            needs_resolution |= unresolved_site(body)
        });
    }
    let mut resolved = needs_resolution.then(|| app.clone());
    if let Some(resolved) = &mut resolved {
        crate::analyze::Analyzer::new(resolved).analyze(resolved);
    }
    let app = resolved.as_ref().unwrap_or(app);
    let mut diags = site_diagnostics(app, Some(target));
    if !ruby_family(target) {
        for class in &app.library_classes {
            if let Some(LibraryClassOrigin::CurrentSet { site }) = class.origin {
                diags.push(target_refusal(site, target));
            }
        }
    }
    if diags.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{}\nCurrent.set output refusal cannot be bypassed with --allow-unsupported.",
        diags
            .iter()
            .map(|d| d.render(&app.sources))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

pub fn apply_current_set_lowering(app: &mut App) -> Vec<Diagnostic> {
    let classes = scoped_classes(app);
    if classes.supported.is_empty() {
        return Vec::new();
    }
    let mut reserved = BTreeSet::new();
    for name in app
        .models
        .iter()
        .map(|m| &m.name)
        .chain(app.controllers.iter().map(|c| &c.name))
        .chain(app.test_modules.iter().map(|tm| &tm.name))
    {
        reserved.extend(name.0.as_str().split("::").map(str::to_string));
    }
    for class in app
        .library_classes
        .iter()
        .chain(app.rails_application.iter())
        .chain(app.test_modules.iter().flat_map(|tm| &tm.inner_classes))
    {
        reserved.extend(class.name.0.as_str().split("::").map(str::to_string));
        reserved.extend(class.constants.iter().map(|(name, _)| name.to_string()));
    }
    for tm in &app.test_modules {
        reserved.extend(tm.constants.iter().map(|(name, _)| name.to_string()));
    }
    fn reserve(expr: &Expr, names: &mut BTreeSet<String>) {
        match &*expr.node {
            ExprNode::Const { path }
            | ExprNode::Assign {
                target: LValue::Const { path },
                ..
            }
            | ExprNode::OpAssign {
                target: LValue::Const { path },
                ..
            } => {
                names.extend(path.iter().map(ToString::to_string));
            }
            _ => {}
        }
        expr.node.for_each_child(&mut |child| reserve(child, names));
    }
    for_each_body(app, &mut |expr, _| reserve(expr, &mut reserved));
    let reader_types = app
        .library_classes
        .iter()
        .flat_map(|class| {
            class.methods.iter().filter_map(|method| {
                if method.receiver != MethodReceiver::Instance {
                    return None;
                }
                let Ty::Fn { ret, .. } = method.signature.as_ref()? else {
                    return None;
                };
                Some((
                    (class.name.clone(), method.name.to_string()),
                    (**ret).clone(),
                ))
            })
        })
        .collect();
    let mut pass = ScopeLowering {
        classes,
        reserved,
        reader_types,
        generated: Vec::new(),
    };
    let initializing: Vec<_> = app
        .library_classes
        .iter()
        .filter(|class| app.current_attribute_classes.contains(&class.name))
        .flat_map(|class| {
            class
                .constants
                .iter()
                .map(|(_, value)| value as *const Expr)
                .chain(class.unknown_calls.iter().map(|call| call as *const Expr))
        })
        .collect();
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |e| {
        if initializing
            .iter()
            .any(|initializer| std::ptr::eq(*initializer, e as *const Expr))
        {
            return;
        }
        pass.rewrite(e, &mut diags);
    });
    for view in &mut app.views {
        pass.rewrite(&mut view.body, &mut diags);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            pass.rewrite(setup, &mut diags);
        }
        for t in &mut tm.tests {
            pass.rewrite(&mut t.body, &mut diags);
        }
        for m in &mut tm.helpers {
            pass.rewrite(&mut m.body, &mut diags);
        }
    }
    app.library_classes.extend(pass.generated);
    diags
}

struct ScopeLowering {
    classes: ScopedClasses,
    reserved: BTreeSet<String>,
    reader_types: BTreeMap<(ClassId, String), Ty>,
    generated: Vec<LibraryClass>,
}

impl ScopeLowering {
    fn rewrite(&mut self, e: &mut Expr, diags: &mut Vec<Diagnostic>) {
        // Classify before replacing inner sites with generated callers.
        let site = match &*e.node {
            ExprNode::Send {
                recv,
                method,
                args,
                block,
                ..
            } => {
                let site =
                    classify_site(recv.as_ref(), method, args, block.as_ref(), &self.classes);
                if site.is_none() {
                    if let Some(class) = set_receiver_class(recv.as_ref(), method, &self.classes) {
                        report(
                            e.span,
                            class.0.as_str(),
                            "its arguments and block are not both literal",
                            diags,
                        );
                    }
                }
                site
            }
            _ => None,
        };
        e.node
            .for_each_child_mut(&mut |child| self.rewrite(child, diags));
        let Some(site) = site else { return };
        if site.refusal.is_some() {
            return;
        }
        let ExprNode::Send {
            args,
            block: Some(block),
            ..
        } = &*e.node
        else {
            unreachable!("classified literal send")
        };
        let pairs = keyword_pairs(args).expect("classified literal hash");
        let mut n = self.generated.len() + 1;
        let name = loop {
            let candidate = format!("RoundhouseCurrentSetScope{n}");
            if self.reserved.insert(candidate.clone()) {
                break candidate;
            }
            n += 1;
        };
        let generated = self.scope_class(
            e.span,
            &name,
            &site.class.expect("admitted scope identity"),
            &pairs,
            e.ty.clone(),
        );
        *e.node = ExprNode::Send {
            recv: Some(const_path(e.span, &name)),
            method: Symbol::from("run"),
            args: pairs.into_iter().map(|(_, value)| value).collect(),
            block: Some(block.clone()),
            parenthesized: true,
        };
        self.generated.push(generated);
    }

    fn scope_class(
        &self,
        span: Span,
        name: &str,
        current: &ClassId,
        pairs: &[(String, Expr)],
        result: Option<Ty>,
    ) -> LibraryClass {
        let context_ty = Ty::Class {
            id: current.clone(),
            args: Vec::new(),
        };
        let context = || {
            typed(
                local_read(span, Symbol::from("__context")),
                Some(context_ty.clone()),
            )
        };
        // Arguments evaluate once, before entry and instance capture.
        let mut stmts = vec![assign_local(
            span,
            Symbol::from("__context"),
            typed(
                send(
                    span,
                    const_path(span, current.0.as_str()),
                    "instance",
                    Vec::new(),
                ),
                Some(context_ty.clone()),
            ),
        )];
        let mut protected = Vec::new();
        let mut restore = Vec::new();
        let mut params = Vec::new();
        let mut typed_params = Vec::new();
        for (i, (attribute, value)) in pairs.iter().enumerate() {
            let argument = Symbol::from(format!("__value{i}"));
            let previous = Symbol::from(format!("__previous{i}"));
            let saved = Symbol::from(format!("__saved{i}"));
            params.push(Param::positional(argument.clone()));
            typed_params.push(value.ty.clone().map(|ty| crate::ty::Param {
                name: argument.clone(),
                ty,
                kind: ParamKind::Required,
            }));
            stmts.push(assign_local(
                span,
                saved.clone(),
                typed(
                    Expr::new(
                        span,
                        ExprNode::Lit {
                            value: Literal::Bool { value: false },
                        },
                    ),
                    Some(Ty::Bool),
                ),
            ));
            let read_ty = self
                .reader_types
                .get(&(current.clone(), attribute.clone()))
                .cloned();
            // Object#with saves/writes per key inside the protected region.
            // A raising writer was saved; a raising reader was not.
            protected.push(assign_local(
                span,
                previous.clone(),
                typed(
                    send(span, context(), attribute, Vec::new()),
                    read_ty.clone(),
                ),
            ));
            protected.push(assign_local(
                span,
                saved.clone(),
                typed(
                    Expr::new(
                        span,
                        ExprNode::Lit {
                            value: Literal::Bool { value: true },
                        },
                    ),
                    Some(Ty::Bool),
                ),
            ));
            protected.push(typed(
                send(
                    span,
                    context(),
                    &format!("{attribute}="),
                    vec![typed(local_read(span, argument), value.ty.clone())],
                ),
                value.ty.clone(),
            ));
            restore.push(typed(
                Expr::new(
                    span,
                    ExprNode::If {
                        cond: typed(local_read(span, saved), Some(Ty::Bool)),
                        then_branch: typed(
                            send(
                                span,
                                context(),
                                &format!("{attribute}="),
                                vec![typed(local_read(span, previous), read_ty.clone())],
                            ),
                            read_ty.clone(),
                        ),
                        else_branch: typed(
                            Expr::new(
                                span,
                                ExprNode::Lit {
                                    value: Literal::Nil,
                                },
                            ),
                            Some(Ty::Nil),
                        ),
                    },
                ),
                read_ty.map(|ty| crate::analyze::union_of(ty, Ty::Nil)),
            ));
        }
        protected.push(typed(
            Expr::new(
                span,
                ExprNode::Yield {
                    args: vec![context()],
                },
            ),
            result.clone(),
        ));
        stmts.push(typed(
            Expr::new(
                span,
                ExprNode::BeginRescue {
                    body: typed(seq(span, protected), result.clone()),
                    rescues: Vec::new(),
                    else_branch: None,
                    // One insertion-order ensure: a restoring raise stops later keys.
                    ensure: Some(seq(span, restore)),
                    implicit: false,
                },
            ),
            result.clone(),
        ));
        let signature = result
            .clone()
            .zip(typed_params.into_iter().collect::<Option<Vec<_>>>())
            .map(|(ret, params)| Ty::Fn {
                params,
                block: Some(Box::new(Ty::Fn {
                    params: vec![crate::ty::Param {
                        name: Symbol::from("context"),
                        ty: context_ty,
                        kind: ParamKind::Required,
                    }],
                    block: None,
                    ret: Box::new(ret.clone()),
                    effects: EffectSet::pure(),
                })),
                ret: Box::new(ret),
                effects: EffectSet::pure(),
            });
        LibraryClass {
            name: ClassId(Symbol::from(name)),
            is_module: false,
            parent: None,
            includes: Vec::new(),
            methods: vec![MethodDef {
                name: Symbol::from("run"),
                receiver: MethodReceiver::Class,
                visibility: crate::dialect::MethodVisibility::Public,
                params,
                unsupported_formals: None,
                has_anonymous_block: false,
                block_param: None,
                name_span: span,
                body: typed(seq(span, stmts), result),
                signature,
                effects: EffectSet::pure(),
                enclosing_class: Some(Symbol::from(name)),
                kind: AccessorKind::Method,
                is_async: false,
                mutates_self: false,
            }],
            class_ivar_initializers: Vec::new(),
            nullable_columns: Vec::new(),
            origin: Some(LibraryClassOrigin::CurrentSet { site: span }),
            constants: Vec::new(),
            unknown_calls: Vec::new(),
        }
    }
}

fn typed(mut expr: Expr, ty: Option<Ty>) -> Expr {
    expr.ty = ty;
    expr
}

fn seq(span: Span, exprs: Vec<Expr>) -> Expr {
    if exprs.len() == 1 {
        return exprs.into_iter().next().expect("checked");
    }
    Expr::new(span, ExprNode::Seq { exprs })
}

fn send(span: Span, recv: Expr, name: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(name),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn assign_local(span: Span, name: Symbol, value: Expr) -> Expr {
    let ty = value.ty.clone();
    typed(
        Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name },
                value,
            },
        ),
        ty,
    )
}

fn local_read(span: Span, name: Symbol) -> Expr {
    Expr::new(span, ExprNode::Var { id: VarId(0), name })
}

fn const_path(span: Span, class: &str) -> Expr {
    Expr::new(
        span,
        ExprNode::Const {
            path: class.split("::").map(Symbol::from).collect(),
        },
    )
}

fn report(span: Span, class: &str, why: &str, diags: &mut Vec<Diagnostic>) {
    let mut d = Diagnostic::unsupported(
        span,
        None,
        "CurrentAttributes#set",
        format!(
            "`{class}.set` is served only as a block form over a literal keyword hash, which is what lets the save/restore be spelled at compile time: {why}"
        ),
    );
    d.severity = Severity::Warning;
    diags.push(d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::ClassId;

    fn app_with(body: &str) -> App {
        let src =
            format!("class T < ActiveSupport::TestCase\n  test \"t\" do\n    {body}\n  end\nend\n");
        let mut app = App::new();
        app.current_attribute_classes
            .push(ClassId(Symbol::from("Current")));
        app.library_classes.extend(
            crate::ingest::ingest_library_classes(b"class Current\n def request; @request; end\n def request=(value); @request = value; end\n def user; @user; end\n def user=(value); @user = value; end\n def account; @account; end\n def account=(value); @account = value; end\nend\n", "current.rb").expect("Current class"),
        );
        app.test_modules.extend(
            crate::ingest::test::ingest_test_files(src.as_bytes(), "t.rb").expect("ingest"),
        );
        app
    }

    fn lowered(app: &App) -> String {
        crate::emit::ruby::emit_expr(&app.test_modules[0].tests[0].body)
    }

    fn scope_body(app: &App) -> String {
        let class = app
            .library_classes
            .iter()
            .find(|class| matches!(class.origin, Some(LibraryClassOrigin::CurrentSet { .. })))
            .expect("generated scope");
        crate::emit::ruby::emit_expr(&class.methods[0].body)
    }

    #[test]
    fn one_attribute_saves_assigns_and_restores_in_an_ensure() {
        let mut app = app_with("Current.set(request: r) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(lowered(&app).contains("RoundhouseCurrentSetScope1.run(r) do"));
        let out = scope_body(&app);
        assert!(out.contains("__previous0 = __context.request"), "{out}");
        assert!(out.contains("__context.request = __value0"), "{out}");
        assert!(
            out.contains("yield(__context)") || out.contains("yield __context"),
            "{out}"
        );
        assert!(out.contains("ensure"), "{out}");
        assert!(
            out.contains("__context.request = __previous0 if __saved0"),
            "{out}"
        );
    }

    #[test]
    fn two_attributes_are_saved_and_written_sequentially() {
        let mut app = app_with("Current.set(request: r, user: u) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = scope_body(&app);
        let save_user = out.find("__previous1 = __context.user").expect(&out);
        let write_request = out.find("__context.request = __value0").expect(&out);
        assert!(
            write_request < save_user,
            "expected per-key sequential save/write:\n{out}"
        );
    }

    #[test]
    fn a_non_keyword_argument_is_reported_and_left() {
        let mut app = app_with("Current.set(attrs) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            lowered(&app).contains("Current.set(attrs)"),
            "{}",
            lowered(&app)
        );
    }

    #[test]
    fn another_receivers_set_is_untouched() {
        let mut app = app_with("Cache.set(key: k) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(lowered(&app).contains("Cache.set"), "{}", lowered(&app));
    }
}
