//! Each superclass and `include` target the source writes, resolved
//! once from Rubydex's answers and stored on the IR in place.
//!
//! Ruby looks a constant up in the lexical scopes around the reference
//! (`Module.nesting`), then in the ancestors of the innermost one, then
//! at the top level. The written name alone cannot say which: in
//!
//! ```ruby
//! module Billing
//!   class Invoice < Record; end      # Billing::Record
//! end
//! class Billing::Receipt < Record; end # ::Record
//! ```
//!
//! both classes write `Record`, and only the second opens `Billing`
//! without making it a lexical scope. Rubydex has already resolved
//! every written reference against the whole app, so ingest asks it
//! once here, and analysis, lowering, emit and the IDE read the stored
//! name instead of each re-deriving it from the class's own name.
//!
//! A name keeps its written spelling when it has no source position
//! (a class a lowering synthesized) or Rubydex has no namespace answer
//! for it (a gem's class, an alias, a value). Ruby's own lookup found
//! no source declaration there, so no lexical guess could do better.

use crate::App;
use crate::analyze::ConstResolver;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::span::{SourceFile, Span};

/// Rewrite every source-written superclass and mixin in `app` to the
/// declaration Rubydex resolved it to.
pub(super) fn resolve(app: &mut App, sources: &[SourceFile], resolver: &ConstResolver) {
    let names = Names { sources, resolver };
    for class in &mut app.library_classes {
        // A `Data.define` block's includes are absolute already, and its
        // spans locate the factory call, not a written name.
        let data = class.data_define().is_some();
        if let Some(parent) = &mut class.parent {
            names.rewrite_parent(&class.name, parent, class.parent_span);
        }
        if data {
            continue;
        }
        for (included, call) in class.includes.iter_mut().zip(&class.include_spans) {
            if let Some(span) = names.argument_span(*call, included.0.as_str()) {
                names.rewrite_id(included, span);
            }
        }
        // `extend`/`prepend` a library class's own body does not model
        // structurally (unlike top-level `include`, above) stay in
        // `unknown_calls` as a raw `Send`; resolve those the same way,
        // so the Ruby/Spinel emitter's own lookup for which ones to
        // replay (`app_extension_calls` in `src/emit/ruby/library.rs`)
        // reads Rubydex's answer instead of guessing a second one.
        for call in &mut class.unknown_calls {
            names.rewrite_mixin_call(call, &["extend", "prepend"]);
        }
    }
    // A concern's `included do` statements, which the concern splices
    // copy onto each includer: an `include Guard` there names the module
    // as seen from inside the concern, not from the includer.
    for items in app.concern_model_items.values_mut() {
        for item in items {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                names.rewrite_mixin_call(expr, &["include"]);
            }
        }
    }
    for model in &mut app.models {
        if let Some(parent) = &mut model.parent {
            names.rewrite_parent(&model.name, parent, model.parent_span);
        }
        for item in &mut model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                names.rewrite_mixin_call(expr, &["include"]);
            }
        }
    }
    for controller in &mut app.controllers {
        if let Some(parent) = &mut controller.parent {
            names.rewrite_parent(&controller.name, parent, controller.parent_span);
        }
        for sibling in &mut controller.sibling_classes {
            let mut parent = ClassId(sibling.parent.clone());
            names.rewrite_parent(&ClassId(sibling.name.clone()), &mut parent, sibling.parent_span);
            sibling.parent = parent.0;
        }
        for item in &mut controller.body {
            if let ControllerBodyItem::Unknown { expr, .. } = item {
                names.rewrite_mixin_call(expr, &["include"]);
            }
        }
    }
}

struct Names<'a> {
    sources: &'a [SourceFile],
    resolver: &'a ConstResolver,
}

impl Names<'_> {
    fn source(&self, span: Span) -> Option<&str> {
        if span.is_synthetic() {
            return None;
        }
        let index = span.file.0.checked_sub(1)? as usize;
        Some(self.sources.get(index)?.text.as_str())
    }

    /// The namespace `written`, ending at `span.end`, resolves to.
    /// `None` unless the source spells that name there, so a span that
    /// a pass moved or a name it rewrote is never answered for a
    /// different reference.
    fn resolved(&self, span: Span, written: &str) -> Option<ClassId> {
        let text = self.source(span)?;
        let path: Vec<Symbol> = written.split("::").filter(|s| !s.is_empty()).map(Symbol::from).collect();
        let last = path.last()?.as_str();
        let start = (span.end as usize).checked_sub(last.len())?;
        if text.get(start..span.end as usize) != Some(last) {
            return None;
        }
        let Some(class) = self.resolver.namespace(span, &path) else {
            return self.resolved_prefix(span, &path, text);
        };
        let name = class.0.as_str();
        // A scoped `A::B` never names a top-level constant: Ruby does not
        // search `Object` for a scoped lookup (since 2.5 it is a
        // NameError). Rubydex does when the app reopens `A` without
        // declaring `B` (a gem's `A::B`), and its answer `Object` would
        // replace the real, undeclared `A::B`.
        if path.len() > 1 && !name.contains("::") {
            return None;
        }
        (name.rsplit("::").next() == Some(last)).then(|| class.clone())
    }

    /// A name Rubydex has no namespace for (a gem's module) whose leading
    /// segments it does resolve: Ruby binds the first segment lexically
    /// and then looks the rest up inside it, so `Testing::Assertions`
    /// inside `module ActiveSupport` is `ActiveSupport::Testing::
    /// Assertions` once the app declares `ActiveSupport::Testing`. Stored
    /// as written, the name reads as the top-level `Testing::Assertions`
    /// to every consumer that is not Ruby's own lexical lookup.
    /// The longest resolved prefix, with the rest appended.
    ///
    /// Not for a name led by a framework root (`Rails::Railtie` inside
    /// `class Billing`): the framework defines it before any app file
    /// loads, while an app namesake (`Billing::Rails`, from a file that
    /// may load after this one) is only there once its file has run.
    /// Binding the prefix statically read `Billing::Rails::Railtie`, a
    /// NameError at boot. The written name stays.
    ///
    /// Nor for a name Ruby's core or standard library defines whole
    /// (`Logger::Formatter` inside `module Tessera`, once a later file
    /// opens `Tessera::Logger`): until that file loads, Ruby's lexical
    /// lookup finds the top-level `Logger`, and after it the app's
    /// namespace, which declares no `Formatter`, is a NameError. The
    /// written name is the only answer that loads.
    fn resolved_prefix(&self, span: Span, path: &[Symbol], text: &str) -> Option<ClassId> {
        if path.first().is_some_and(|head| super::app::FRAMEWORK_ROOTS.contains(&head.as_str())) {
            return None;
        }
        let written = path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::");
        if crate::emit::audit::CORE_CONSTANTS.binary_search(&written.as_str()).is_ok() {
            return None;
        }
        let written_len: usize = path.iter().map(|s| s.as_str().len()).sum::<usize>() + 2 * (path.len() - 1);
        let start = (span.end as usize).checked_sub(written_len)?;
        (1..path.len()).rev().find_map(|keep| {
            let prefix = &path[..keep];
            let prefix_len = prefix.iter().map(|s| s.as_str().len()).sum::<usize>() + 2 * (keep - 1);
            let end = start + prefix_len;
            let last = prefix.last()?.as_str();
            if text.get(end - last.len()..end) != Some(last) {
                return None;
            }
            let class = self.resolver.namespace(Span { file: span.file, start: start as u32, end: end as u32 }, prefix)?;
            let name = class.0.as_str();
            if name.rsplit("::").next() != Some(last) {
                return None;
            }
            let rest = path[keep..].iter().map(Symbol::as_str).collect::<Vec<_>>().join("::");
            Some(ClassId(Symbol::from(format!("{name}::{rest}"))))
        })
    }

    fn rewrite_id(&self, id: &mut ClassId, span: Span) {
        if let Some(resolved) = self.resolved(span, id.0.as_str()) {
            *id = resolved;
        }
    }

    /// A superclass of the class `owner`. Ruby evaluates the superclass
    /// before it creates the class, so `module Admin; class
    /// NotesController < NotesController` names the top-level one;
    /// Rubydex answers with the class being declared. A class is never
    /// its own superclass, so that answer leaves the name as written.
    fn rewrite_parent(&self, owner: &ClassId, parent: &mut ClassId, span: Span) {
        if let Some(resolved) = self.resolved(span, parent.0.as_str())
            && resolved != *owner
        {
            *parent = resolved;
        }
    }

    /// The span of `written` as an argument of the `include` call at
    /// `call`: the one place inside the call that spells it whole.
    fn argument_span(&self, call: Span, written: &str) -> Option<Span> {
        let text = self.source(call)?.get(call.start as usize..call.end as usize)?;
        let identifier = |c: char| c.is_alphanumeric() || c == '_' || c == ':';
        let mut found = None;
        for (offset, _) in text.match_indices(written) {
            let before = text[..offset].chars().next_back();
            let after = text[offset + written.len()..].chars().next();
            if before.is_some_and(identifier) || after.is_some_and(identifier) {
                continue;
            }
            if found.is_some() {
                // `include A, A` or a name repeated in a comment; ask
                // about neither rather than guess.
                return None;
            }
            let end = call.start + (offset + written.len()) as u32;
            found = Some(Span { file: call.file, start: call.start + offset as u32, end });
        }
        found
    }

    /// `include X` in a model or controller body, or (passing
    /// `&["extend", "prepend"]`) a library class's own `extend`/
    /// `prepend` captured verbatim in its `unknown_calls`. A rooted
    /// `::X` is absolute as written.
    fn rewrite_mixin_call(&self, expr: &mut Expr, methods: &[&str]) {
        let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
        if !methods.contains(&method.as_str()) {
            return;
        }
        for arg in args.iter_mut() {
            let span = arg.span;
            let ExprNode::Const { path } = &mut *arg.node else { continue };
            if path.first().is_none_or(|head| head.as_str().is_empty()) {
                continue;
            }
            let written = path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::");
            if let Some(resolved) = self.resolved(span, &written) {
                *path = resolved.0.as_str().split("::").map(Symbol::from).collect();
            }
            arg.decisions |= crate::expr::RESOLVED_MIXIN;
        }
    }
}
