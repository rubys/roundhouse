use ruby_prism::{CallNode, Node};

use crate::dialect::{LibraryClass, LibraryClassOrigin};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

use super::library_class::ingest_library_method_with_keywords;
use super::util::{constant_id_str, constant_path_of, flatten_statements, node_span};
use super::visibility::{self, Visibility};
use super::{IngestError, IngestResult, ingest_expr};

/// Recognize a constant-assigned `Data.define` block containing definitions or
/// visibility markers. This identifies a lifting candidate, not a validated
/// built-in factory; block-shape and constant-identity checks happen separately.
pub(super) fn declaration<'pr>(node: &Node<'pr>) -> Option<CallNode<'pr>> {
    let write = node.as_constant_write_node()?;
    let call = write.value().as_call_node()?;
    if constant_id_str(&call.name()) != "define"
        || constant_path_of(&call.receiver()?)?.join("::") != "Data"
        || call.block().is_none()
    {
        return None;
    }
    let body = call.block()?.as_block_node()?.body()?;
    flatten_statements(body)
        .iter()
        .any(|statement| {
            visibility::definition(statement).is_some()
                || statement
                    .as_call_node()
                    .is_some_and(|call| visibility::marker(&call))
        })
        .then_some(call)
}

/// Ingest the factory receiver and arguments without its lifted block. Retain
/// the call's source span so emission can reunite it with the collected methods
/// at the original constant declaration rather than hoisting a separate class.
pub(super) fn head(call: &CallNode<'_>, file: &str) -> IngestResult<Expr> {
    let recv = call
        .receiver()
        .map(|node| ingest_expr(&node, file))
        .transpose()?;
    let args = call
        .arguments()
        .map(|args| {
            args.arguments()
                .iter()
                .map(|node| ingest_expr(&node, file))
                .collect()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(Expr::new(
        node_span(&call.as_node(), file),
        ExprNode::Send {
            recv,
            method: Symbol::from("define"),
            args,
            block: None,
            parenthesized: true,
        },
    ))
}

/// Lift direct custom factory constants from an owner's body into library classes.
/// Preserve instance and `self` methods, keyword formals, and instance visibility;
/// reject block parameters and other statements instead of discarding them.
pub(super) fn collect(
    body: Option<Node<'_>>,
    owner: &ClassId,
    file: &str,
) -> IngestResult<Vec<LibraryClass>> {
    let mut factories = Vec::new();
    let Some(body) = body else {
        return Ok(factories);
    };
    for statement in flatten_statements(body) {
        let Some(call) = declaration(&statement) else {
            continue;
        };
        let unsupported = || {
            IngestError::Unsupported {
            file: file.into(),
            message: "Data.define blocks support only instance methods and static visibility declarations".into(),
        }
        };
        let block = call
            .block()
            .and_then(|node| node.as_block_node())
            .ok_or_else(unsupported)?;
        if block.parameters().is_some() {
            return Err(unsupported());
        }
        let Some(body) = block.body() else {
            return Err(unsupported());
        };
        let name = statement.as_constant_write_node().unwrap().name();
        let id = ClassId(Symbol::from(format!(
            "{}::{}",
            owner.0.as_str(),
            constant_id_str(&name)
        )));
        let visibility = Visibility::resolve(Some(&body), file, None)?;
        let mut methods = Vec::new();
        for statement in flatten_statements(body) {
            if let Some(def) = visibility::definition(&statement) {
                if def
                    .receiver()
                    .is_some_and(|receiver| receiver.as_self_node().is_none())
                {
                    return Err(unsupported());
                }
                let mut method = ingest_library_method_with_keywords(&def, &id, file, true)?;
                // Lexical defaults affect instance methods only, but explicit
                // visibility wrappers such as `private_class_method def self.helper`
                // also apply to singleton methods.
                visibility.apply(&statement, &mut method);
                methods.push(method);
            } else if !statement.as_call_node().is_some_and(|call| {
                visibility::marker(&call)
                    && call.receiver().is_none()
                    && call.block().is_none()
                    && matches!(
                        constant_id_str(&call.name()),
                        "public" | "protected" | "private"
                    )
            }) {
                return Err(unsupported());
            }
        }
        factories.push(LibraryClass {
            name: id,
            is_module: false,
            parent: Some(ClassId(Symbol::from("Data"))),
            parent_span: node_span(&call.receiver().unwrap(), file),
            includes: Vec::new(),
            methods,
            class_ivar_initializers: Vec::new(),
            nullable_columns: Vec::new(),
            origin: Some(LibraryClassOrigin::DataFactory {
                declaration_span: node_span(&call.as_node(), file),
            }),
            constants: Vec::new(),
            unknown_calls: Vec::new(),
        });
    }
    Ok(factories)
}
