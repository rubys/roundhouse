//! Classify syntax roles once, without deciding whether a use is safe.
//! `ancestors` excludes the current node; branch and leaf visits share this
//! contract. Prism can elide ArgumentsNode, so original call argument spans
//! distinguish argument values from receivers in this one classifier.

use ruby_prism::Node;

use super::super::super::expr;
use super::super::super::util::constant_id_str;
use super::{EnumConstants, ValueContext};

/// Class/module-body self is known. Method bodies and potentially rebound
/// blocks do not acquire that identity merely from lexical nesting.
pub(super) fn namespace_body(ancestors: &[Node<'_>]) -> bool {
    ancestors
        .iter()
        .rposition(|node| matches!(node, Node::ClassNode { .. } | Node::ModuleNode { .. }))
        .is_some_and(|scope| {
            !ancestors[scope + 1..].iter().any(|node| {
                matches!(
                    node,
                    Node::DefNode { .. }
                        | Node::BlockNode { .. }
                        | Node::LambdaNode { .. }
                        | Node::SingletonClassNode { .. }
                )
            })
        })
}

pub(super) fn value(ancestors: &[Node<'_>], node: &Node<'_>) -> ValueContext {
    for parent in ancestors.iter().rev() {
        match parent {
            Node::CallNode { .. } => {
                let call = parent.as_call_node().unwrap();
                if call.arguments().is_some_and(|args| {
                    args.arguments().iter().any(|argument| {
                        argument.location().start_offset() <= node.location().start_offset()
                            && argument.location().end_offset() >= node.location().end_offset()
                    })
                }) {
                    return ValueContext::Argument {
                        receiver: call.receiver().and_then(|r| EnumConstants::path(&r)),
                        method: constant_id_str(&call.name()).to_string(),
                    };
                }
                if call.receiver().is_none() {
                    return ValueContext::Discarded;
                }
                return if call.receiver().is_some_and(|r| {
                    EnumConstants::path(&r).is_some()
                        || (r.as_self_node().is_some() && namespace_body(ancestors))
                }) {
                    ValueContext::Receiver
                } else if super::super::serialized_enum_receiver(parent).is_some() {
                    ValueContext::SerializedPair
                } else {
                    ValueContext::Chained
                };
            }
            Node::LocalVariableWriteNode { .. }
            | Node::LocalVariableAndWriteNode { .. }
            | Node::LocalVariableOrWriteNode { .. }
            | Node::LocalVariableOperatorWriteNode { .. }
            | Node::InstanceVariableWriteNode { .. }
            | Node::ClassVariableWriteNode { .. }
            | Node::GlobalVariableWriteNode { .. } => return ValueContext::Stored,
            Node::ReturnNode { .. } | Node::DefNode { .. } | Node::BlockNode { .. } => {
                return ValueContext::Returned;
            }
            Node::ParenthesesNode { .. }
            | Node::ArgumentsNode { .. }
            | Node::StatementsNode { .. }
            | Node::ArrayNode { .. }
            | Node::HashNode { .. }
            | Node::KeywordHashNode { .. }
            | Node::AssocNode { .. }
            | Node::SplatNode { .. }
            | Node::ClassNode { .. }
            | Node::ModuleNode { .. } => {}
            Node::ProgramNode { .. } => return ValueContext::Discarded,
            _ => return ValueContext::Unmodeled,
        }
    }
    ValueContext::Discarded
}

/// The referenced path and its original use role. Declaration paths and write
/// targets are not value references. Assertions exempt only their bare T
/// receiver, never any values passed to them or their type arguments.
pub(super) fn reference(ancestors: &[Node<'_>], node: &Node<'_>) -> Option<(String, ValueContext)> {
    if !matches!(
        node,
        Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }
    ) {
        return None;
    }
    let parent = ancestors.last()?;
    if matches!(
        parent,
        Node::ConstantPathNode { .. } | Node::ClassNode { .. } | Node::ModuleNode { .. }
    ) {
        return None;
    }
    let destination = parent
        .as_constant_path_write_node()
        .map(|w| w.target().as_node())
        .or_else(|| {
            parent
                .as_constant_path_and_write_node()
                .map(|w| w.target().as_node())
        })
        .or_else(|| {
            parent
                .as_constant_path_or_write_node()
                .map(|w| w.target().as_node())
        })
        .or_else(|| {
            parent
                .as_constant_path_operator_write_node()
                .map(|w| w.target().as_node())
        });
    if destination
        .is_some_and(|target| target.location().start_offset() == node.location().start_offset())
    {
        return None;
    }
    if ancestors.iter().any(|parent| {
        expr::sorbet_assertion_argument(parent).is_some()
            && parent
                .as_call_node()
                .and_then(|call| call.receiver())
                .is_some_and(|receiver| {
                    receiver.location().start_offset() == node.location().start_offset()
                        && receiver.location().end_offset() == node.location().end_offset()
                })
    }) {
        return None;
    }
    // Constant containers/tuples may alias even when a call sits between the
    // assignment and this reference. Preserve that conservative source fact.
    let context = if ancestors.iter().any(|parent| {
        matches!(
            parent,
            Node::ConstantWriteNode { .. }
                | Node::ConstantAndWriteNode { .. }
                | Node::ConstantOrWriteNode { .. }
                | Node::ConstantOperatorWriteNode { .. }
                | Node::ConstantPathWriteNode { .. }
                | Node::ConstantPathAndWriteNode { .. }
                | Node::ConstantPathOrWriteNode { .. }
                | Node::ConstantPathOperatorWriteNode { .. }
                | Node::MultiWriteNode { .. }
        )
    }) {
        ValueContext::Stored
    } else {
        value(ancestors, node)
    };
    Some((EnumConstants::path(node)?, context))
}
