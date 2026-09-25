//! SQL functions an initializer registers on SQLite connections.
//!
//! Lobsters' `config/initializers/sqlite_functions.rb` patches the
//! adapter's `initialize` so every connection carries three functions
//! its raw SQL calls:
//!
//! ```ruby
//! raw_connection.create_function("regexp", 2) do |fn, pattern, expr|
//!   fn.result = expr.to_s.match(Regexp.new(pattern.to_s, Regexp::IGNORECASE)) ? 1 : 0
//! end
//! raw_connection.create_aggregate("stddev", 1) do
//!   step do |fn, value|
//!     next if value.nil?
//!     fn[:n] ||= 0
//!     …
//!   end
//!   finalize do |fn|
//!     fn.result = …
//!   end
//! end
//! ```
//!
//! The adapter patch around them is Rails plumbing with no counterpart
//! here; what carries over is the functions. Each call is read wherever
//! it sits in an initializer (receiver and nesting are irrelevant), and
//! each block becomes a method whose first parameter is the block's
//! context parameter under its own name, so the body is ingested as
//! written. A call whose name or arity is not a literal, or whose block
//! is not of these shapes, is skipped: registering a function whose
//! shape we guessed would answer SQL with the wrong values.

use crate::app::{SqlFunction, SqlFunctionKind};
use crate::dialect::{MethodDef, MethodReceiver, Param};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

use super::expr::ingest_expr;
use super::util::constant_id_str;

pub fn extract_sql_functions(source: &[u8], file: &str) -> Vec<SqlFunction> {
    struct V<'f> {
        file: &'f str,
        out: Vec<SqlFunction>,
    }

    impl<'pr> ruby_prism::Visit<'pr> for V<'_> {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let name = constant_id_str(&node.name());
            if matches!(name, "create_function" | "create_aggregate") {
                if let Some(f) = read_call(node, name == "create_aggregate", self.file) {
                    self.out.push(f);
                    return;
                }
            }
            ruby_prism::visit_call_node(self, node);
        }
    }

    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let mut v = V { file, out: Vec::new() };
    ruby_prism::Visit::visit(&mut v, &result.node());
    v.out
}

fn read_call(
    node: &ruby_prism::CallNode<'_>,
    aggregate: bool,
    file: &str,
) -> Option<SqlFunction> {
    let args: Vec<_> = node.arguments()?.arguments().iter().collect();
    if args.len() != 2 {
        return None;
    }
    let name = String::from_utf8_lossy(args[0].as_string_node()?.unescaped()).into_owned();
    let arity: usize = std::str::from_utf8(args[1].as_integer_node()?.location().as_slice())
        .ok()?
        .parse()
        .ok()?;
    let block = node.block()?.as_block_node()?;
    let kind = if aggregate {
        // `step do |fn, *args| … end` and `finalize do |fn| … end`, the
        // two calls inside the aggregate's block.
        let mut step = None;
        let mut finalize = None;
        for stmt in super::util::flatten_statements(block.body()?) {
            let call = stmt.as_call_node()?;
            if call.receiver().is_some() {
                return None;
            }
            let inner = call.block()?.as_block_node()?;
            match constant_id_str(&call.name()) {
                "step" => {
                    step = Some(method(&format!("agg_{name}_step"), &inner, arity + 1, file)?)
                }
                "finalize" => {
                    finalize = Some(method(&format!("agg_{name}_finalize"), &inner, 1, file)?)
                }
                _ => return None,
            }
        }
        SqlFunctionKind::Aggregate { step: step?, finalize: finalize? }
    } else {
        SqlFunctionKind::Scalar { method: method(&format!("fn_{name}"), &block, arity + 1, file)? }
    };
    Some(SqlFunction { name, arity, kind })
}

/// The block as a class method of `SqlFunctions`: its parameters (the
/// context first, then one per SQL argument — `want` in all), its body
/// with block-exiting `next` turned into `return`.
fn method(
    name: &str,
    block: &ruby_prism::BlockNode<'_>,
    want: usize,
    file: &str,
) -> Option<MethodDef> {
    let params_node = block.parameters()?.as_block_parameters_node()?.parameters()?;
    if params_node.optionals().iter().next().is_some()
        || params_node.rest().is_some()
        || params_node.keywords().iter().next().is_some()
    {
        return None;
    }
    let params: Vec<Param> = params_node
        .requireds()
        .iter()
        .map(|p| {
            p.as_required_parameter_node()
                .map(|r| Param::positional(Symbol::from(constant_id_str(&r.name()))))
        })
        .collect::<Option<_>>()?;
    if params.len() != want {
        return None;
    }
    let mut body = match block.body() {
        Some(b) => ingest_expr(&b, file).ok()?,
        None => Expr::new(crate::span::Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };
    next_to_return(&mut body);
    Some(MethodDef {
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from(name),
        receiver: MethodReceiver::Class,
        params,
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(Symbol::from("SqlFunctions")),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    })
}

/// `next` leaves the block; in the method it becomes, `return` does.
/// Not inside a nested block, where `next` still means that block's.
fn next_to_return(e: &mut Expr) {
    if let ExprNode::Next { value } = &mut *e.node {
        let value = value.take().unwrap_or_else(|| {
            Expr::new(e.span, ExprNode::Lit { value: crate::expr::Literal::Nil })
        });
        *e.node = ExprNode::Return { value };
        return;
    }
    if matches!(&*e.node, ExprNode::Lambda { .. }) {
        return;
    }
    if let ExprNode::Send { block: Some(_), .. } = &*e.node {
        // Recurse into receiver and args, not the block.
        if let ExprNode::Send { recv, args, .. } = &mut *e.node {
            if let Some(r) = recv {
                next_to_return(r);
            }
            for a in args {
                next_to_return(a);
            }
        }
        return;
    }
    e.node.for_each_child_mut(&mut |c| next_to_return(c));
}
