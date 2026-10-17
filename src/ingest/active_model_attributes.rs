//! `include ActiveModel::Attributes` on a plain class -> a reader, a
//! casting writer and a nil-filling `initialize`, and the include dropped.
//!
//! ```ruby
//! class Event
//!   include ActiveModel::Attributes
//!   attribute :subject_id, :integer
//!   attribute :timestamp, :datetime
//! end
//! ```
//!
//! Rails defines each accessor into a generated module and keeps the
//! values in an `AttributeSet`; no emitted tree has either, so the class
//! is flattened here, once in the IR, as `current_attributes` does. Each
//! writer casts as the declared `ActiveModel::Type` does in Rails 8.1:
//!
//! * `:string`: nil stays nil, `true`/`false` are `"t"`/`"f"`, a String
//!   is copied, anything else is `to_s`;
//! * `:integer`: nil, `true` 1, `false` 0, an Integer as is, a finite
//!   Float truncated (a non-finite one nil), a blank String nil and any
//!   other String `to_i` of its first 16 bytes (Rails' 4-byte limit);
//! * `:boolean`: `ActiveSupport.cast_boolean`, the existing seam;
//! * `:datetime`: `ActiveSupport.cast_datetime`, per tree beside
//!   `parse_time` (the CRuby/JRuby overlay is Rails' algorithm; Spinel's
//!   reads an ISO 8601 subset and raises on any other string).
//!
//! A value of another class raises `TypeError` where Rails would call
//! `to_i` (integer) or keep it (datetime): the static targets cannot ask
//! an arbitrary object, and raising is not a different answer.
//!
//! A class is left as it was, include and all (so `check` still reports
//! it), when anything here would be a guess: an `attribute` call with
//! no type, another type, options or a block; a class that defines
//! `initialize` or one of the accessors itself (Rails' would reach
//! the generated one through `super`); a superclass, which may declare
//! attributes of its own; or a class that also includes
//! `ActiveModel::Model`/`API`/`AttributeAssignment`, whose constructor
//! assigns through these writers.

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{ExprNode, Literal};
use crate::ident::Symbol;

const MODULE: &str = "ActiveModel::Attributes";

/// Modules whose constructor would assign through the writers this pass
/// generates; their combination is not lowered here.
const ASSIGNING: &[&str] =
    &["ActiveModel::Model", "ActiveModel::API", "ActiveModel::AttributeAssignment"];

#[derive(Clone, Copy)]
enum AttrType {
    String,
    Integer,
    Boolean,
    DateTime,
}

impl AttrType {
    fn from_symbol(name: &str) -> Option<AttrType> {
        match name {
            "string" => Some(AttrType::String),
            "integer" => Some(AttrType::Integer),
            "boolean" => Some(AttrType::Boolean),
            "datetime" => Some(AttrType::DateTime),
            _ => None,
        }
    }

    /// The right-hand side of the writer's ivar assignment.
    fn cast(self) -> &'static str {
        match self {
            AttrType::String => {
                "if value.nil?\n      nil\n    elsif value == true\n      \"t\".freeze\n    elsif value == false\n      \"f\".freeze\n    \
                 elsif value.is_a?(String)\n      value.dup\n    else\n      value.to_s\n    end"
            }
            AttrType::Integer => {
                "if value.nil?\n      nil\n    elsif value == true\n      1\n    elsif value == false\n      0\n    \
                 elsif value.is_a?(Integer)\n      value\n    elsif value.is_a?(Float)\n      value.finite? ? value.to_i : nil\n    \
                 elsif value.is_a?(String)\n      value.match?(/\\A[[:space:]]*\\z/) ? nil : value[0, 16].to_i\n    \
                 else\n      raise TypeError, \"can't cast #{value.class} to an integer\"\n    end"
            }
            AttrType::Boolean => "ActiveSupport.cast_boolean(value)",
            AttrType::DateTime => "ActiveSupport.cast_datetime(value)",
        }
    }
}

pub fn lower_active_model_attributes(app: &mut crate::App) {
    for lc in &mut app.library_classes {
        if !lc.includes.iter().any(|i| i.0.as_str() == MODULE) {
            continue;
        }
        let Some(attrs) = declared_attributes(lc) else { continue };
        let src = synthesized_source(lc, &attrs);
        // Its own `prism::scope`, as `current_attributes` does, so a parse
        // error here is never reported against an unrelated app file.
        let (parsed, diags) = crate::ingest::prism::scope(|| {
            crate::ingest::ingest_library_classes(src.as_bytes(), "<active_model_attributes>")
        });
        let methods: Vec<_> = match parsed {
            Ok(classes) if diags.is_empty() => classes.into_iter().flat_map(|c| c.methods).collect(),
            Ok(_) => {
                super::survey::record_synthesis_failure(
                    "<active_model_attributes>",
                    &format!("ActiveModel::Attributes accessors for `{}`", lc.name.0.as_str()),
                    &diags,
                );
                continue;
            }
            Err(err) => {
                super::survey::record(&err);
                continue;
            }
        };
        lc.methods.extend(methods);
        lc.unknown_calls.retain(|call| attribute_call(call).is_none());
        lc.includes.retain(|i| i.0.as_str() != MODULE);
    }
}

/// Every `attribute :name, :type` the class declares, or `None` when the
/// class is left as it was (see the module doc).
fn declared_attributes(lc: &LibraryClass) -> Option<Vec<(Symbol, AttrType)>> {
    if lc.is_module || lc.parent.is_some() {
        return None;
    }
    if lc.includes.iter().any(|i| ASSIGNING.contains(&i.0.as_str())) {
        return None;
    }
    let mut attrs: Vec<(Symbol, AttrType)> = Vec::new();
    for call in &lc.unknown_calls {
        let Some(args) = attribute_call(call) else { continue };
        let [name, ty] = args else { return None };
        let (Some(name), Some(ty)) = (sym(name), sym(ty)) else { return None };
        let ty = AttrType::from_symbol(ty.as_str())?;
        if attrs.iter().any(|(n, _)| *n == name) {
            return None;
        }
        attrs.push((name, ty));
    }
    if attrs.is_empty() {
        return None;
    }
    let defines = |name: &str| {
        lc.methods
            .iter()
            .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == name)
    };
    if defines("initialize")
        || attrs
            .iter()
            .any(|(n, _)| defines(n.as_str()) || defines(&format!("{}=", n.as_str())))
    {
        return None;
    }
    Some(attrs)
}

/// The arguments of a receiverless, blockless `attribute` call.
fn attribute_call(call: &crate::expr::Expr) -> Option<&[crate::expr::Expr]> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
        return None;
    };
    (method.as_str() == "attribute").then_some(args.as_slice())
}

fn sym(e: &crate::expr::Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

/// The accessors as Ruby, parsed by ingest like any class body.
fn synthesized_source(lc: &LibraryClass, attrs: &[(Symbol, AttrType)]) -> String {
    let class = lc.name.0.as_str();
    // Rails' `initialize(*)` takes no attribute hash without
    // `ActiveModel::Model`; every attribute reads nil until written.
    let mut body = String::from("  def initialize\n");
    for (name, _) in attrs {
        body.push_str(&format!("    @{} = nil\n", name.as_str()));
    }
    body.push_str("  end\n\n");
    for (name, ty) in attrs {
        let a = name.as_str();
        body.push_str(&format!("  def {a}\n    @{a}\n  end\n\n"));
        body.push_str(&format!("  def {a}=(value)\n    @{a} = {}\n  end\n\n", ty.cast()));
    }
    format!("class {class}\n{body}end\n")
}
