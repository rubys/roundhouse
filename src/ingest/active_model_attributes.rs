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
//! * `:string`: `ActiveSupport.cast_string` — nil stays nil,
//!   `true`/`false` are `"t"`/`"f"`, a String is copied, anything else
//!   is `to_s`;
//! * `:integer`: `ActiveSupport.cast_integer` — nil, `true` 1, `false` 0,
//!   an Integer as is, a finite Float truncated (a non-finite one nil),
//!   a blank String nil and any other String `to_i` of its first 16
//!   bytes (Rails' 4-byte limit);
//! * `:boolean`: `ActiveSupport.cast_boolean`, the existing seam;
//! * `:datetime`: `ActiveSupport.cast_datetime`, per tree beside
//!   `parse_time` (the CRuby/JRuby overlay is Rails' algorithm; Spinel's
//!   reads an ISO 8601 subset and raises on any other string).
//!
//! The writer returns the argument it was given. Rails'
//! `AttributeSet#write_from_user` does, and `obj.n = "08"` is `"08"`.
//!
//! `default:` is a literal (`true`, `false`, a number, a String, a
//! Symbol; `nil` is no default) or a lambda that takes no parameters. As
//! in Rails, the reader applies it on the attribute's first read, cast as
//! an assigned value is, and keeps it, and a write before that read means
//! the default is never read. The value lives in a class method,
//! `__default_<name>`, whose body is the literal or the lambda's, so a
//! lambda runs with the class as `self` and a String default is a fresh
//! String each time.
//!
//! A value of another class is nil for `:integer` where it is blank or
//! has no `to_i` (a Symbol, an Array, a Hash), as Rails' `to_i rescue
//! nil`, and its `to_i` otherwise. For `:datetime` it raises
//! `TypeError` where Rails would keep it: the static targets cannot ask
//! an arbitrary object, and raising is not a different answer.
//!
//! A class is left as it was, include and all (so `check` still reports
//! it), when anything here would be a guess: an `attribute` call with
//! no type, another type, an option other than a readable `default:`, a block, a receiver, or a trailing
//! `#:` cast; a class that defines `initialize` or one of the accessors
//! itself (Rails' would reach the generated one through `super`); a
//! superclass, which may declare attributes of its own; any other
//! `include` (that module's `initialize` would be skipped); or
//! `ActiveModel::Model`/`API`/`AttributeAssignment`, whose constructor
//! assigns through these writers.
//!
//! The synthesized `initialize` does not call `super`. After this pass
//! the only ancestor is `Object`, and a bare `super` is not a
//! constructor call on every target that emits the class (TypeScript
//! rejects `super()` with no `extends`; Kotlin emits a comment). Values
//! live in instance variables, so `dup` copies them; this is not
//! `Attributes#initialize_dup`.

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{ExprNode, Literal};
use crate::ident::Symbol;

const MODULE: &str = "ActiveModel::Attributes";

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

    /// The cast of `value`. The bodies live on `ActiveSupport`, beside
    /// `cast_boolean`, so every target compiles the same method.
    fn cast(self, value: &str) -> String {
        let method = match self {
            AttrType::String => "cast_string",
            AttrType::Integer => "cast_integer",
            AttrType::Boolean => "cast_boolean",
            AttrType::DateTime => "cast_datetime",
        };
        format!("ActiveSupport.{method}({value})")
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
        let mut methods: Vec<_> = match parsed {
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
        // Each default's class method carries the declared value: the
        // literal, or the lambda's body, which Rails calls with the class
        // as `self`.
        for attr in &attrs {
            let Some(default) = &attr.default else { continue };
            if let Some(method) = methods
                .iter_mut()
                .find(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == default_method(&attr.name))
            {
                method.body = default.clone();
            }
        }
        lc.methods.extend(methods);
        lc.unknown_calls.retain(|call| attribute_call(call).is_none());
        lc.includes.retain(|i| i.0.as_str() != MODULE);
    }
}

/// Every `attribute :name, :type` the class declares, or `None` when the
/// class is left as it was (see the module doc).
fn declared_attributes(lc: &LibraryClass) -> Option<Vec<Attr>> {
    if lc.is_module || lc.parent.is_some() {
        return None;
    }
    // Any other include may define `initialize`. Flattening would put
    // this `initialize` in front of it. A `super` call is not honest on
    // every target that emits the class, so the include stays.
    if lc.includes.iter().any(|i| i.0.as_str() != MODULE) {
        return None;
    }
    let mut attrs: Vec<Attr> = Vec::new();
    for call in &lc.unknown_calls {
        match attribute_declaration(call) {
            AttributeDecl::Absent => {}
            // A block, a receiver, a trailing `#:`, an option other than
            // `default:`, or another type: dropping it would keep the
            // include removed.
            AttributeDecl::Refuse => return None,
            AttributeDecl::Take(attr) => {
                if attrs.iter().any(|a| a.name == attr.name) {
                    return None;
                }
                attrs.push(attr);
            }
        }
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
            .any(|a| defines(a.name.as_str()) || defines(&format!("{}=", a.name.as_str())))
    {
        return None;
    }
    Some(attrs)
}

/// One declared attribute: its name, its type, and its `default:`, the
/// literal or the zero-arity lambda's body.
struct Attr {
    name: Symbol,
    ty: AttrType,
    default: Option<crate::expr::Expr>,
}

enum AttributeDecl {
    Absent,
    Refuse,
    Take(Attr),
}

/// The class method that answers an attribute's default.
fn default_method(name: &Symbol) -> String {
    format!("__default_{}", name.as_str())
}

/// `default: <literal>` or `default: -> { … }`. A literal is `true`,
/// `false`, a number, a String or a Symbol (`nil` is no default); a lambda
/// takes no parameters. Anything else is refused.
fn default_value(options: &crate::expr::Expr) -> Result<Option<crate::expr::Expr>, ()> {
    let ExprNode::Hash { entries, .. } = &*options.node else { return Err(()) };
    let [(key, value)] = entries.as_slice() else { return Err(()) };
    if sym(key).as_ref().map(|k| k.as_str()) != Some("default") {
        return Err(());
    }
    match &*value.node {
        ExprNode::Lit { value: Literal::Nil } => Ok(None),
        ExprNode::Lit {
            value: Literal::Bool { .. } | Literal::Int { .. } | Literal::Float { .. } | Literal::Str { .. } | Literal::Sym { .. },
        } => Ok(Some(value.clone())),
        ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. }
            if params.is_empty() && extra_params.is_empty() =>
        {
            Ok(Some(body.clone()))
        }
        _ => Err(()),
    }
}

/// One `attribute` declaration. A trailing `#:` is a cast around the
/// call (`current_attributes` unwraps those). Here the type is the
/// second symbol, so a cast, a receiver, or a block is a shape this
/// pass would drop.
fn attribute_declaration(call: &crate::expr::Expr) -> AttributeDecl {
    let (expr, cast) = match &*call.node {
        ExprNode::Cast { value, .. } => (value, true),
        _ => (call, false),
    };
    let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else {
        return AttributeDecl::Absent;
    };
    if method.as_str() != "attribute" {
        return AttributeDecl::Absent;
    }
    if cast || recv.is_some() || block.is_some() {
        return AttributeDecl::Refuse;
    }
    let (name, ty, default) = match args.as_slice() {
        [name, ty] => (name, ty, None),
        [name, ty, options] => match default_value(options) {
            Ok(default) => (name, ty, default),
            Err(()) => return AttributeDecl::Refuse,
        },
        _ => return AttributeDecl::Refuse,
    };
    let (Some(name), Some(ty)) = (sym(name), sym(ty)) else {
        return AttributeDecl::Refuse;
    };
    match AttrType::from_symbol(ty.as_str()) {
        Some(ty) => AttributeDecl::Take(Attr { name, ty, default }),
        None => AttributeDecl::Refuse,
    }
}

/// The arguments of a declaration this pass consumes.
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
fn synthesized_source(lc: &LibraryClass, attrs: &[Attr]) -> String {
    let class = lc.name.0.as_str();
    // Rails' `initialize(*)` takes no attribute hash without
    // `ActiveModel::Model`; an attribute reads its default, or nil, until
    // written. No `super`: see the module comment.
    let mut body = String::from("  def initialize\n");
    for attr in attrs {
        let a = attr.name.as_str();
        body.push_str(&format!("    @{a} = nil\n"));
        if attr.default.is_some() {
            body.push_str(&format!("    @{a}_default_applied = false\n"));
        }
    }
    body.push_str("  end\n\n");
    for attr in attrs {
        let a = attr.name.as_str();
        if attr.default.is_some() {
            // Rails reads a default on the attribute's first read, cast as
            // an assigned value is, and keeps it; a write before that read
            // means it is never read. The class method's body is the
            // declared default (`lower_active_model_attributes`).
            let default = default_method(&attr.name);
            body.push_str(&format!("  def self.{default}\n    nil\n  end\n\n"));
            body.push_str(&format!(
                "  def {a}\n    unless @{a}_default_applied\n      @{a} = {}\n      @{a}_default_applied = true\n    end\n    @{a}\n  end\n\n",
                attr.ty.cast(&format!("self.class.{default}"))
            ));
            body.push_str(&format!(
                "  def {a}=(value)\n    @{a} = {}\n    @{a}_default_applied = true\n    value\n  end\n\n",
                attr.ty.cast("value")
            ));
        } else {
            body.push_str(&format!("  def {a}\n    @{a}\n  end\n\n"));
            // The last expression is the assigned argument, as
            // `AttributeSet#write_from_user` returns it.
            body.push_str(&format!(
                "  def {a}=(value)\n    @{a} = {}\n    value\n  end\n\n",
                attr.ty.cast("value")
            ));
        }
    }
    format!("class {class}\n{body}end\n")
}
