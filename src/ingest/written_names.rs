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
    }
    for model in &mut app.models {
        if let Some(parent) = &mut model.parent {
            names.rewrite_parent(&model.name, parent, model.parent_span);
        }
        for item in &mut model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                names.rewrite_include(expr);
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
                names.rewrite_include(expr);
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
        let class = self.resolver.namespace(span, &path)?;
        let name = class.0.as_str();
        (name.rsplit("::").next() == Some(last)).then(|| class.clone())
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

    /// `include X` in a model or controller body. A rooted `::X` is
    /// absolute as written.
    fn rewrite_include(&self, expr: &mut Expr) {
        let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
        if method.as_str() != "include" {
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
        }
    }
}
