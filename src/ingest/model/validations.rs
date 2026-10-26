//! `validates` / `validates_*_of` option parsing.
//!
//! Recognized rules become `Validation` items. An option this build
//! does not lower is a warning and a survey gap, and the call is not
//! turned into a partial check.

use ruby_prism::Node;

use crate::Symbol;
use crate::dialect::{Comment, ModelBodyItem};
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, Literal};
use crate::span::Span;

use super::super::IngestError;
use super::super::util::{string_value, symbol_value};

use crate::ingest::prism_api::PrismLocation;
pub(super) fn validation_items(
    parsed: Vec<crate::dialect::Validation>,
    leading_comments: Vec<Comment>,
    span: Span,
) -> Vec<ModelBodyItem> {
    parsed
        .into_iter()
        .enumerate()
        .map(|(i, validation)| ModelBodyItem::Validation {
            validation,
            // The declaration's comments belong to the first of the
            // items it expands to.
            leading_comments: if i == 0 {
                leading_comments.clone()
            } else {
                Vec::new()
            },
            leading_blank_line: false,
            span,
        })
        .collect()
}

/// Which `validates` spelling `parse_validates_call` is reading.
#[derive(Clone, Copy)]
pub(super) enum ValidatesKind {
    /// `validates :attr, presence: true, numericality: { … }`.
    Map,
    Numericality,
    Inclusion,
    Exclusion,
    Format,
}

pub(super) enum ParsedValidates {
    /// Every option on the call is lowered. Empty rules are a real
    /// no-op (`presence: false`), not a dropped validator.
    Modeled(Vec<crate::dialect::Validation>),
    /// An option this build does not lower. The call is not a
    /// `Validation` and not an `Unknown` re-warning: the other rules
    /// must not run while this one vanishes, and a condition must not
    /// become an unconditional check.
    Gap,
    /// Not a shape this parser claims (`validates` with a block). The
    /// call stays on the unsupported-DSL ledger.
    Decline,
    /// Every option is lowered but some keys that name no built-in
    /// validator (`domain: true` for an app's `DomainValidator`): the rules
    /// that are, and those keys. Which keys name an app validator is known
    /// only once the whole app is ingested (`ingest::each_validator`), which
    /// lowers them or reports the call.
    Custom(Vec<crate::dialect::Validation>, Vec<String>),
}

struct BuiltRule {
    rule: crate::dialect::ValidationRule,
    if_cond: Option<Expr>,
    unless_cond: Option<Expr>,
    /// `on:` in the rule's own hash, which wins over the call-level one:
    /// `None` unset, `Some(guard)` as `context_guard` reads it.
    on_guard: Option<Option<Expr>>,
    /// A call-level `allow_nil: true` on a rule that stores no flag of its
    /// own (presence, inclusion, exclusion): the rule runs only when the
    /// attribute is not nil.
    nil_skip: bool,
    /// The same for `allow_blank: true`: the rule runs only when the
    /// attribute is not blank (nil, `""`, `"  "`).
    blank_skip: bool,
    /// A literal String `message:` in the rule's own hash.
    message: Option<String>,
    /// The rule's own hash named `allow_nil:` / `allow_blank:`, which
    /// wins over the call-level option (Rails merges the call's options
    /// under each validator's own).
    own_allow_nil: bool,
    own_allow_blank: bool,
}

impl BuiltRule {
    fn bare(rule: crate::dialect::ValidationRule) -> Self {
        Self { rule, if_cond: None, unless_cond: None, on_guard: None, nil_skip: false, blank_skip: false, message: None, own_allow_nil: false, own_allow_blank: false }
    }
}

enum RuleParse {
    /// `presence: false` — Rails adds no validator.
    Skip,
    Rule(BuiltRule),
    Gap(Vec<String>),
}

/// The attribute names a `*CONST` splat expands to: a list constant the
/// class body folds (`%i[a b]`, `%w[a b]`, `.freeze`d or not) whose every
/// element is a Symbol or a String.
fn splat_names(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<Vec<Symbol>> {
    class_consts
        .list(node)?
        .into_iter()
        .map(|lit| match lit {
            Literal::Sym { value } => Some(value),
            Literal::Str { value } => Some(Symbol::from(value.as_str())),
            _ => None,
        })
        .collect()
}

pub(super) fn parse_validates_call(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    kind: ValidatesKind,
    class_consts: &super::ClassConsts,
) -> ParsedValidates {
    use crate::dialect::{Validation, ValidationRule};
    if call.block().is_some() {
        return ParsedValidates::Decline;
    }
    let Some(args) = call.arguments() else {
        return ParsedValidates::Decline;
    };
    let span = Span {
        file: super::super::sources::file_id(file),
        start: call.location().start_offset() as u32,
        end: call.location().end_offset() as u32,
    };
    let method = match kind {
        ValidatesKind::Map => "validates",
        ValidatesKind::Numericality => "validates_numericality_of",
        ValidatesKind::Inclusion => "validates_inclusion_of",
        ValidatesKind::Exclusion => "validates_exclusion_of",
        ValidatesKind::Format => "validates_format_of",
    };

    let mut attrs: Vec<Symbol> = Vec::new();
    let mut option_node: Option<Node<'_>> = None;
    let mut gaps: Vec<String> = Vec::new();
    // Keys naming no built-in validator: an app's `EachValidator`, maybe.
    let mut custom: Vec<String> = Vec::new();
    for arg in args.arguments().iter() {
        if let Some(name) = super::super::util::symbol_or_string_value(&arg) {
            attrs.push(Symbol::from(name.as_str()));
            continue;
        }
        // `validates(*FIELDS, presence: true)` with `FIELDS = %i[a b]` in
        // the class body: Ruby expands the splat into the attribute names.
        if let Some(splat) = arg.as_splat_node() {
            if let Some(names) = splat.expression().and_then(|e| splat_names(&e, class_consts)) {
                attrs.extend(names);
            } else {
                gaps.push("argument".to_string());
            }
            continue;
        }
        if hash_elements(&arg).is_some() {
            if option_node.is_some() {
                gaps.push("options".to_string());
            } else {
                option_node = Some(arg);
            }
            continue;
        }
        gaps.push("argument".to_string());
    }

    let mut allow_blank = false;
    let mut allow_nil: Option<bool> = None;
    let mut call_if_cond: Option<Expr> = None;
    let mut call_unless_cond: Option<Expr> = None;
    // `on: :create` / `:update`: a guard ANDed into every rule's `if`.
    let mut on_guard: Option<Expr> = None;
    let mut drafts: Vec<BuiltRule> = Vec::new();

    if let Some(node) = &option_node {
        let elements = hash_elements(node).unwrap();
        match kind {
            ValidatesKind::Map => {
                for el in elements.iter() {
                    let Some(assoc) = el.as_assoc_node() else {
                        gaps.push("options".to_string());
                        continue;
                    };
                    let Some(key) = symbol_value(&assoc.key()) else {
                        gaps.push("options".to_string());
                        continue;
                    };
                    let value = assoc.value();
                    match key.as_str() {
                        "allow_blank" => match super::super::util::bool_value(&value) {
                            Some(true) => allow_blank = true,
                            Some(false) => {}
                            None => gaps.push("allow_blank".to_string()),
                        },
                        "allow_nil" => match super::super::util::bool_value(&value) {
                            Some(flag) => allow_nil = Some(flag),
                            None => gaps.push("allow_nil".to_string()),
                        },
                        "if" => push_condition(&value, file, &mut call_if_cond, "if", &mut gaps),
                        "unless" => {
                            push_condition(&value, file, &mut call_unless_cond, "unless", &mut gaps)
                        }
                        "on" => match context_guard(&value) {
                            Some(guard) => on_guard = guard,
                            None => gaps.push(key),
                        },
                        "strict" | "message" => gaps.push(key),
                        other => match parse_validator(other, &value, file, class_consts) {
                            RuleParse::Skip => {}
                            RuleParse::Rule(built) => drafts.push(built),
                            RuleParse::Gap(more) if !BUILT_IN_VALIDATORS.contains(&other) && more == [other] => {
                                custom.push(other.to_string())
                            }
                            RuleParse::Gap(more) => gaps.extend(more),
                        },
                    }
                }
            }
            ValidatesKind::Numericality | ValidatesKind::Inclusion | ValidatesKind::Exclusion | ValidatesKind::Format => {
                parse_of_options(
                    kind,
                    &elements,
                    file,
                    &mut allow_blank,
                    &mut allow_nil,
                    &mut call_if_cond,
                    &mut call_unless_cond,
                    &mut on_guard,
                    &mut drafts,
                    &mut gaps,
                    class_consts,
                );
            }
        }
    } else if matches!(kind, ValidatesKind::Numericality) && gaps.is_empty() && !attrs.is_empty() {
        // `validates_numericality_of :price` is `numericality: true`.
        drafts.push(BuiltRule::bare(ValidationRule::Numericality {
            only_integer: false,
            gt: None,
            lt: None,
            gte: None,
            lte: None,
            allow_nil: None,
            allow_blank: None,
            in_range: None,
        }));
    } else if !matches!(kind, ValidatesKind::Map) && !attrs.is_empty() {
        gaps.push(match kind {
            ValidatesKind::Inclusion => "inclusion".to_string(),
            ValidatesKind::Exclusion => "exclusion".to_string(),
            ValidatesKind::Format => "format".to_string(),
            ValidatesKind::Numericality | ValidatesKind::Map => "options".to_string(),
        });
    }

    // Uniqueness, Numericality and Format store both flags, so a call-level
    // one applies to them (below) rather than refusing the call.
    let is_uniqueness = |draft: &BuiltRule| matches!(draft.rule, ValidationRule::Uniqueness { .. });
    let stores_flags = |draft: &BuiltRule| {
        matches!(draft.rule, ValidationRule::Numericality { .. } | ValidationRule::Format { .. })
    };
    // Acceptance stores `allow_nil` (below); `allow_blank` stays the gap.
    let stores_nil = |draft: &BuiltRule| matches!(draft.rule, ValidationRule::Acceptance { .. });
    // Call-level `allow_blank: true` on a rule that stores no flag of its
    // own: Rails skips a blank value for every validator in the call, so
    // the rule runs only when the value is not blank. Its own option wins.
    if allow_blank {
        for draft in &mut drafts {
            if !blank_safe(&draft.rule) && !is_uniqueness(draft) && !stores_flags(draft) && !draft.own_allow_blank {
                draft.blank_skip = true;
            }
        }
    }
    // Call-level `allow_nil: true` is Rails' skip-when-nil for every
    // validator in the call. Length, Uniqueness, Numericality and Format store it, and
    // Presence skips nil (`nil_skip` below). The other
    // rules this build lowers reject nil, so applying them would reject
    // records Rails accepts.
    if allow_nil == Some(true) {
        for draft in &mut drafts {
            if !matches!(draft.rule, ValidationRule::Length { .. } | ValidationRule::Presence)
                && !is_uniqueness(draft)
                && !stores_flags(draft)
                && !stores_nil(draft)
                && !draft.own_allow_nil
            {
                draft.nil_skip = true;
            }
        }
    }

    if !gaps.is_empty() {
        // Another gap drops the call; its custom keys are reported with it.
        gaps.extend(custom);
        gaps.sort();
        gaps.dedup();
        record_validation_gaps(file, span, method, &attrs, &gaps);
        return ParsedValidates::Gap;
    }
    if attrs.is_empty() {
        return ParsedValidates::Decline;
    }

    if allow_blank {
        drafts.retain(|draft| !matches!(draft.rule, ValidationRule::Presence));
    }
    for draft in &mut drafts {
        let (own_nil, own_blank) = (draft.own_allow_nil, draft.own_allow_blank);
        if let ValidationRule::Uniqueness { allow_nil: nil_slot, allow_blank: blank_slot, .. } = &mut draft.rule {
            if let (Some(flag), false) = (allow_nil, own_nil) {
                *nil_slot = flag;
            }
            if allow_blank && !own_blank {
                *blank_slot = true;
            }
        }
    }
    // Presence under a call-level `allow_nil: true`: Rails' EachValidator
    // skips nil, so presence checks only a non-nil value for blank. The
    // rule's own `allow_nil: false` wins (Rails merges the call's options
    // under the validator's own), and presence then rejects nil.
    if allow_nil == Some(true) {
        for draft in &mut drafts {
            if matches!(draft.rule, ValidationRule::Presence) && !draft.own_allow_nil {
                draft.nil_skip = true;
            }
        }
    }
    if let Some(flag) = allow_nil {
        for draft in &mut drafts {
            if let ValidationRule::Length { allow_nil: slot, .. }
            | ValidationRule::Numericality { allow_nil: slot, .. }
            | ValidationRule::Format { allow_nil: slot, .. }
            | ValidationRule::Acceptance { allow_nil: slot, .. } = &mut draft.rule
            {
                if slot.is_none() {
                    *slot = Some(flag);
                }
            }
        }
    }
    // A call-level `allow_blank:` reaches the numericality and format
    // validators as Rails passes it, under the validator's own key.
    if allow_blank {
        for draft in &mut drafts {
            if let ValidationRule::Numericality { allow_blank: slot, .. } | ValidationRule::Format { allow_blank: slot, .. } =
                &mut draft.rule
            {
                if slot.is_none() {
                    *slot = Some(true);
                }
            }
        }
    }

    let mut out = Vec::new();
    for attr in &attrs {
        if drafts.is_empty() {
            out.push(Validation {
                attribute: attr.clone(),
                rules: Vec::new(),
                if_cond: and_guard(on_guard.clone(), call_if_cond.clone()),
                unless_cond: call_unless_cond.clone(),
                message: None,
            });
            continue;
        }
        let mut grouped: Option<Validation> = None;
        for draft in &drafts {
            // A condition inside the rule hash replaces the call-level
            // one for that key — the other key still merges in.
            // The rule's own `on:` replaces the call's, as Rails merges the
            // call's options under each validator's own.
            let rule_on = draft.on_guard.clone().unwrap_or_else(|| on_guard.clone());
            let rule_on = if draft.nil_skip { and_guard(Some(not_nil(attr)), rule_on) } else { rule_on };
            let rule_on = if draft.blank_skip { and_guard(Some(not_blank(attr)), rule_on) } else { rule_on };
            let if_cond = and_guard(rule_on, draft.if_cond.clone().or_else(|| call_if_cond.clone()));
            let unless_cond = draft.unless_cond.clone().or_else(|| call_unless_cond.clone());
            if let Some(current) = grouped.as_mut() {
                if current.if_cond == if_cond && current.unless_cond == unless_cond && current.message == draft.message {
                    current.rules.push(draft.rule.clone());
                    continue;
                }
                out.push(grouped.take().unwrap());
            }
            grouped = Some(Validation {
                attribute: attr.clone(),
                rules: vec![draft.rule.clone()],
                if_cond,
                unless_cond,
                message: draft.message.clone(),
            });
        }
        if let Some(validation) = grouped {
            out.push(validation);
        }
    }
    if custom.is_empty() {
        ParsedValidates::Modeled(out)
    } else {
        ParsedValidates::Custom(out, custom)
    }
}

/// The validators Rails defines for a `validates` key, whether or not
/// this parser lowers them: never an app's `EachValidator`.
const BUILT_IN_VALIDATORS: &[&str] =
    &["presence", "absence", "acceptance", "length", "numericality", "inclusion", "exclusion", "uniqueness", "format", "comparison", "confirmation", "associated"];

/// `message: "…"` in a validator's own hash: Rails' text in place of the
/// default (after a model-specific translation, which the message lookup
/// still prefers). Only a literal String whose every `%{name}` the rule's
/// errors always supply: `value`, `attribute`, `model`, and `count` where
/// every error of the rule carries one (`counted`). Rails raises
/// `I18n::MissingInterpolationArgument` for any other name when the
/// message renders (`%{count}` on numericality's "is not a number"). A
/// Symbol (an i18n key), a Proc, or an interpolated Ruby String stays the
/// gap.
fn push_message(value: &Node<'_>, kind: &str, counted: bool, slot: &mut Option<String>, gaps: &mut Vec<String>) {
    let Some(text) = string_value(value) else {
        gaps.push(format!("{kind}: message"));
        return;
    };
    // The placeholders as I18n reads the String: `%%` is a literal `%`
    // (`%%{x}` names nothing), a `%<name>` format reference is refused,
    // and a `%<` without a name and `>` (`"must be %< 10"`) is text.
    let allowed = |name: &str| matches!(name, "value" | "attribute" | "model") || (counted && name == "count");
    let known = crate::i18n::pieces(&text)
        .is_ok_and(|pieces| pieces.iter().all(|p| !matches!(p, crate::i18n::Piece::Name(n) if !allowed(n))));
    if !known {
        gaps.push(format!("{kind}: message"));
        return;
    }
    *slot = Some(text);
}

/// `if:` / `unless:` — the same reader a callback uses. A Symbol is the
/// predicate called on the record; a zero-arity lambda/proc contributes
/// its body. A later value for the same key replaces the earlier one,
/// matching Ruby's last-duplicate-key-wins rule. Anything the reader
/// rejects (an arity>0 lambda, a String, an unsupported array, a
/// captured local, a body survey mode had to stub with `nil`) stays a
/// reported gap.
fn push_condition(
    value: &Node<'_>,
    file: &str,
    cond_slot: &mut Option<Expr>,
    option: &str,
    gaps: &mut Vec<String>,
) {
    // Survey mode turns an unsupported expression into `nil` and still
    // returns it. A guard built from that placeholder skips or runs
    // checks Rails would not. The expression error stays on the survey
    // ledger; the validates call is the reported gap.
    let before = super::super::survey::recorded().len();
    if let Some(cond) = super::callback_condition(value, file)
        && super::super::survey::recorded().len() == before
    {
        *cond_slot = Some(cond);
        return;
    }
    gaps.push(option.to_string());
}

/// `on:` as a guard on the record. Rails runs a validation in the context
/// `valid?` / `save` use: `:create` for a new record, `:update` for a
/// persisted one. So `on: :create` is `!self.persisted?`, `on: :update` is
/// `self.persisted?`, and an array naming both is no guard at all
/// (`Some(None)`). A custom context (`on: :publish`, run only by
/// `valid?(:publish)`) is not modeled (`None`, a gap).
///
/// `self.persisted?` rather than a bare `new_record?`: `persisted?` is
/// synthesized per model on every target, while `new_record?` lives on
/// the runtime Base, which a rust or go struct does not inherit (the same
/// spelling `model_to_library::markers` uses, for the same reason).
/// `!self.<attr>.nil?`, the guard a call-level `allow_nil:` puts on a rule
/// that stores no flag (presence).
/// `!self.<attr>.blank?`: Rails' `allow_blank:` skips nil, `""` and `"  "`.
fn not_blank(attr: &Symbol) -> Expr {
    let read = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: attr.clone(),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    let blank = Expr::new(
        Span::synthetic(),
        ExprNode::Send { recv: Some(read), method: Symbol::from("blank?"), args: Vec::new(), block: None, parenthesized: false },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Send { recv: Some(blank), method: Symbol::from("!"), args: Vec::new(), block: None, parenthesized: false },
    )
}

fn not_nil(attr: &Symbol) -> Expr {
    let read = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: attr.clone(),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    let nil = Expr::new(
        Span::synthetic(),
        ExprNode::Send { recv: Some(read), method: Symbol::from("nil?"), args: Vec::new(), block: None, parenthesized: false },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Send { recv: Some(nil), method: Symbol::from("!"), args: Vec::new(), block: None, parenthesized: false },
    )
}

pub(super) fn context_guard(value: &Node<'_>) -> Option<Option<Expr>> {
    let mut contexts: Vec<String> = Vec::new();
    if let Some(array) = value.as_array_node() {
        for element in array.elements().iter() {
            contexts.push(symbol_value(&element)?);
        }
    } else {
        contexts.push(symbol_value(value)?);
    }
    if contexts.is_empty() || contexts.iter().any(|c| c != "create" && c != "update") {
        return None;
    }
    let create = contexts.iter().any(|c| c == "create");
    let update = contexts.iter().any(|c| c == "update");
    let persisted = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: Symbol::from("persisted?"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    Some(match (create, update) {
        (true, true) => None,
        (false, true) => Some(persisted),
        _ => Some(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(persisted),
                method: Symbol::from("!"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )),
    })
}

/// Both guards hold (`on:`'s context guard and an `if:` condition), or
/// whichever one is present.
pub(super) fn and_guard(a: Option<Expr>, b: Option<Expr>) -> Option<Expr> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp { op: BoolOpKind::And, surface: BoolOpSurface::Symbol, left: a, right: b },
        )),
        (a, b) => a.or(b),
    }
}

/// `uniqueness: true` or `{ scope:, case_sensitive:, allow_nil:,
/// allow_blank:, if:, unless: }`. `conditions:` (a relation callable) and
/// `message:` stay gaps. A model with a `default_scope` lowers too: the
/// check queries `Model.unscoped`, as Rails' UniquenessValidator does.
fn parse_uniqueness(value: &Node<'_>, file: &str, _class_consts: &super::ClassConsts) -> RuleParse {
    use crate::dialect::ValidationRule;
    let rule = |scope, case_sensitive, allow_nil, allow_blank| ValidationRule::Uniqueness {
        scope,
        case_sensitive,
        allow_nil,
        allow_blank,
    };
    match super::super::util::bool_value(value) {
        Some(true) => return RuleParse::Rule(BuiltRule::bare(rule(Vec::new(), true, false, false))),
        Some(false) => return RuleParse::Skip,
        None => {}
    }
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec!["uniqueness".to_string()]);
    };
    let mut scope: Vec<Symbol> = Vec::new();
    let mut case_sensitive = true;
    let mut allow_nil = false;
    let mut allow_blank = false;
    let mut built = BuiltRule::bare(rule(Vec::new(), true, false, false));
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push("uniqueness".to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push("uniqueness".to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "scope" => {
                let names: Option<Vec<Symbol>> = if let Some(array) = child.as_array_node() {
                    array.elements().iter().map(|e| symbol_value(&e).map(|s| Symbol::from(s.as_str()))).collect()
                } else {
                    symbol_value(&child).map(|s| vec![Symbol::from(s.as_str())])
                };
                match names {
                    Some(names) => scope = names,
                    None => gaps.push("uniqueness: scope".to_string()),
                }
            }
            "case_sensitive" => match super::super::util::bool_value(&child) {
                Some(flag) => case_sensitive = flag,
                None => gaps.push("uniqueness: case_sensitive".to_string()),
            },
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(flag) => {
                    allow_nil = flag;
                    built.own_allow_nil = true;
                }
                None => gaps.push("uniqueness: allow_nil".to_string()),
            },
            "allow_blank" => match super::super::util::bool_value(&child) {
                Some(flag) => {
                    allow_blank = flag;
                    built.own_allow_blank = true;
                }
                None => gaps.push("uniqueness: allow_blank".to_string()),
            },
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps),
            "message" => push_message(&child, "uniqueness", false, &mut built.message, &mut gaps),
            other => gaps.push(format!("uniqueness: {other}")),
        }
    }
    if !gaps.is_empty() {
        return RuleParse::Gap(gaps);
    }
    built.rule = rule(scope, case_sensitive, allow_nil, allow_blank);
    RuleParse::Rule(built)
}

/// `allow_blank: true` skips every validator when the value is blank.
/// Presence is then a dead check (Rails runs validations before
/// `before_save`, which is what lobsters' `allow_blank: true, presence:
/// true` session token relies on). A max-only length passes on blank,
/// and absence wants blank. Anything else needs a skip-when-blank guard
/// this build does not lower.
fn blank_safe(rule: &crate::dialect::ValidationRule) -> bool {
    use crate::dialect::ValidationRule;
    match rule {
        // A blank value passes absence, and presence under `allow_blank:` is
        // dropped. A maximum-only length is not safe: `"  "` is blank and
        // two characters long, so `maximum: 1` would reject what Rails skips.
        ValidationRule::Presence | ValidationRule::Absence => true,
        _ => false,
    }
}

fn record_validation_gaps(
    file: &str,
    span: Span,
    method: &str,
    attrs: &[Symbol],
    options: &[String],
) {
    let targets: Vec<(String, String)> = if attrs.is_empty() {
        options
            .iter()
            .map(|option| (format!("{method} {option}:"), option.clone()))
            .collect()
    } else {
        attrs
            .iter()
            .flat_map(|attr| {
                options
                    .iter()
                    .map(move |option| (format!("{method} :{attr}, {option}:"), option.clone()))
            })
            .collect()
    };
    for (construct, option) in targets {
        let detail = if matches!(
            option.as_str(),
            "if" | "unless" | "on" | "allow_nil" | "allow_blank"
        ) || option.ends_with(": if")
            || option.ends_with(": unless")
        {
            "condition not lowered; emitting the check would reject records Rails accepts"
        } else {
            "validation option not lowered; Rails rejects records this build saves"
        };
        let mut diagnostic =
            crate::diagnostic::Diagnostic::unsupported(span, None, construct, detail);
        diagnostic.severity = crate::diagnostic::Severity::Warning;
        let message = diagnostic.message.clone();
        super::super::prism::push(diagnostic);
        super::super::survey::record(&IngestError::Unsupported {
            file: file.into(),
            message,
        });
    }
}

fn hash_elements<'a>(node: &Node<'a>) -> Option<ruby_prism::NodeList<'a>> {
    if let Some(hash) = node.as_hash_node() {
        return Some(hash.elements());
    }
    node.as_keyword_hash_node().map(|hash| hash.elements())
}

fn parse_of_options<'a>(
    kind: ValidatesKind,
    elements: &ruby_prism::NodeList<'a>,
    file: &str,
    allow_blank: &mut bool,
    allow_nil: &mut Option<bool>,
    call_if_cond: &mut Option<Expr>,
    call_unless_cond: &mut Option<Expr>,
    on_guard: &mut Option<Expr>,
    drafts: &mut Vec<BuiltRule>,
    gaps: &mut Vec<String>,
    class_consts: &super::ClassConsts,
) {
    use crate::dialect::ValidationRule;
    let mut only_integer = false;
    let mut gt: Option<f64> = None;
    let mut lt: Option<f64> = None;
    let mut gte: Option<f64> = None;
    let mut lte: Option<f64> = None;
    let mut inclusion: Option<InclusionSpec> = None;
    let mut pattern: Option<(String, String)> = None;
    let mut saw_rule_key = false;
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push("options".to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push("options".to_string());
            continue;
        };
        let value = assoc.value();
        match key.as_str() {
            "allow_blank" => match super::super::util::bool_value(&value) {
                Some(true) => *allow_blank = true,
                Some(false) => {}
                None => gaps.push("allow_blank".to_string()),
            },
            "allow_nil" => match super::super::util::bool_value(&value) {
                Some(flag) => *allow_nil = Some(flag),
                None => gaps.push("allow_nil".to_string()),
            },
            "if" => push_condition(&value, file, call_if_cond, "if", gaps),
            "unless" => push_condition(&value, file, call_unless_cond, "unless", gaps),
            "on" => match context_guard(&value) {
                Some(guard) => *on_guard = guard,
                None => gaps.push(key),
            },
            "strict" | "message" => gaps.push(key),
            "only_integer" if matches!(kind, ValidatesKind::Numericality) => {
                saw_rule_key = true;
                match super::super::util::bool_value(&value) {
                    Some(flag) => only_integer = flag,
                    None => gaps.push("numericality: only_integer".to_string()),
                }
            }
            "greater_than" if matches!(kind, ValidatesKind::Numericality) => {
                saw_rule_key = true;
                match bound_value(&value, class_consts) {
                    Some(n) => gt = Some(n),
                    None => gaps.push("numericality: greater_than".to_string()),
                }
            }
            "less_than" if matches!(kind, ValidatesKind::Numericality) => {
                saw_rule_key = true;
                match bound_value(&value, class_consts) {
                    Some(n) => lt = Some(n),
                    None => gaps.push("numericality: less_than".to_string()),
                }
            }
            "greater_than_or_equal_to" if matches!(kind, ValidatesKind::Numericality) => {
                saw_rule_key = true;
                match bound_value(&value, class_consts) {
                    Some(n) => gte = Some(n),
                    None => gaps.push("numericality: greater_than_or_equal_to".to_string()),
                }
            }
            "less_than_or_equal_to" if matches!(kind, ValidatesKind::Numericality) => {
                saw_rule_key = true;
                match bound_value(&value, class_consts) {
                    Some(n) => lte = Some(n),
                    None => gaps.push("numericality: less_than_or_equal_to".to_string()),
                }
            }
            "equal_to" | "other_than" if matches!(kind, ValidatesKind::Numericality) => {
                saw_rule_key = true;
                gaps.push(format!("numericality: {key}"));
            }
            "in" | "within" if matches!(kind, ValidatesKind::Inclusion | ValidatesKind::Exclusion) => {
                saw_rule_key = true;
                match inclusion_values(&value, class_consts) {
                    Some(spec) => inclusion = Some(spec),
                    None => gaps.push(format!("{}: in", set_label(&kind))),
                }
            }
            "with" if matches!(kind, ValidatesKind::Format) => {
                saw_rule_key = true;
                match resolve_format_regex(&value, class_consts) {
                    Some(resolved) => pattern = Some(resolved),
                    None => gaps.push("format: with".to_string()),
                }
            }
            other => {
                let prefix = match kind {
                    ValidatesKind::Numericality => "numericality",
                    ValidatesKind::Inclusion => "inclusion",
                    ValidatesKind::Exclusion => "exclusion",
                    ValidatesKind::Format => "format",
                    ValidatesKind::Map => "validates",
                };
                gaps.push(format!("{prefix}: {other}"));
            }
        }
    }
    if !gaps.is_empty() {
        return;
    }
    let rule = match kind {
        ValidatesKind::Numericality => ValidationRule::Numericality {
            only_integer,
            gt,
            lt,
            gte,
            lte,
            allow_nil: None,
            allow_blank: None,
            in_range: None,
        },
        ValidatesKind::Inclusion | ValidatesKind::Exclusion => match inclusion {
            Some(spec) => set_rule(spec, matches!(kind, ValidatesKind::Exclusion)),
            None if !saw_rule_key => {
                gaps.push(set_label(&kind).to_string());
                return;
            }
            None => {
                gaps.push(format!("{}: in", set_label(&kind)));
                return;
            }
        },
        ValidatesKind::Format => match pattern {
            Some((pattern, flags)) => ValidationRule::Format { pattern, flags, allow_nil: None, allow_blank: None },
            None => {
                gaps.push("format: with".to_string());
                return;
            }
        },
        ValidatesKind::Map => return,
    };
    drafts.push(BuiltRule::bare(rule));
}

fn parse_validator(key: &str, value: &Node<'_>, file: &str, class_consts: &super::ClassConsts) -> RuleParse {
    use crate::dialect::ValidationRule;
    match key {
        "presence" => parse_bool_or_hash(value, ValidationRule::Presence, "presence", file),
        "absence" => parse_bool_or_hash(value, ValidationRule::Absence, "absence", file),
        "acceptance" => parse_acceptance(value, file),
        "length" => parse_length_rule(value, file, class_consts),
        "numericality" => parse_numericality(value, file, class_consts),
        "inclusion" => parse_set(value, file, false, class_consts),
        "exclusion" => parse_set(value, file, true, class_consts),
        "uniqueness" => parse_uniqueness(value, file, class_consts),
        "format" => parse_format(value, file, class_consts),
        other => RuleParse::Gap(vec![other.to_string()]),
    }
}

/// `acceptance: true`, `acceptance: false`, or `acceptance: { accept:,
/// allow_nil:, … }`. Rails' `AcceptanceValidator` merges the options over
/// `{ allow_nil: true, accept: ["1", true] }`, so an unset `accept:` is that
/// default and an unset `allow_nil:` stays `None` (read as `true`). An
/// `accept:` value is a literal String, Integer or Boolean, or an Array of
/// them; anything else stays the gap.
fn parse_acceptance(value: &Node<'_>, file: &str) -> RuleParse {
    use crate::dialect::ValidationRule;
    let default_accept = || vec![Literal::Str { value: "1".to_string() }, Literal::Bool { value: true }];
    match super::super::util::bool_value(value) {
        Some(true) => {
            return RuleParse::Rule(BuiltRule::bare(ValidationRule::Acceptance { accept: default_accept(), allow_nil: None }))
        }
        Some(false) => return RuleParse::Skip,
        None => {}
    }
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec!["acceptance".to_string()]);
    };
    let mut accept = None;
    let mut allow_nil = None;
    let mut built = BuiltRule::bare(ValidationRule::Acceptance { accept: Vec::new(), allow_nil: None });
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push("acceptance".to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push("acceptance".to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "accept" => match acceptance_values(&child) {
                Some(values) => accept = Some(values),
                None => gaps.push("acceptance: accept".to_string()),
            },
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(flag) => {
                    allow_nil = Some(flag);
                    built.own_allow_nil = true;
                }
                None => gaps.push("allow_nil".to_string()),
            },
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps),
            "message" => push_message(&child, "acceptance", false, &mut built.message, &mut gaps),
            other => gaps.push(format!("acceptance: {other}")),
        }
    }
    if !gaps.is_empty() {
        return RuleParse::Gap(gaps);
    }
    built.rule = ValidationRule::Acceptance { accept: accept.unwrap_or_else(default_accept), allow_nil };
    RuleParse::Rule(built)
}

/// `accept: true`, `accept: "yes"` or `accept: [true, "true", "1"]`
/// (Rails reads it through `Array(...)`).
fn acceptance_values(node: &Node<'_>) -> Option<Vec<Literal>> {
    let atom = |node: &Node<'_>| {
        literal_atom(node).filter(|lit| matches!(lit, Literal::Str { .. } | Literal::Int { .. } | Literal::Bool { .. }))
    };
    match node.as_array_node() {
        Some(array) => array.elements().iter().map(|element| atom(&element)).collect(),
        None => atom(node).map(|lit| vec![lit]),
    }
}

/// `presence: true`, `presence: false`, or `presence: { if: :live? }`.
fn parse_bool_or_hash(
    value: &Node<'_>,
    rule: crate::dialect::ValidationRule,
    name: &str,
    file: &str,
) -> RuleParse {
    match super::super::util::bool_value(value) {
        Some(true) => return RuleParse::Rule(BuiltRule::bare(rule)),
        Some(false) => return RuleParse::Skip,
        None => {}
    }
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec![name.to_string()]);
    };
    let mut built = BuiltRule::bare(rule);
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push(name.to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push(name.to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => {
                push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps)
            }
            // `allow_nil: false` is Rails' default, but stated in the
            // rule's own hash it wins over a call-level `allow_nil: true`.
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(false) => built.own_allow_nil = true,
                _ => gaps.push("allow_nil".to_string()),
            },
            "message" => push_message(&child, name, false, &mut built.message, &mut gaps),
            other => gaps.push(format!("{name}: {other}")),
        }
    }
    if gaps.is_empty() {
        RuleParse::Rule(built)
    } else {
        RuleParse::Gap(gaps)
    }
}

/// Every key Rails 8.1.4's `NumericalityValidator` reads: its
/// `RESERVED_OPTIONS` (the compare checks, `odd` / `even`, `in`,
/// `only_integer`, `only_numeric`), the options every validator takes,
/// and the ones Rails consumes before the validator runs: scheduling's
/// `VALID_OPTIONS_FOR_VALIDATE` (`except_on`, `prepend`) and
/// `EachValidator`'s `attributes`, which a rule's own hash overrides the
/// validated fields with. Any other key is an interpolation option for
/// the error message, which the default messages never name, so Rails
/// ignores it.
const NUMERICALITY_OPTIONS: &[&str] = &[
    "greater_than",
    "greater_than_or_equal_to",
    "equal_to",
    "less_than",
    "less_than_or_equal_to",
    "other_than",
    "odd",
    "even",
    "in",
    "only_integer",
    "only_numeric",
    "allow_nil",
    "allow_blank",
    "message",
    "if",
    "unless",
    "on",
    "strict",
    "except_on",
    "prepend",
    "attributes",
];

/// A key the validator does not read: Rails ignores it, and so does the
/// lowering, with a note naming the key (and the option it most likely
/// meant: `greater_than_or_equal` for `greater_than_or_equal_to`).
fn ignored_validator_option(validator: &str, option: &str, known: &[&str], assoc: &ruby_prism::AssocNode<'_>, file: &str) {
    let hint = known.iter().find(|k| k.strip_suffix("_to") == Some(option)).map(|k| Symbol::from(*k));
    let kind = crate::diagnostic::DiagnosticKind::IgnoredValidatorOption {
        validator: Symbol::from(validator),
        option: Symbol::from(option),
        hint,
    };
    let location = assoc.location();
    let span = Span {
        file: super::super::sources::file_id(file),
        start: location.start_offset() as u32,
        end: location.end_offset() as u32,
    };
    super::super::prism::push(crate::diagnostic::Diagnostic {
        span,
        severity: crate::diagnostic::Diagnostic::default_severity(&kind),
        message: crate::diagnostic::Diagnostic::stub_text(&kind),
        kind,
    });
}

fn parse_numericality(value: &Node<'_>, file: &str, class_consts: &super::ClassConsts) -> RuleParse {
    use crate::dialect::ValidationRule;
    if super::super::util::bool_value(value) == Some(true) {
        return RuleParse::Rule(BuiltRule::bare(ValidationRule::Numericality {
            only_integer: false,
            gt: None,
            lt: None,
            gte: None,
            lte: None,
            allow_nil: None,
            allow_blank: None,
            in_range: None,
        }));
    }
    if super::super::util::bool_value(value) == Some(false) {
        return RuleParse::Skip;
    }
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec!["numericality".to_string()]);
    };
    let mut only_integer = false;
    let mut gt = None;
    let mut lt = None;
    let mut gte = None;
    let mut lte = None;
    let mut allow_nil = None;
    let mut allow_blank = None;
    let mut in_range = None;
    let mut built = BuiltRule::bare(ValidationRule::Numericality {
        only_integer: false,
        gt: None,
        lt: None,
        gte: None,
        lte: None,
        allow_nil: None,
        allow_blank: None,
        in_range: None,
    });
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push("numericality".to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push("numericality".to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "only_integer" => match super::super::util::bool_value(&child) {
                Some(flag) => only_integer = flag,
                None => gaps.push("numericality: only_integer".to_string()),
            },
            "greater_than" => match bound_value(&child, class_consts) {
                Some(n) => gt = Some(n),
                None => gaps.push("numericality: greater_than".to_string()),
            },
            "less_than" => match bound_value(&child, class_consts) {
                Some(n) => lt = Some(n),
                None => gaps.push("numericality: less_than".to_string()),
            },
            "greater_than_or_equal_to" => match bound_value(&child, class_consts) {
                Some(n) => gte = Some(n),
                None => gaps.push("numericality: greater_than_or_equal_to".to_string()),
            },
            "less_than_or_equal_to" => match bound_value(&child, class_consts) {
                Some(n) => lte = Some(n),
                None => gaps.push("numericality: less_than_or_equal_to".to_string()),
            },
            "in" => match bound_range(&child, class_consts) {
                Some(range) => in_range = Some(range),
                None => gaps.push("numericality: in".to_string()),
            },
            "equal_to" | "other_than" => gaps.push(format!("numericality: {key}")),
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => {
                push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps)
            }
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(flag) => allow_nil = Some(flag),
                None => gaps.push("allow_nil".to_string()),
            },
            "allow_blank" => match super::super::util::bool_value(&child) {
                Some(flag) => allow_blank = Some(flag),
                None => gaps.push("allow_blank".to_string()),
            },
            "message" => push_message(&child, "numericality", false, &mut built.message, &mut gaps),
            other if NUMERICALITY_OPTIONS.contains(&other) => gaps.push(format!("numericality: {other}")),
            other => ignored_validator_option("numericality", other, NUMERICALITY_OPTIONS, &assoc, file),
        }
    }
    if !gaps.is_empty() {
        return RuleParse::Gap(gaps);
    }
    built.rule = ValidationRule::Numericality {
        only_integer,
        gt,
        lt,
        gte,
        lte,
        allow_nil,
        allow_blank,
        in_range,
    };
    RuleParse::Rule(built)
}

/// `inclusion:` or, with `exclusion`, `exclusion:` — the same `in:` /
/// `within:` shapes, as a membership or a non-membership rule.
fn parse_set(value: &Node<'_>, file: &str, exclusion: bool, class_consts: &super::ClassConsts) -> RuleParse {
    let label = if exclusion { "exclusion" } else { "inclusion" };
    // `exclusion: false` / `inclusion: false` — Rails' `validates` skips
    // a validator whose options are false, keeping the call's other rules.
    if super::super::util::bool_value(value) == Some(false) {
        return RuleParse::Skip;
    }
    if let Some(spec) = inclusion_values(value, class_consts) {
        return RuleParse::Rule(BuiltRule::bare(set_rule(spec, exclusion)));
    }
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec![label.to_string()]);
    };
    let mut values = None;
    let mut built = BuiltRule::bare(set_rule(InclusionSpec::List(Vec::new()), exclusion));
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push(label.to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push(label.to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "in" | "within" => match inclusion_values(&child, class_consts).or_else(|| inclusion_call(&child, file)) {
                Some(spec) => values = Some(spec),
                None => gaps.push(format!("{label}: in")),
            },
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => {
                push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps)
            }
            // The rule's own option wins over the call's, as Rails merges
            // the call's options under each validator's own.
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(flag) => {
                    built.nil_skip = flag;
                    built.own_allow_nil = true;
                }
                None => gaps.push("allow_nil".to_string()),
            },
            "allow_blank" => match super::super::util::bool_value(&child) {
                Some(flag) => {
                    built.blank_skip = flag;
                    built.own_allow_blank = true;
                }
                None => gaps.push("allow_blank".to_string()),
            },
            "message" => push_message(&child, label, false, &mut built.message, &mut gaps),
            other => gaps.push(format!("{label}: {other}")),
        }
    }
    if !gaps.is_empty() {
        return RuleParse::Gap(gaps);
    }
    match values {
        Some(spec) => {
            built.rule = set_rule(spec, exclusion);
            RuleParse::Rule(built)
        }
        None => RuleParse::Gap(vec![format!("{label}: in")]),
    }
}

fn parse_format(value: &Node<'_>, file: &str, class_consts: &super::ClassConsts) -> RuleParse {
    use crate::dialect::ValidationRule;
    if let Some((pattern, flags)) = resolve_format_regex(value, class_consts) {
        return RuleParse::Rule(BuiltRule::bare(ValidationRule::Format { pattern, flags, allow_nil: None, allow_blank: None }));
    }
    if value.as_regular_expression_node().is_some()
        || value.as_interpolated_regular_expression_node().is_some()
    {
        // A flagged literal with a flag no target can reproduce exactly
        // (`/a/x`), or an interpolation this build can't flatten, is not
        // the pattern the lowerer emits.
        return RuleParse::Gap(vec!["format: with".to_string()]);
    }
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec!["format".to_string()]);
    };
    let mut pattern: Option<(String, String)> = None;
    let mut allow_nil = None;
    let mut allow_blank = None;
    let mut built = BuiltRule::bare(ValidationRule::Format {
        pattern: String::new(),
        flags: String::new(),
        allow_nil: None,
        allow_blank: None,
    });
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push("format".to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push("format".to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "with" => match resolve_format_regex(&child, class_consts) {
                Some(resolved) => pattern = Some(resolved),
                None => gaps.push("format: with".to_string()),
            },
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => {
                push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps)
            }
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(flag) => allow_nil = Some(flag),
                None => gaps.push("allow_nil".to_string()),
            },
            "allow_blank" => match super::super::util::bool_value(&child) {
                Some(flag) => allow_blank = Some(flag),
                None => gaps.push("allow_blank".to_string()),
            },
            "message" => push_message(&child, "format", false, &mut built.message, &mut gaps),
            other => gaps.push(format!("format: {other}")),
        }
    }
    if !gaps.is_empty() {
        return RuleParse::Gap(gaps);
    }
    match pattern {
        Some((pattern, flags)) => {
            built.rule = ValidationRule::Format { pattern, flags, allow_nil, allow_blank };
            RuleParse::Rule(built)
        }
        None => RuleParse::Gap(vec!["format: with".to_string()]),
    }
}

/// Resolve a `format: { with: … }` value (or a bare `format: /…/`
/// shorthand) to the regex literal it names, restricted to the flag
/// combinations every emit target this build ships can reproduce with
/// Ruby's exact meaning: no flags, `i` (case-insensitive) alone, `m`
/// (Ruby's dot-matches-newline) alone, or both together. `x`
/// (extended) has no faithful JS equivalent and `o` / an encoding flag
/// changes match semantics in ways no target mirrors, so either one
/// stays a gap rather than silently dropping. The value can be a
/// direct `/pattern/flags` literal, a model constant assigned one
/// (`NAME_RE`, `.freeze`d or aliased), or a constant assigned an
/// interpolation of exactly one other such constant whose flags agree
/// (`ClassConsts::regex` flattens that case).
fn resolve_format_regex(
    node: &Node<'_>,
    class_consts: &super::ClassConsts,
) -> Option<(String, String)> {
    let (pattern, flags) = class_consts
        .regex(node)
        .or_else(|| stdlib_regexp_constant(node, class_consts))
        .or_else(|| stdlib_regexp_call(node, class_consts))?;
    if flags.chars().all(|c| matches!(c, 'i' | 'm')) {
        Some((pattern, flags))
    } else {
        None
    }
}

/// A well-known Regexp constant of Ruby's standard library, named by
/// its full path (`URI::MailTo::EMAIL_REGEXP`), folded to its
/// `(source, flags)`. Consulted only after the class body's own
/// constants fail to resolve the value. The rooted spelling
/// (`::URI::MailTo::EMAIL_REGEXP`) always names the standard-library
/// constant. The unrooted one resolves lexically, so it folds only when
/// nothing could shadow its first segment: the class is not nested in
/// a module or class, its body defines no constant, module, or class by
/// that name, and no app class or module (so no app superclass or
/// included module) defines one either. Otherwise it stays the gap.
/// Neither spelling folds when the app itself defines or reopens the
/// top-level namespace (`URI = …`, `module URI`, `URI::MailTo` in
/// `lib/`), since it may have replaced the standard value.
///
/// Each entry is copied verbatim from
/// `ruby -ruri -e 'p CONST.source, CONST.options'` on Ruby 4.0, so the
/// rule is the one writing that literal inline would give (prism's
/// unescaped `/…\/…/` source equals Ruby's `Regexp#source`, `\#`
/// included). Any other constant stays the gap; a method that builds a
/// regex is [`stdlib_regexp_call`]'s.
fn stdlib_regexp_constant(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<(String, String)> {
    let (source, flags) = match stdlib_path(node, class_consts)?.as_str() {
        "URI::MailTo::EMAIL_REGEXP" => (
            r"\A[a-zA-Z0-9.!\#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z",
            "",
        ),
        _ => return None,
    };
    Some((source.to_string(), flags.to_string()))
}

/// `URI::DEFAULT_PARSER.make_regexp` (or the `RFC2396_PARSER` /
/// `RFC3986_PARSER` spelling, the same Regexp on Ruby 4.0), with no
/// schemes, under the same shadowing rules as [`stdlib_regexp_constant`].
///
/// Ruby's value is a 1440-character extended-mode (`/x`) pattern with
/// `(?#…)` comments and a `(?!//)` lookahead, which no strict target's
/// regex engine reads as Ruby does. It has no anchors, and everything
/// after the scheme's `:` is optional: the opaque and hierarchical parts,
/// the authority after a `//` (so a `//` matches with an empty one),
/// the path, query and fragment. So `match?` on it, which is all a format
/// rule asks, is true exactly when the string holds a scheme and its
/// colon: `[a-zA-Z][a-zA-Z0-9+.-]*:` (Ruby's `\d` is ASCII only, and the
/// `-` goes last so no target reads an escape). That pattern is what the
/// rule gets. With schemes
/// (`make_regexp(%w[http https])`) the pattern adds a lookahead on them
/// and stays the gap.
fn stdlib_regexp_call(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<(String, String)> {
    let call = node.as_call_node()?;
    if super::super::util::constant_id_str(&call.name()) != "make_regexp"
        || call.arguments().is_some()
        || call.block().is_some()
    {
        return None;
    }
    match stdlib_path(&call.receiver()?, class_consts)?.as_str() {
        "URI::DEFAULT_PARSER" | "URI::RFC2396_PARSER" | "URI::RFC3986_PARSER" => {
            Some(("[a-zA-Z][a-zA-Z0-9+.-]*:".to_string(), String::new()))
        }
        _ => None,
    }
}

/// A constant path (`URI::MailTo`, `::URI::DEFAULT_PARSER`) joined with
/// `::`, when it can only name the standard-library constant (see
/// [`stdlib_regexp_constant`]).
fn stdlib_path(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<String> {
    /// The path's segments, and whether it starts with `::`.
    fn segments(node: &Node<'_>, out: &mut Vec<String>) -> Option<bool> {
        if let Some(c) = node.as_constant_read_node() {
            out.push(super::super::util::constant_id_str(&c.name()).to_string());
            return Some(false);
        }
        let p = node.as_constant_path_node()?;
        let rooted = match p.parent() {
            Some(parent) => segments(&parent, out)?,
            None => true,
        };
        out.push(super::super::util::constant_id_str(&p.name()?).to_string());
        Some(rooted)
    }
    node.as_constant_path_node()?;
    let mut path = Vec::new();
    let rooted = segments(node, &mut path)?;
    // An app that defines or reopens the top-level namespace may have
    // replaced the standard value: neither spelling folds then.
    if class_consts.app_defines_top_level(path.first()?) {
        return None;
    }
    if !rooted && !class_consts.resolves_at_top_level(path.first()?) {
        return None;
    }
    Some(path.join("::"))
}

/// `length: { minimum: N, maximum: M }`. `is:`, `in:`, and `within:`
/// change which lengths Rails accepts; they are a gap, not a silent
/// drop beside a bound this build does lower.
fn parse_length_rule(value: &ruby_prism::Node<'_>, file: &str, class_consts: &super::ClassConsts) -> RuleParse {
    use crate::dialect::ValidationRule;
    let Some(elements) = hash_elements(value) else {
        return RuleParse::Gap(vec!["length".to_string()]);
    };
    let mut min: Option<u32> = None;
    let mut max: Option<u32> = None;
    let mut allow_nil: Option<bool> = None;
    let mut message: Option<String> = None;
    let mut built = BuiltRule::bare(ValidationRule::Length {
        min: None,
        max: None,
        message: None,
        allow_nil: None,
    });
    let mut gaps = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            gaps.push("length".to_string());
            continue;
        };
        let Some(key) = symbol_value(&assoc.key()) else {
            gaps.push("length".to_string());
            continue;
        };
        let child = assoc.value();
        match key.as_str() {
            "minimum" | "maximum" => match class_consts.integer(&child) {
                Some(n) if n >= 0 => {
                    let n = n as u32;
                    if key.as_str() == "minimum" {
                        min = Some(n);
                    } else {
                        max = Some(n);
                    }
                }
                _ => gaps.push(format!("length: {key}")),
            },
            "allow_nil" => match super::super::util::bool_value(&child) {
                Some(flag) => allow_nil = Some(flag),
                None => gaps.push("length: allow_nil".to_string()),
            },
            "message" => match string_value(&child) {
                Some(text) => message = Some(text),
                None => gaps.push("length: message".to_string()),
            },
            "on" => match context_guard(&child) {
                Some(guard) => built.on_guard = Some(guard),
                None => gaps.push("on".to_string()),
            },
            "if" => push_condition(&child, file, &mut built.if_cond, "if", &mut gaps),
            "unless" => {
                push_condition(&child, file, &mut built.unless_cond, "unless", &mut gaps)
            }
            "is" | "in" | "within" => gaps.push(format!("length: {key}")),
            other => gaps.push(format!("length: {other}")),
        }
    }
    if !gaps.is_empty() {
        return RuleParse::Gap(gaps);
    }
    if min.is_none() && max.is_none() {
        return RuleParse::Gap(vec!["length".to_string()]);
    }
    built.rule = ValidationRule::Length {
        min,
        max,
        message,
        allow_nil,
    };
    RuleParse::Rule(built)
}

/// A numericality bound: a number literal, or an integer constant of the
/// class (`less_than_or_equal_to: MAX_SCORE`).
fn bound_value(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<f64> {
    // An Integer bound, literal or folded (`LIMIT - 1`, `2**63`), compared
    // as a Float only when that Float is the same number: `2**63` is,
    // `2**63 + 1` is not, and stays the gap rather than a rounded bound.
    if let Some(n) = class_consts.int_expr(node) {
        let f = n as f64;
        return (f as i128 == n).then_some(f);
    }
    number_value(node)
}

/// `in: a..b` / `in: (a...b)` for numericality: both ends are Integer
/// bounds (literals, constants, Integer arithmetic). A Float end stays
/// the gap: the range is stored as `f64`, and Rails' message prints it
/// with `Range#inspect`, which shows `1.0..3.0` where an Integer range
/// shows `1..3`.
fn bound_range(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<crate::dialect::InclusionRange> {
    if let Some(paren) = node.as_parentheses_node() {
        let body = paren.body()?;
        let stmts = body.as_statements_node()?;
        let parts: Vec<Node<'_>> = stmts.body().iter().collect();
        let [only] = parts.as_slice() else { return None };
        return bound_range(only, class_consts);
    }
    let range = node.as_range_node()?;
    let end = |node: Node<'_>| class_consts.int_expr(&node).and_then(|_| bound_value(&node, class_consts));
    Some(crate::dialect::InclusionRange {
        begin: end(range.left()?)?,
        end: end(range.right()?)?,
        exclusive: range.is_exclude_end(),
    })
}

fn number_value(node: &Node<'_>) -> Option<f64> {
    if let Some(n) = super::super::util::integer_value(node) {
        return Some(n as f64);
    }
    node.as_float_node().map(|float| float.value())
}

enum InclusionSpec {
    List(Vec<Literal>),
    Range(crate::dialect::InclusionRange),
    /// A set the record answers at validation time (`in: ->(r) { … }`).
    Call(crate::expr::Expr),
}

/// The membership (`inclusion:`) or non-membership (`exclusion:`) rule for
/// an `in:` value.
fn set_rule(spec: InclusionSpec, exclusion: bool) -> crate::dialect::ValidationRule {
    use crate::dialect::ValidationRule;
    let (values, range) = match spec {
        InclusionSpec::List(values) => (values, None),
        InclusionSpec::Range(range) => (Vec::new(), Some(range)),
        InclusionSpec::Call(set) if exclusion => return ValidationRule::ExclusionCall { set },
        InclusionSpec::Call(set) => return ValidationRule::InclusionCall { set },
    };
    if exclusion {
        ValidationRule::Exclusion { values, range }
    } else {
        ValidationRule::Inclusion { values, range }
    }
}

fn set_label(kind: &ValidatesKind) -> &'static str {
    if matches!(kind, ValidatesKind::Exclusion) { "exclusion" } else { "inclusion" }
}

/// An array is list membership. A numeric range stays a range:
/// `Range#include?` covers the numbers between the endpoints, which a
/// list of the integers in between does not.
fn inclusion_values(node: &Node<'_>, class_consts: &super::ClassConsts) -> Option<InclusionSpec> {
    // `in: (MODES)` / `inclusion: ([…])`: the parentheses only group.
    if let Some(inner) = super::single_parenthesized(node) {
        return inclusion_values(&inner, class_consts);
    }
    // A list constant of the class (`in: MODES`, `MODES = %w[a b].freeze`),
    // or of another owner (`in: Fasp::DATA_CATEGORIES`).
    if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some() || node.as_call_node().is_some() {
        if let Some(list) = class_consts.list(node) {
            return Some(InclusionSpec::List(list));
        }
        // A Hash constant: Rails asks `NAME.include?(value)`, which is
        // `key?`. That compares with `eql?`, so a numeric key does not
        // hold an equal number of the other class (`{ 1 => … }` and
        // `1.0`) as the list check would: only non-numeric keys fold.
        let keys = class_consts.hash_keys_of(node)?;
        if keys.iter().any(|k| matches!(k, Literal::Int { .. } | Literal::Float { .. })) {
            return None;
        }
        return Some(InclusionSpec::List(keys));
    }
    if let Some(array) = node.as_array_node() {
        let mut out = Vec::new();
        for element in array.elements().iter() {
            out.push(
                literal_atom(&element)
                    .or_else(|| super::super::util::duration_seconds(&element).map(|value| Literal::Int { value }))?,
            );
        }
        return Some(InclusionSpec::List(out));
    }
    let range = node.as_range_node()?;
    let left = number_value(&range.left()?)?;
    let right = number_value(&range.right()?)?;
    Some(InclusionSpec::Range(crate::dialect::InclusionRange {
        begin: left,
        end: right,
        exclusive: range.is_exclude_end(),
    }))
}

/// `in: ->(record) { body }`, `in: -> { body }`, `lambda { |r| … }`,
/// `proc { … }` or `in: :method` (Rails' `ResolveValue`): a zero-arity
/// callable runs on the record (`instance_exec`), so its body reads as
/// written in the record's method; a one-parameter callable is called
/// with the record (`value.call(record)`), so its parameter becomes
/// `self`; a Symbol is that method of the record. A one-parameter body
/// keeps the class body's `self` in Rails, so one that calls a method
/// without a receiver or reads `self` would run on the record here, not
/// the class: it stays a gap, as does anything else (more parameters, a
/// body ingest can't read).
fn inclusion_call(node: &Node<'_>, file: &str) -> Option<InclusionSpec> {
    use crate::expr::{Expr, ExprNode};
    if let Some(name) = symbol_value(node) {
        return Some(InclusionSpec::Call(Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Send { recv: None, method: Symbol::from(name.as_str()), args: vec![], block: None, parenthesized: false },
        )));
    }
    let expr = super::super::expr::ingest_expr(node, file).ok()?;
    let lambda = match &*expr.node {
        ExprNode::Lambda { .. } => expr.clone(),
        // `lambda { |r| … }` / `proc { … }` / `Proc.new { … }`.
        ExprNode::Send { recv, method, args, block: Some(block), .. }
            if args.is_empty()
                && matches!(&*block.node, ExprNode::Lambda { .. })
                && match recv {
                    None => matches!(method.as_str(), "lambda" | "proc"),
                    Some(r) => method.as_str() == "new" && matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Proc"),
                } =>
        {
            block.clone()
        }
        _ => return None,
    };
    let ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. } = &*lambda.node else {
        return None;
    };
    if !extra_params.is_empty() || params.len() > 1 {
        return None;
    }
    // `-> { _1.allowed }` takes one parameter (arity 1, so Rails calls it
    // with the record), which the parsed lambda does not list. Two or more
    // (`_1`, `_2`) make Rails raise `ArgumentError`: a gap.
    let numbered = match numbered_parameters(node) {
        Some(0) | None => None,
        Some(1) => Some(Symbol::from("_1")),
        Some(_) => return None,
    };
    let mut body = body.clone();
    if let Some(param) = params.first().or(numbered.as_ref()) {
        if reads_implicit_self(&body) {
            return None;
        }
        replace_var_with_self(&mut body, param);
    }
    Some(InclusionSpec::Call(body))
}

/// The highest numbered parameter (`_1`, `_2`, …) a lambda literal or a
/// `lambda`/`proc`/`Proc.new` block reads, or `None` when it declares none.
fn numbered_parameters(node: &Node<'_>) -> Option<u8> {
    let params = if let Some(lambda) = node.as_lambda_node() {
        lambda.parameters()
    } else {
        node.as_call_node()?.block()?.as_block_node()?.parameters()
    };
    Some(params?.as_numbered_parameters_node()?.maximum())
}

/// Whether `e` calls a method without a receiver or reads `self`.
fn reads_implicit_self(e: &crate::expr::Expr) -> bool {
    use crate::expr::ExprNode;
    if matches!(&*e.node, ExprNode::SelfRef | ExprNode::Send { recv: None, .. }) {
        return true;
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || reads_implicit_self(c));
    found
}

/// The record a callable `in:` is given is the validating record itself.
/// A nested block that names a parameter the same rebinds it, so its body
/// is left alone.
fn replace_var_with_self(e: &mut crate::expr::Expr, name: &Symbol) {
    use crate::expr::ExprNode;
    match &*e.node {
        ExprNode::Var { name: n, .. } if n == name => {
            *e = crate::expr::Expr::new(e.span, ExprNode::SelfRef);
            return;
        }
        ExprNode::Lambda { params, rest_param, extra_params, block_param, .. }
            if params.contains(name)
                || rest_param.as_ref() == Some(name)
                || block_param.as_ref() == Some(name)
                || extra_params.iter().any(|p| p.name == *name) =>
        {
            return;
        }
        _ => {}
    }
    e.node.for_each_child_mut(&mut |c| replace_var_with_self(c, name));
}

fn literal_atom(node: &Node<'_>) -> Option<Literal> {
    if let Some(text) = string_value(node) {
        return Some(Literal::Str { value: text });
    }
    if let Some(text) = symbol_value(node) {
        return Some(Literal::Sym {
            value: Symbol::from(text.as_str()),
        });
    }
    if let Some(n) = super::super::util::integer_value(node) {
        return Some(Literal::Int { value: n });
    }
    if let Some(float) = node.as_float_node() {
        return Some(Literal::Float {
            value: float.value(),
        });
    }
    if let Some(flag) = super::super::util::bool_value(node) {
        return Some(Literal::Bool { value: flag });
    }
    node.as_nil_node().is_some().then_some(Literal::Nil)
}

