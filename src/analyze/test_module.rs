//! Canonical typing of original test scopes, shared with test emission.
use std::collections::HashMap;

use super::{Analyzer, ClassInfo, ConstScope, Ctx, extract_ivar_assignments};
use crate::App;
use crate::dialect::{AccessorKind, MethodReceiver};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(super) fn register(classes: &mut HashMap<ClassId, ClassInfo>, app: &App) {
    // File-local stand-ins are real source declarations too. Use the
    // same declared surface as test lowering, without replacing an
    // already registered production class or guessing a return type.
    for inner in app.test_modules.iter().flat_map(|module| &module.inner_classes) {
        classes.entry(inner.name.clone())
            .or_insert_with(|| crate::lower::class_info_from_library_class(inner));
    }
    for module in &app.test_modules {
        let cls = classes.entry(module.name.clone()).or_default();
        cls.parent = module.parent.clone();
        cls.includes = module.includes.clone();
        for method in &module.helpers {
            let table = match method.receiver {
                MethodReceiver::Instance => &mut cls.instance_methods,
                MethodReceiver::Class => &mut cls.class_methods,
            };
            table
                .entry(method.name.clone())
                .or_insert_with(|| method.signature.clone().unwrap_or(Ty::Untyped));
            match method.receiver {
                MethodReceiver::Instance => &mut cls.instance_method_kinds,
                MethodReceiver::Class => &mut cls.class_method_kinds,
            }
            .insert(method.name.clone(), method.kind);
        }
        // Register fixture accessors only on source test roots. Children
        // inherit that surface; a synthetic child entry would shadow a
        // real helper defined on an ancestor test class.
        if module.parent.as_ref().is_some_and(|parent| {
            app.test_modules
                .iter()
                .any(|ancestor| ancestor.name == *parent)
        }) {
            continue;
        }
        for fixture in &app.fixtures {
            cls.instance_methods
                .entry(fixture.name.clone())
                .or_insert_with(|| fixture.accessor_signature(&app.models));
            cls.instance_method_kinds
                .entry(fixture.name.clone())
                .or_insert(AccessorKind::Method);
        }
    }
}

impl Analyzer {
    /// Test call sites infer test helpers, never production signatures.
    /// Attribute inherited helpers to their real defining test class, but
    /// stop at a nearer registered/include surface that shadows that def.
    pub(super) fn test_helper_owner(
        &self,
        app: &App,
        class: &ClassId,
        method: &Symbol,
    ) -> Option<ClassId> {
        let mut current = Some(class);
        for _ in 0..32 {
            let id = current?;
            if app.test_modules.iter().any(|module| {
                module.name == *id && module.helpers.iter().any(|helper| helper.name == *method)
            }) {
                return Some(id.clone());
            }
            let registered = self.classes.get(id)?;
            if registered.instance_methods.contains_key(method)
                || registered.class_methods.contains_key(method)
            {
                return None;
            }
            current = registered.parent.as_ref();
        }
        None
    }

    /// Retype original test scopes only. View trees stay as the last
    /// full views/tests pass typed them; helper-chain rounds must not
    /// walk every template.
    pub(super) fn type_tests_only(&mut self, app: &mut App) {
        let (fallback, resolved_values) = self.build_constant_registry(app);
        self.typed_constants = resolved_values;
        let global_constants = ConstScope::global(fallback);
        self.type_test_modules(app, &global_constants);
    }

    pub(super) fn type_test_modules(&self, app: &mut App, constants: &ConstScope) {
        for module in &mut app.test_modules {
            let mut ctx = Ctx {
                self_ty: Some(Ty::Class {
                    id: module.name.clone(),
                    args: vec![],
                }),
                constants: constants.clone(),
                ..Ctx::default()
            };
            let mut own_constants = HashMap::new();
            for (name, value) in &mut module.constants {
                let ty = self.body_typer().analyze_expr(value, &ctx);
                own_constants.insert(name.clone(), ty);
                ctx.constants = constants.with_own(own_constants.clone());
            }
            if let Some(setup) = &mut module.setup {
                self.body_typer().analyze_expr(setup, &ctx);
                extract_ivar_assignments(setup, &mut ctx.ivar_bindings);
                ctx.ivar_bindings.retain(|_, ty| !ty.is_unknown());
            }
            for method in &mut module.helpers {
                let mut default_ctx = ctx.clone();
                for index in 0..method.params.len() {
                    let param = &mut method.params[index];
                    if let Some(default) = &mut param.default {
                        self.body_typer().analyze_expr(default, &default_ctx);
                    }
                    // Defaults see preceding parameter bindings, not later
                    // parameters or locals from another Ruby method scope.
                    // Re-seed after typing this default: a dependent default
                    // must see the fresh binding, not last round's default.
                    let name = param.name.clone();
                    let seeded = self.seed_method_params(&ctx, &module.name, method, false);
                    if let Some(ty) = seeded.local_bindings.get(&name) {
                        default_ctx.local_bindings.insert(name, ty.clone());
                    }
                }
                let method_ctx = self.seed_method_params(&ctx, &module.name, method, false);
                self.body_typer()
                    .analyze_expr(&mut method.body, &method_ctx);
            }
            for test in &mut module.tests {
                // Setup's ivars survive, its locals do not. Keep bare source
                // calls bare for the existing keyword-forwarding lowerer.
                self.body_typer().analyze_expr(&mut test.body, &ctx);
            }
        }
    }
}
