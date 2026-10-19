//! Constructor-call normalization, separate from ordinary helper keyword calls.
//!
//! This pass has stricter semantic constraints: moving keywords must preserve
//! Ruby evaluation order and defaults must retain their lexical meaning.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::{Slot, slot_of};
use super::owned::{Defined, OwnerIndex, Side};

pub(super) struct InstanceCallParams {
    pub(super) slots: HashMap<(String, Symbol), Vec<Slot>>,
    pub(super) custom_new_slots: HashMap<(String, Symbol), Vec<Slot>>,
    custom_new: HashSet<String>,
    pub(super) unknown_new: HashSet<String>,
    pub(super) lexical_refinement_calls: HashSet<crate::span::Span>,
    unqualified_class_ids: HashMap<String, String>,
    ambiguous_names: HashSet<String>,
    model_names: HashSet<String>,
    /// The owner-relative walk's ancestor/module-aware lookup, reused
    /// here to tell "this union member doesn't define the method at
    /// all" apart from "defines it, but not with flattened keyword
    /// slots" when narrowing a nilable receiver's union — `params.slots`
    /// alone cannot tell those apart (see `resolve_instance_receiver_class`).
    owner_index: OwnerIndex,
}

impl InstanceCallParams {
    pub(super) fn new(
        app: &App,
        constructors: HashMap<
            crate::ident::ClassId,
            crate::analyze::forwarding::ConstructorContract<'_>,
        >,
        names: &super::super::forwarding::ConstructorNames,
    ) -> Self {
        let mut slots: HashMap<(String, Symbol), Vec<Slot>> = HashMap::new();
        for class in &app.library_classes {
            for method in &class.methods {
                if method.receiver != crate::dialect::MethodReceiver::Instance {
                    continue;
                }
                let key = (class.name.0.as_str().to_string(), method.name.clone());
                // A later native definition replaces the earlier flattened
                // ABI, just like class-method keyword normalization.
                slots.remove(&key);
                if method
                    .params
                    .iter()
                    .any(|param| param.rest || param.keyword || param.forwarding)
                    || !method.params.iter().any(|param| param.from_keyword)
                {
                    continue;
                }
                slots.insert(key, method.params.iter().map(slot_of).collect());
            }
        }
        // A model's own instance methods, and a concern's methods under
        // the one class that includes it: campfire's
        // `@message.broadcast_create(html: @message_html)` against
        // `Message::Broadcasts#broadcast_create(html: nil)`, which the
        // receiver types as `Message`. With several includers the
        // receiver still names one, but which module's definition wins
        // is the includer's ancestry, so those are left alone.
        let mut defined: HashSet<(String, Symbol)> = HashSet::new();
        for model in &app.models {
            for method in model.methods() {
                if method.receiver != crate::dialect::MethodReceiver::Instance {
                    continue;
                }
                let key = (model.name.0.as_str().to_string(), method.name.clone());
                defined.insert(key.clone());
                slots.remove(&key);
                if let Some(params) = flattened_keyword_slots(method) {
                    slots.insert(key, params);
                }
            }
        }
        for (module, includer) in app.sole_includer_of_modules() {
            let Some(class) = app.library_classes.iter().find(|c| c.name == module) else {
                continue;
            };
            for method in &class.methods {
                if method.receiver != crate::dialect::MethodReceiver::Instance {
                    continue;
                }
                let key = (includer.0.as_str().to_string(), method.name.clone());
                if defined.contains(&key) || slots.contains_key(&key) {
                    continue;
                }
                if let Some(params) = flattened_keyword_slots(method) {
                    slots.insert(key, params);
                }
            }
        }

        let mut custom_new = HashSet::new();
        let mut custom_new_slots = HashMap::new();
        let mut unknown_new = HashSet::new();
        let unqualified_class_ids = names.unqualified_class_ids.clone();
        for (class_id, contract) in constructors {
            add_constructor_slots(
                &class_id,
                contract,
                &mut slots,
                &mut custom_new_slots,
                &mut custom_new,
                &mut unknown_new,
            );
        }
        Self {
            slots,
            custom_new_slots,
            custom_new,
            unknown_new,
            lexical_refinement_calls: names.lexical_refinement_calls.clone(),
            unqualified_class_ids,
            ambiguous_names: names.ambiguous_names.clone(),
            model_names: names.model_names.clone(),
            owner_index: OwnerIndex::new(app),
        }
    }
}

/// The positional slots of a method whose keywords ingest flattened, or
/// `None` when it took none or kept a parameter this rule cannot move.
fn flattened_keyword_slots(method: &crate::dialect::MethodDef) -> Option<Vec<Slot>> {
    if method.params.iter().any(|param| param.rest || param.keyword || param.forwarding)
        || !method.params.iter().any(|param| param.from_keyword)
    {
        return None;
    }
    Some(method.params.iter().map(slot_of).collect())
}

pub(super) fn lexical_refinement_calls(app: &App) -> HashSet<crate::span::Span> {
    fn using_spans(expr: &Expr, owner: &str, activations: &mut Vec<(String, crate::span::Span)>) {
        if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "using") {
            activations.push((owner.to_string(), expr.span));
        }
        expr.node
            .for_each_child(&mut |child| using_spans(child, owner, activations));
    }
    fn collect_new_calls(
        expr: &Expr,
        refinements: &[crate::span::Span],
        calls: &mut HashSet<crate::span::Span>,
    ) {
        if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "new")
            && refinements
                .iter()
                .any(|using| using.file == expr.span.file && using.start < expr.span.start)
        {
            calls.insert(expr.span);
        }
        expr.node
            .for_each_child(&mut |child| collect_new_calls(child, refinements, calls));
    }

    fn refinements_for(
        owner: &str,
        activations: &[(String, crate::span::Span)],
    ) -> Vec<crate::span::Span> {
        activations
            .iter()
            .filter(|(scope, _)| {
                scope.is_empty()
                    || owner == scope
                    || owner
                        .strip_prefix(scope)
                        .is_some_and(|tail| tail.starts_with("::"))
            })
            .map(|(_, span)| *span)
            .collect()
    }

    let mut activations = Vec::new();
    for class in &app.library_classes {
        let owner = class.name.0.as_str();
        for expr in &class.unknown_calls {
            using_spans(expr, owner, &mut activations);
        }
        for (_, expr) in &class.constants {
            using_spans(expr, owner, &mut activations);
        }
    }
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                using_spans(expr, model.name.0.as_str(), &mut activations);
            }
        }
    }
    for (index, source) in app.sources.iter().enumerate() {
        if !source.path.ends_with(".rb") || !source.text.contains("using") {
            continue;
        }
        let parsed = ruby_prism::parse(source.text.as_bytes());
        let Some(program) = parsed.node().as_program_node() else {
            continue;
        };
        struct TopLevelUsing {
            spans: Vec<(u32, u32)>,
        }
        impl<'pr> ruby_prism::Visit<'pr> for TopLevelUsing {
            fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
                if call.receiver().is_none() && call.name().as_slice() == b"using" {
                    let location = call.location();
                    self.spans
                        .push((location.start_offset() as u32, location.end_offset() as u32));
                }
                ruby_prism::visit_call_node(self, call);
            }

            fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {}

            fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {}

            fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {}

            fn visit_singleton_class_node(&mut self, _: &ruby_prism::SingletonClassNode<'pr>) {}
        }
        let mut using = TopLevelUsing { spans: Vec::new() };
        ruby_prism::Visit::visit(&mut using, &program.as_node());
        for (start, end) in using.spans {
            activations.push((
                String::new(),
                crate::span::Span {
                    file: crate::span::FileId(index as u32 + 1),
                    start,
                    end,
                },
            ));
        }
    }

    let mut spans = HashSet::new();
    for class in &app.library_classes {
        let refinements = refinements_for(class.name.0.as_str(), &activations);
        if !refinements.is_empty() {
            for expr in &class.unknown_calls {
                collect_new_calls(expr, &refinements, &mut spans);
            }
            for (_, expr) in &class.constants {
                collect_new_calls(expr, &refinements, &mut spans);
            }
            for expr in &class.class_ivar_initializers {
                collect_new_calls(expr, &refinements, &mut spans);
            }
            for method in &class.methods {
                collect_new_calls(&method.body, &refinements, &mut spans);
                for default in method
                    .params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    collect_new_calls(default, &refinements, &mut spans);
                }
            }
        }
    }
    for model in &app.models {
        let refinements = refinements_for(model.name.0.as_str(), &activations);
        if !refinements.is_empty() {
            for method in model.methods() {
                collect_new_calls(&method.body, &refinements, &mut spans);
                for default in method
                    .params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    collect_new_calls(default, &refinements, &mut spans);
                }
            }
        }
    }
    spans
}

fn add_constructor_slots(
    class_id: &crate::ident::ClassId,
    contract: crate::analyze::forwarding::ConstructorContract<'_>,
    slots: &mut HashMap<(String, Symbol), Vec<Slot>>,
    custom_new_slots: &mut HashMap<(String, Symbol), Vec<Slot>>,
    custom_new: &mut HashSet<String>,
    unknown_new: &mut HashSet<String>,
) {
    let class_name = class_id.0.as_str().to_string();
    let method = match contract {
        crate::analyze::forwarding::ConstructorContract::CustomNew(method) => {
            custom_new.insert(class_name.clone());
            if let Some(method) = method
                && !method
                    .params
                    .iter()
                    .any(|param| param.rest || param.keyword || param.forwarding)
                && method.params.iter().any(|param| param.from_keyword)
            {
                custom_new_slots.insert(
                    (class_name, Symbol::from("new")),
                    method.params.iter().map(slot_of).collect(),
                );
            }
            return;
        }
        crate::analyze::forwarding::ConstructorContract::UnknownLookup => {
            unknown_new.insert(class_name);
            return;
        }
        crate::analyze::forwarding::ConstructorContract::Initialize(method) => method,
    };
    if method
        .params
        .iter()
        .any(|param| param.rest || param.keyword || param.forwarding)
        || !method.params.iter().any(|param| param.from_keyword)
    {
        return;
    }
    slots.insert(
        (class_name, Symbol::from("initialize")),
        method.params.iter().map(slot_of).collect(),
    );
}

pub(super) fn refuse_keyword_splats(
    expr: &mut Expr,
    params: &InstanceCallParams,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    expr.node
        .for_each_child_mut(&mut |child| refuse_keyword_splats(child, params, diagnostics));
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        ..
    } = &*expr.node
    else {
        return;
    };
    if method.as_str() != "new" || !matches!(&*recv.node, ExprNode::Const { .. }) {
        return;
    }
    let Some(crate::ty::Ty::Class { id, .. }) = &recv.ty else {
        return;
    };
    let class_name = params
        .unqualified_class_ids
        .get(id.0.as_str())
        .map_or(id.0.as_str(), String::as_str);
    if let Some(reason) =
        constructor_refusal_reason(params, expr.span, id.0.as_str(), class_name, args)
    {
        refuse_constructor(expr.span, crate::expr::ExprCold::diagnostic_mut(&mut expr.cold), reason, diagnostics);
    }
}

pub(super) fn rewrite_instance_call_node(
    expr: &mut Expr,
    params: &InstanceCallParams,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    rewrite_instance_call(expr, params, diagnostics);
}

/// What `resolve_instance_receiver_class` found for a non-constructor
/// instance call's receiver type.
enum ClassResolution {
    /// No candidate class defines this method with flattened keyword
    /// slots (including an untyped/non-class remainder after dropping
    /// `Nil`): nothing for this pass to rewrite.
    None,
    /// Exactly one class remains after dropping `Nil`, or several
    /// remain whose flattened slots for this method are identical —
    /// either way the keywords land in the same positions whichever
    /// branch runs at runtime.
    One(String),
    /// Several classes remain that disagree about this method: either
    /// their flattened slots differ, or one of them defines the method
    /// without a flattened keyword ABI at all (real keywords, `rest`,
    /// forwarding, or no keywords — already binding correctly as
    /// written). No one respelling is right for every branch, so the
    /// call is left exactly as written — the dynamic path, with no
    /// diagnostic, the same as before this fix existed and the same as
    /// the general untyped-receiver gap.
    Mismatch,
}

/// Resolve the library class a non-constructor instance call's receiver
/// may be, after dropping `Nil` from a nilable union. `Array#first`,
/// `find_by`, and an `[]` read all type the receiver
/// `Ty::Union { [Class, Nil] }` — the analyzer knows the class, the
/// receiver just isn't pinned to it the way a plain `Ty::Class` is. A
/// wider union of several classes is handled too, as long as they agree
/// on this method's slot shape; anything else (an `Untyped`/`Var` arm,
/// a non-class type) returns `None`, leaving that gap exactly as it was.
///
/// A union member with no entry in `params.slots` is NOT assumed to
/// skip the method: `params.slots` only ever holds flattened keyword
/// ABIs, so a class that defines the method with real keywords, `rest`,
/// forwarding, or a plain positional parameter (`def deliver(opts)`,
/// which already binds the caller's keyword Hash correctly as written)
/// looks identical to a class that doesn't define the method at all.
/// Respelling the call for another branch would silently change that
/// one's binding. `OwnerIndex::defines` — the same ancestor/module-aware
/// lookup the owner-relative walk uses — tells the two apart: truly
/// missing is skipped (that branch's dispatch never reaches the method,
/// so it raises `NoMethodError` regardless of this rewrite), while
/// "defined but not flattened" is a `Mismatch` — left exactly as
/// written, the dynamic path, with no diagnostic (see `ClassResolution`).
fn resolve_instance_receiver_class(
    ty: &crate::ty::Ty,
    params: &InstanceCallParams,
    method: &Symbol,
) -> ClassResolution {
    let stripped = ty.clone().strip_nil();
    match &stripped {
        // A single class is unambiguous — there is no other branch to
        // agree or disagree with — so this is exactly the plain typed
        // receiver case #814 already handled: a miss in `params.slots`
        // means nothing to rewrite, not a refusal. The ancestor-aware
        // `Mismatch` check below only makes sense across several
        // candidates, so it does not apply here (it would otherwise
        // turn every ordinary, non-keyword method call on a typed
        // instance — an `attr_reader`, `b.add(...).rows` below — into a
        // spurious refusal, since `OwnerIndex::defines` resolves nearly
        // every real method name as "defined, not flattened").
        crate::ty::Ty::Class { id, .. } => {
            let class_id = id.0.as_str().to_string();
            match params.slots.get(&(class_id.clone(), method.clone())) {
                Some(_) => ClassResolution::One(class_id),
                None => ClassResolution::None,
            }
        }
        crate::ty::Ty::Union { variants } => {
            let mut class_ids = Vec::with_capacity(variants.len());
            for variant in variants.iter() {
                let crate::ty::Ty::Class { id, .. } = variant else {
                    return ClassResolution::None;
                };
                class_ids.push(id.0.as_str().to_string());
            }
            let mut representative: Option<(&str, &Vec<Slot>)> = None;
            for class_id in &class_ids {
                let slots = match params.slots.get(&(class_id.clone(), method.clone())) {
                    Some(slots) => slots,
                    // `params.slots` only keys each class's OWN direct
                    // definitions; fall back to the ancestor/module-aware
                    // lookup for one reached only through inheritance or
                    // an included module (e.g. a subclass that doesn't
                    // override the method at all).
                    None => match params.owner_index.defines(class_id, Side::Instance, method) {
                        Defined::Missing => {
                            // This branch of the union doesn't define
                            // the method at all: calling it raises
                            // `NoMethodError` regardless of this
                            // rewrite, so it doesn't constrain the
                            // respelling.
                            continue;
                        }
                        Defined::Flattened(slots) => slots,
                        Defined::Unflattened | Defined::Ambiguous => {
                            // Defined, but without a flattened keyword
                            // ABI this call site could respell to (real
                            // keywords, rest, forwarding, no keywords at
                            // all — already binds correctly as written),
                            // or the lookup itself is ambiguous.
                            // Rewriting the call for another branch
                            // would silently change this one's binding,
                            // so this is a `Mismatch` — left dynamic,
                            // not guessed at.
                            return ClassResolution::Mismatch;
                        }
                    },
                };
                match representative {
                    None => representative = Some((class_id.as_str(), slots)),
                    Some((_, rep_slots)) if !slots_match_shape(rep_slots, slots) => {
                        // Two members flatten this method differently:
                        // same reasoning, same `Mismatch`.
                        return ClassResolution::Mismatch;
                    }
                    Some(_) => {}
                }
            }
            match representative {
                Some((id, _)) => ClassResolution::One(id.to_string()),
                None => ClassResolution::None,
            }
        }
        _ => ClassResolution::None,
    }
}

/// Whether two classes' flattened slots for the same method name agree
/// on shape: same slot count, same names in the same order, the same
/// keyword/kwrest origin per slot, and structurally equal defaults
/// (defaults are spliced into gaps the caller left unnamed, so a
/// mismatched default would change behavior depending on which branch
/// actually runs).
fn slots_match_shape(a: &[Slot], b: &[Slot]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.name == y.name
                && x.from_keyword == y.from_keyword
                && x.from_kwrest == y.from_kwrest
                && match (&x.default, &y.default) {
                    (None, None) => true,
                    (Some(dx), Some(dy)) => dx.node == dy.node,
                    _ => false,
                }
        })
}

fn rewrite_instance_call(
    expr: &mut Expr,
    params: &InstanceCallParams,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    if matches!(
        &expr.diagnostic(),
        Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. })
            if construct.as_str() == crate::diagnostic::CONSTRUCTOR_KEYWORD_ARGUMENTS
    ) {
        return;
    }
    if params.slots.is_empty()
        && params.unknown_new.is_empty()
        && params.ambiguous_names.is_empty()
        && params.lexical_refinement_calls.is_empty()
    {
        return;
    }
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        ..
    } = &mut *expr.node
    else {
        return;
    };
    let is_constructor = method.as_str() == "new" && matches!(&*recv.node, ExprNode::Const { .. });
    let owned_class_id: String;
    let class_id: &str = if is_constructor {
        let Some(crate::ty::Ty::Class { id, .. }) = &recv.ty else {
            return;
        };
        let class_id = params
            .unqualified_class_ids
            .get(id.0.as_str())
            .map_or(id.0.as_str(), String::as_str);
        if let Some(reason) =
            constructor_refusal_reason(params, expr.span, id.0.as_str(), class_id, args)
        {
            refuse_constructor(expr.span, crate::expr::ExprCold::diagnostic_mut(&mut expr.cold), reason, diagnostics);
            return;
        }
        if params.custom_new.contains(class_id) {
            return;
        }
        class_id
    } else {
        // A receiver whose type isn't a single `Ty::Class` is usually
        // untyped (out of scope here), but `Array#first`, `find_by` and
        // an `[]` read type it `Ty::Union { [Class, Nil] }`: the
        // analyzer knows the class(es), the call just isn't pinned to
        // one. Strip `Nil` and respell against what's left when that's
        // unambiguous; a call on the dropped `nil` branch would raise
        // `NoMethodError` either way, so narrowing it away doesn't
        // change that call's behavior.
        let Some(ty) = &recv.ty else { return };
        match resolve_instance_receiver_class(ty, params, method) {
            ClassResolution::None => return,
            ClassResolution::One(id) => {
                owned_class_id = id;
                owned_class_id.as_str()
            }
            // Several classes disagree about this method (different
            // flattened shapes, or one of them isn't flattened at all):
            // no one respelling is right for every branch. Per #959's
            // CodeRabbit review, this keeps the DYNAMIC path — the call
            // is left exactly as written, with no diagnostic — the same
            // as the general untyped-receiver gap and as every union
            // receiver before this fix existed. A hard refusal here
            // regressed `critic_corrections_preserve_class_objects_
            // reflection_and_operators`, which passes on main precisely
            // because its mismatched union fell through silently.
            ClassResolution::Mismatch => return,
        }
    };
    if !is_constructor && super::SEND_METHODS.contains(&method.as_str()) {
        let class_id = class_id.to_string();
        super::owned::refuse_send(
            expr.span,
            args,
            |name| params.slots.contains_key(&(class_id.clone(), name.clone())),
            diagnostics,
        );
        return;
    }
    let method = if is_constructor {
        Symbol::from("initialize")
    } else {
        method.clone()
    };
    let Some(slots) = params.slots.get(&(class_id.to_string(), method)) else {
        return;
    };
    if is_constructor {
        if respell_constructor(args, slots).is_err() {
            refuse_constructor(
                expr.span,
                crate::expr::ExprCold::diagnostic_mut(&mut expr.cold),
                "cannot safely move these keywords into initialize's positional slots without changing Ruby argument binding or evaluation semantics",
                diagnostics,
            );
        }
    } else if let Err(reason) = super::respell(args, slots, true) {
        super::refuse(expr.span, reason, diagnostics);
    }
}

pub(super) fn rewrite_custom_new_call(
    expr: &mut Expr,
    params: &InstanceCallParams,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) -> bool {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        ..
    } = &mut *expr.node
    else {
        return false;
    };
    if method.as_str() != "new" || !matches!(&*recv.node, ExprNode::Const { .. }) {
        return false;
    }
    let Some(crate::ty::Ty::Class { id, .. }) = &recv.ty else {
        return false;
    };
    let class_id = params
        .unqualified_class_ids
        .get(id.0.as_str())
        .map_or(id.0.as_str(), String::as_str);
    let Some(slots) = params
        .custom_new_slots
        .get(&(class_id.to_string(), method.clone()))
    else {
        return false;
    };
    if respell_constructor(args, slots).is_err() {
        refuse_constructor(
            expr.span,
            crate::expr::ExprCold::diagnostic_mut(&mut expr.cold),
            "cannot safely move these keywords into custom `new` positional slots without changing Ruby argument binding or evaluation semantics",
            diagnostics,
        );
    }
    true
}

fn has_keyword_arguments(args: &[Expr]) -> bool {
    args.iter().any(|arg| match &*arg.node {
        ExprNode::KeywordSplat { .. } => true,
        ExprNode::Hash {
            entries,
            kwargs: true,
        } => !entries.is_empty(),
        _ => false,
    })
}

fn constructor_refusal_reason(
    params: &InstanceCallParams,
    span: crate::span::Span,
    unqualified_name: &str,
    class_name: &str,
    args: &[Expr],
) -> Option<&'static str> {
    if !has_keyword_arguments(args) {
        return None;
    }
    if params.ambiguous_names.contains(unqualified_name) && !params.model_names.contains(class_name)
    {
        return Some(
            "cannot verify constructor lookup because this unqualified class name is ambiguous",
        );
    }
    if params.lexical_refinement_calls.contains(&span) {
        return Some("a lexical refinement may replace this class's `new` method");
    }
    if params.model_names.contains(class_name)
        && !params
            .slots
            .contains_key(&(class_name.to_string(), Symbol::from("initialize")))
    {
        return None;
    }
    let has_keyword_splat = args
        .iter()
        .any(|arg| matches!(&*arg.node, ExprNode::KeywordSplat { .. }));
    if params.custom_new.contains(class_name) {
        return (has_keyword_splat
            && params
                .custom_new_slots
                .contains_key(&(class_name.to_string(), Symbol::from("new"))))
        .then_some("cannot safely rebind keyword splats to custom `new` positional slots");
    }
    if params.unknown_new.contains(class_name) {
        return Some(
            "cannot verify the effective `new` method because this class has unmodeled constructor lookup",
        );
    }
    if has_keyword_splat
        && params
            .slots
            .contains_key(&(class_name.to_string(), Symbol::from("initialize")))
    {
        return Some("cannot safely rebind keyword splats to initialize's positional slots");
    }
    None
}

fn refuse_constructor(
    span: crate::span::Span,
    diagnostic_kind: &mut Option<crate::diagnostic::DiagnosticKind>,
    detail: &str,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    let diagnostic = crate::diagnostic::Diagnostic::unsupported(
        span,
        None,
        crate::diagnostic::CONSTRUCTOR_KEYWORD_ARGUMENTS,
        detail,
    );
    *diagnostic_kind = Some(diagnostic.kind.clone());
    diagnostics.push(diagnostic);
}

/// Constructors need stricter mapping than ordinary keyword lowering:
/// preserve source evaluation order, duplicate-key evaluation, and splats;
/// refuse any shape that cannot be represented by positional arguments.
fn respell_constructor(args: &mut Vec<Expr>, slots: &[Slot]) -> Result<(), ()> {
    let Some(Expr { node, .. }) = args.last() else {
        return Ok(());
    };
    let ExprNode::Hash {
        entries,
        kwargs: true,
    } = &**node
    else {
        return Ok(());
    };
    if entries.is_empty() {
        return Ok(());
    }
    let filled = args.len() - 1;
    if args[..filled]
        .iter()
        .any(|arg| matches!(&*arg.node, ExprNode::Splat { .. }))
    {
        return Err(());
    }

    let mut supplied = Vec::with_capacity(entries.len());
    let mut positions = HashSet::new();
    let mut previous_pos = None;
    for (key, value) in entries {
        let ExprNode::Lit {
            value: Literal::Sym { value: name },
        } = &*key.node
        else {
            return Err(());
        };
        let Some(pos) = slots.iter().position(|slot| slot.name == *name) else {
            return Err(());
        };
        if !slots[pos].from_keyword
            || pos < filled
            || !positions.insert(pos)
            || previous_pos.is_some_and(|previous| pos < previous)
        {
            return Err(());
        }
        previous_pos = Some(pos);
        supplied.push((pos, value.clone()));
    }

    let last_pos = previous_pos.expect("non-empty entries have a positional slot");
    let mut moved = Vec::with_capacity(last_pos - filled + 1);
    let mut supplied = supplied.into_iter().peekable();
    for pos in filled..=last_pos {
        if supplied
            .peek()
            .is_some_and(|(provided, _)| *provided == pos)
        {
            moved.push(supplied.next().expect("peeked supplied value").1);
            continue;
        }
        let Some(default) = slots.get(pos).and_then(|slot| slot.default.as_ref()) else {
            return Err(());
        };
        if !is_context_free_constructor_default(default) {
            return Err(());
        }
        moved.push(default.clone());
    }
    args.pop();
    args.extend(moved);
    Ok(())
}

/// A constructor default is evaluated in the class's lexical scope. Moving
/// even a constant expression to the caller can resolve a different constant.
fn is_context_free_constructor_default(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Regex { .. },
        } => false,
        ExprNode::Lit { .. } => true,
        ExprNode::Array { elements, .. } => {
            elements.iter().all(is_context_free_constructor_default)
        }
        ExprNode::Hash { entries, .. } => entries.iter().all(|(key, value)| {
            is_context_free_constructor_default(key) && is_context_free_constructor_default(value)
        }),
        _ => false,
    }
}
