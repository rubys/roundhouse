//! Validation lowering.
//!
//! Input: a `Model`'s surface validations (e.g. `Validation { attribute:
//! "body", rules: [Presence, Length { min: 10, max: None }] }`).
//!
//! Output: a list of `LoweredValidation` with `Check` entries each
//! describing one semantic check the evaluator must perform. A single
//! source rule may expand into multiple checks (e.g. `Length { min,
//! max }` lowers to two checks, `MinLength` and `MaxLength`, so the
//! per-target render doesn't carry optional-bound logic).
//!
//! The shape covers every target we ship:
//!
//! | Check         | TS render                  | Rust render                   |
//! |---------------|----------------------------|-------------------------------|
//! | Presence      | `x == null || x === ""`    | `self.x.is_empty()`           |
//! | Absence       | `x != null && x !== ""`    | `!self.x.is_empty()`          |
//! | MinLength n   | `x.length < n`             | `self.x.len() < n`            |
//! | MaxLength n   | `x.length > n`             | `self.x.len() > n`            |
//! | Format re     | `!re.test(x)`              | `!regex::Regex::….is_match`   |
//! | GreaterThan n | `x <= n`                   | `self.x <= n`                 |
//! | LessThan n    | `x >= n`                   | `self.x >= n`                 |
//! | OnlyInteger   | `!Number.isInteger(x)`     | — (type system guarantees)    |
//! | Inclusion vs  | `!vs.includes(x)`          | `![…].contains(&self.x)`      |
//! | Custom m      | `!this.m()` check          | `!self.m()`                   |
//!
//! Each render has a default error message but can be overridden; the
//! lowered form carries both a machine-friendly `Check` variant and a
//! human-readable `default_message()` helper.

use serde::{Deserialize, Serialize};

use crate::dialect::{Model, ValidationRule};
use crate::ident::Symbol;

/// One attribute with all the checks its validations expand into. The
/// order is preserved from the source — first-rule-first, and `Length
/// { min, max }` expands to MinLength-then-MaxLength when both bounds
/// are present, matching Rails' error-order conventions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoweredValidation {
    pub attribute: Symbol,
    pub checks: Vec<Check>,
}

/// A single semantic check. Each variant is named for the condition
/// that triggers a failure — `Presence` fails when the attr *is*
/// blank, `MinLength` fails when the attr is *shorter* than the
/// bound. Emitters render each to target-appropriate "if this, add
/// error" code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    /// The attr must be non-blank. Default message: `"can't be blank"`.
    Presence,
    /// The attr must be blank. Default message: `"must be blank"`.
    Absence,
    /// Length (char count or `len()`) must be ≥ `n`. `message` is a
    /// per-declaration override of the default text.
    /// Default message: `"is too short (minimum is {n} characters)"`.
    MinLength { n: u32, message: Option<String> },
    /// Length must be ≤ `n`.
    /// Default message: `"is too long (maximum is {n} characters)"`.
    MaxLength { n: u32, message: Option<String> },
    /// Numeric value must be > `threshold`.
    /// Default message: `"must be greater than {threshold}"`.
    GreaterThan { threshold: f64 },
    /// Numeric value must be < `threshold`.
    /// Default message: `"must be less than {threshold}"`.
    LessThan { threshold: f64 },
    /// Numeric value must be ≥ `threshold`.
    GreaterThanOrEqual { threshold: f64 },
    /// Numeric value must be ≤ `threshold`.
    LessThanOrEqual { threshold: f64 },
    /// Value must be an integer (no fractional part).
    /// Default message: `"must be an integer"`.
    OnlyInteger,
    /// Value must appear in the set of literal values.
    /// Default message: `"is not included in the list"`.
    Inclusion { values: Vec<InclusionValue> },
    /// Numeric `inclusion: { in: begin..end }`. `Range#include?` covers
    /// every number between the endpoints, so this is not a value list.
    /// Default message: `"is not included in the list"`.
    Cover { begin: f64, end: f64, exclusive: bool },
    /// Value must NOT appear in the set of literal values.
    /// Default message: `"is reserved"`.
    Exclusion { values: Vec<InclusionValue> },
    /// Numeric `exclusion: { in: begin..end }`: the value must not be
    /// covered by the range. Default message: `"is reserved"`.
    NotCover { begin: f64, end: f64, exclusive: bool },
    /// String must match the regex. `flags` is Ruby's own flag
    /// characters (`i`, `m`, or both — see `ValidationRule::Format`).
    /// Default message: `"is invalid"`.
    Format {
        pattern: String,
        #[serde(default)]
        flags: String,
    },
    /// String must be unique in the table (evaluator hits the DB).
    /// Default message: `"has already been taken"`.
    Uniqueness { scope: Vec<Symbol>, case_sensitive: bool },
    /// Call a model-defined method that populates errors itself.
    /// The evaluator just invokes it; no default message (the method
    /// controls its own error text).
    Custom { method: Symbol },
}

impl Check {
    /// Rails-compatible error message. Length checks honor the
    /// per-declaration `message:` override (carried through
    /// `ValidationRule::Length`); other checks add the field when a
    /// fixture needs it.
    pub fn default_message(&self) -> String {
        match self {
            Check::Presence => "can't be blank".into(),
            Check::Absence => "must be blank".into(),
            Check::MinLength { n, message } => message
                .clone()
                .unwrap_or_else(|| format!("is too short (minimum is {n} characters)")),
            Check::MaxLength { n, message } => message
                .clone()
                .unwrap_or_else(|| format!("is too long (maximum is {n} characters)")),
            Check::GreaterThan { threshold } => format!("must be greater than {threshold}"),
            Check::LessThan { threshold } => format!("must be less than {threshold}"),
            Check::GreaterThanOrEqual { threshold } => {
                format!("must be greater than or equal to {threshold}")
            }
            Check::LessThanOrEqual { threshold } => {
                format!("must be less than or equal to {threshold}")
            }
            Check::OnlyInteger => "must be an integer".into(),
            Check::Inclusion { .. } | Check::Cover { .. } => "is not included in the list".into(),
            Check::Exclusion { .. } | Check::NotCover { .. } => "is reserved".into(),
            Check::Format { .. } => "is invalid".into(),
            Check::Uniqueness { .. } => "has already been taken".into(),
            Check::Custom { .. } => String::new(),
        }
    }
}

/// A single literal value in an `Inclusion` set. Wraps the IR's
/// `Literal` type in a serializable form that matches the check's
/// evaluator conventions — Rails' inclusion lists are usually
/// strings or symbols (which lower to strings across all targets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InclusionValue {
    Str { value: String },
    Int { value: i64 },
    Float { value: f64 },
    Bool { value: bool },
}

/// Walk a model's validations and produce the lowered check sequence.
/// The output is flat — one `LoweredValidation` per attribute, with
/// all the checks for that attribute in source order. A single
/// `Validation` in the source IR that covers multiple rules on one
/// attribute produces one `LoweredValidation` here; `validates :title,
/// :body, presence: true` (two attrs) produces two.
pub fn lower_validations(model: &Model) -> Vec<LoweredValidation> {
    let mut out: Vec<LoweredValidation> = Vec::new();
    for validation in model.validations() {
        let mut checks: Vec<Check> = Vec::new();
        for rule in &validation.rules {
            expand_rule(rule, &mut checks);
        }
        if checks.is_empty() {
            continue;
        }
        out.push(LoweredValidation {
            attribute: validation.attribute.clone(),
            checks,
        });
    }
    out
}

/// Convert one surface `ValidationRule` into zero or more `Check`s.
/// `Length { min, max }` fans out into separate `MinLength` and
/// `MaxLength` checks so each emitter renders one condition at a
/// time. `Numericality` similarly splits into gt/lt/only_integer.
fn expand_rule(rule: &ValidationRule, out: &mut Vec<Check>) {
    match rule {
        ValidationRule::Presence => out.push(Check::Presence),
        ValidationRule::Absence => out.push(Check::Absence),
        // Not a `Check`: the emitted `validate` body is
        // `inline_acceptance_check`.
        ValidationRule::Acceptance { .. } => {}
        // Not a `Check`: the set is read on the record when it validates
        // (`inline_set_call_check`).
        ValidationRule::InclusionCall { .. } | ValidationRule::ExclusionCall { .. } => {}
        // `allow_nil` is not a `Check`. This fan-out is only read by
        // `tests/lower_validations.rs`. The emitted `validate` body is
        // `inline_length_check`, which is where nil is decided.
        ValidationRule::Length { min, max, message, allow_nil: _ } => {
            if let Some(n) = min {
                out.push(Check::MinLength { n: *n, message: message.clone() });
            }
            if let Some(n) = max {
                out.push(Check::MaxLength { n: *n, message: message.clone() });
            }
        }
        ValidationRule::Format { pattern, flags, .. } => {
            out.push(Check::Format { pattern: pattern.clone(), flags: flags.clone() });
        }
        ValidationRule::Numericality { only_integer, gt, lt, gte, lte, .. } => {
            if *only_integer {
                out.push(Check::OnlyInteger);
            }
            if let Some(n) = gt {
                out.push(Check::GreaterThan { threshold: *n });
            }
            if let Some(n) = lt {
                out.push(Check::LessThan { threshold: *n });
            }
            if let Some(n) = gte {
                out.push(Check::GreaterThanOrEqual { threshold: *n });
            }
            if let Some(n) = lte {
                out.push(Check::LessThanOrEqual { threshold: *n });
            }
        }
        ValidationRule::Inclusion { values, range } => {
            if let Some(range) = range {
                out.push(Check::Cover {
                    begin: range.begin,
                    end: range.end,
                    exclusive: range.exclusive,
                });
            }
            let vs: Vec<InclusionValue> = values
                .iter()
                .filter_map(lit_to_inclusion_value)
                .collect();
            if !vs.is_empty() {
                out.push(Check::Inclusion { values: vs });
            }
        }
        ValidationRule::Exclusion { values, range } => {
            if let Some(range) = range {
                out.push(Check::NotCover {
                    begin: range.begin,
                    end: range.end,
                    exclusive: range.exclusive,
                });
            }
            let vs: Vec<InclusionValue> = values
                .iter()
                .filter_map(lit_to_inclusion_value)
                .collect();
            if !vs.is_empty() {
                out.push(Check::Exclusion { values: vs });
            }
        }
        ValidationRule::Uniqueness { scope, case_sensitive, .. } => {
            out.push(Check::Uniqueness {
                scope: scope.clone(),
                case_sensitive: *case_sensitive,
            });
        }
        ValidationRule::Custom { method, .. } => {
            out.push(Check::Custom { method: method.clone() });
        }
    }
}

fn lit_to_inclusion_value(lit: &crate::expr::Literal) -> Option<InclusionValue> {
    use crate::expr::Literal;
    match lit {
        Literal::Str { value } => Some(InclusionValue::Str { value: value.clone() }),
        Literal::Sym { value } => Some(InclusionValue::Str { value: value.to_string() }),
        Literal::Int { value } => Some(InclusionValue::Int { value: *value }),
        Literal::Float { value } => Some(InclusionValue::Float { value: *value }),
        Literal::Bool { value } => Some(InclusionValue::Bool { value: *value }),
        Literal::Nil => None,
        Literal::Regex { .. } => None,
    }
}

/// `validates :attr, acceptance:` on an `attribute :attr, :boolean` is
/// lowered only on the targets whose writer keeps Rails' nil (the ruby
/// family, `BuildTarget::has_runtime_relation`). A strict target's field
/// turns nil and `""` into `false`, so the check would reject a record
/// Rails accepts; this reports the rule it leaves out, once per
/// validation, as `uniqueness_gaps_for_target` does.
#[cfg(not(target_arch = "wasm32"))]
pub fn acceptance_gaps_for_target(
    app: &crate::App,
    target: crate::build_target::BuildTarget,
) -> Vec<crate::diagnostic::Diagnostic> {
    if target.has_runtime_relation() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for model in &app.models {
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Validation { validation, .. } = item else { continue };
            if !validation.rules.iter().any(|r| matches!(r, ValidationRule::Acceptance { .. }))
                || !crate::lower::model_to_library::markers::synthesized_boolean_attribute(model, &validation.attribute)
            {
                continue;
            }
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                item.span(),
                Some(Symbol::from(target.as_str())),
                format!("validates :{}, acceptance:", validation.attribute.as_str()),
                "this target's Boolean attribute writer turns nil and \"\" into false, which the check would reject where Rails skips it",
            );
            d.severity = crate::diagnostic::Severity::Warning;
            out.push(d);
        }
    }
    out
}
