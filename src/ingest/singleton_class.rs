//! `class << self … end` inside a class body: the class-side half of the
//! class, written as a block.
//!
//! Ruby evaluates the body with `self` set to the class's singleton
//! class, so:
//!
//! * a `def` defines a CLASS method (`Room.create_for`);
//! * `attr_accessor` / `attr_reader` / `attr_writer` define class-level
//!   accessors over the class's own instance variables;
//! * `alias_method` aliases a class method;
//! * `include Mod` mixes `Mod` into the singleton class — `Mod`'s
//!   instance methods become class methods, the same thing `extend Mod`
//!   says in the class body proper;
//! * a constant assignment lands in the singleton class's lexical
//!   scope, which the block's own methods read by bare name;
//! * `sig`, `abstract!`, `private_constant`, and the visibility markers
//!   carry no behavior the flattened method list can hold.
//!
//! Models and controllers both walk their class body one statement at a
//! time, and a `class << self` is one statement that stands for many, so
//! both expand it through [`ingest_singleton_body`]. (Library classes
//! keep their own richer walk in `library_class::walk_decl_body`.)
//!
//! ## Recovery
//!
//! A statement this walk cannot read costs itself. Before, the first
//! one aborted the whole block, and with it every `def` the block
//! declared: on Shop, Order and Product a single `RATE = 5` or `sig { … }`
//! erased the class's entire class-side API, and every call into it was
//! reported as a missing method. Strict mode still refuses the first
//! unreadable statement; survey mode records it and keeps reading.

use ruby_prism::Node;

use crate::dialect::{MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::library_class::{
    alias_source, is_sorbet_annotation_mixin, is_sorbet_type_alias, synth_attr_reader,
    synth_attr_writer, SORBET_ANNOTATIONS,
};
use super::util::{constant_id_str, flatten_statements, symbol_value};
use super::{IngestError, IngestResult};

/// What one `class << self` block declares.
#[derive(Default)]
pub(super) struct SingletonBody {
    /// The block's methods, all `MethodReceiver::Class`, in source order.
    pub methods: Vec<MethodDef>,
    /// Statements to replay in the enclosing class body: the block's
    /// constant assignments, and `extend Mod` for each `include Mod`.
    /// Held as raw expressions, exactly like any other class-body
    /// statement the enclosing walk does not classify.
    pub class_body: Vec<Expr>,
}

const VISIBILITY: &[&str] =
    &["private", "protected", "public", "private_class_method", "public_class_method"];

/// Expand one `class << self` block. `ingest_def` reads a `def` the way
/// the enclosing walk reads its own methods, so a controller's class
/// methods and a model's carry the parameter model of their side.
pub(super) fn ingest_singleton_body(
    sc: &ruby_prism::SingletonClassNode<'_>,
    owner: &ClassId,
    file: &str,
    ingest_def: &dyn Fn(&ruby_prism::DefNode<'_>) -> IngestResult<MethodDef>,
) -> IngestResult<SingletonBody> {
    let mut out = SingletonBody::default();
    // Only `class << self` is the enclosing class's own singleton. The
    // `class << other_object` form opens somebody else's.
    if sc.expression().as_self_node().is_none() {
        return unsupported(file, "unsupported `class << <expr>`: only `class << self` is modeled")
            .map(|()| out);
    }
    let Some(body) = sc.body() else { return Ok(out) };
    for stmt in flatten_statements(body) {
        walk_statement(&stmt, owner, file, ingest_def, &mut out)?;
    }
    Ok(out)
}

fn walk_statement(
    stmt: &Node<'_>,
    owner: &ClassId,
    file: &str,
    ingest_def: &dyn Fn(&ruby_prism::DefNode<'_>) -> IngestResult<MethodDef>,
    out: &mut SingletonBody,
) -> IngestResult<()> {
    if let Some(def) = stmt.as_def_node() {
        return push_def(&def, ingest_def, out);
    }
    if let Some(cw) = stmt.as_constant_write_node() {
        // `Result = T.type_alias { … }` is a type, not a value.
        if is_sorbet_type_alias(&cw.value()) {
            return Ok(());
        }
        return match ingest_expr(stmt, file) {
            Ok(expr) => {
                out.class_body.push(expr);
                Ok(())
            }
            Err(err) => recover(err),
        };
    }
    // `alias new old` copies the method as it stands, the same as
    // `alias_method` below and the library-class walk's `alias`.
    if let Some(alias) = stmt.as_alias_method_node() {
        let to = super::library_class::alias_keyword_name(&alias.new_name());
        let from = super::library_class::alias_keyword_name(&alias.old_name());
        let source = to.zip(from).and_then(|(to, from)| {
            out.methods
                .iter()
                .rposition(|m| m.name.as_str() == from && m.receiver == MethodReceiver::Class)
                .map(|i| (to, i))
        });
        let Some((to, source)) = source else {
            return unsupported(
                file,
                "unsupported statement inside `class << self`: `alias` of a method the block does not define",
            );
        };
        let mut copy = out.methods[source].clone();
        copy.name = Symbol::from(to.as_str());
        copy.name_span = crate::span::Span {
            file: super::sources::file_id(file),
            start: alias.location().start_offset() as u32,
            end: alias.location().end_offset() as u32,
        };
        out.methods.push(copy);
        return Ok(());
    }
    let Some(call) = stmt.as_call_node() else {
        return unsupported(file, &format!("unsupported statement inside `class << self`: {stmt:?}"));
    };
    let name = constant_id_str(&call.name()).to_string();
    let receiverless = call.receiver().is_none();

    // `sig { … }` and `T::Sig::WithoutRuntime.sig { … }` — types, read
    // by `sorbet_sig` already.
    if name == "sig" || (receiverless && SORBET_ANNOTATIONS.contains(&name.as_str())) {
        return Ok(());
    }
    if !receiverless {
        return unsupported(
            file,
            &format!("unsupported statement inside `class << self`: call to `{name}` on a receiver"),
        );
    }
    let args: Vec<Node<'_>> = call
        .arguments()
        .map(|a| a.arguments().iter().collect())
        .unwrap_or_default();

    // Visibility: `private` alone changes nothing this list carries;
    // `private def x` and `private_class_method def x` also define x.
    if VISIBILITY.contains(&name.as_str()) {
        for arg in &args {
            if let Some(def) = arg.as_def_node() {
                push_def(&def, ingest_def, out)?;
            }
        }
        return Ok(());
    }
    if matches!(name.as_str(), "private_constant" | "public_constant") {
        return Ok(());
    }
    // Deprecation annotates an existing method without changing the
    // class-side surface, just as it does in the library-class walk.
    if name == "deprecate" && call.block().is_none() {
        return Ok(());
    }

    match name.as_str() {
        "attr_reader" | "attr_writer" | "attr_accessor" => {
            for arg in &args {
                let Some(sym) = symbol_value(arg) else {
                    return unsupported(
                        file,
                        &format!("unsupported statement inside `class << self`: `{name}` with a non-symbol name"),
                    );
                };
                let sym = Symbol::from(sym.as_str());
                if name != "attr_writer" {
                    out.methods.push(synth_attr_reader(owner, &sym, MethodReceiver::Class));
                }
                if name != "attr_reader" {
                    out.methods.push(synth_attr_writer(owner, &sym, MethodReceiver::Class));
                }
            }
            Ok(())
        }
        // Copies the method as it stands, like the library-class walk.
        "alias_method" => match alias_source(&call, &out.methods, true) {
            Some((to, source)) => {
                let mut copy = out.methods[source].clone();
                copy.name = Symbol::from(to.as_str());
                out.methods.push(copy);
                Ok(())
            }
            None => unsupported(
                file,
                "unsupported statement inside `class << self`: `alias_method` of a method the block does not define",
            ),
        },
        "include" | "extend" if is_sorbet_annotation_mixin(&call) => Ok(()),
        // The singleton class including `Mod` is the class extending it.
        "include" => match ingest_expr(stmt, file) {
            Ok(mut expr) => {
                if let ExprNode::Send { method, .. } = &mut *expr.node {
                    *method = Symbol::from("extend");
                }
                out.class_body.push(expr);
                Ok(())
            }
            Err(err) => recover(err),
        },
        _ => unsupported(
            file,
            &format!("unsupported statement inside `class << self`: call to `{name}`"),
        ),
    }
}

fn push_def(
    def: &ruby_prism::DefNode<'_>,
    ingest_def: &dyn Fn(&ruby_prism::DefNode<'_>) -> IngestResult<MethodDef>,
    out: &mut SingletonBody,
) -> IngestResult<()> {
    match ingest_def(def) {
        Ok(mut method) => {
            method.receiver = MethodReceiver::Class;
            out.methods.push(method);
            Ok(())
        }
        Err(err) => recover(err),
    }
}

/// Survey mode records the gap and reads on; strict mode refuses.
fn recover(err: IngestError) -> IngestResult<()> {
    if super::survey::is_active() {
        super::survey::record(&err);
        Ok(())
    } else {
        Err(err)
    }
}

fn unsupported(file: &str, message: &str) -> IngestResult<()> {
    recover(IngestError::Unsupported { file: file.into(), message: message.into() })
}
