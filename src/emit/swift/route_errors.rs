//! Bridge the shared router's invalid-capture exception to Swift's HTTP
//! boundary. Application `ArgumentError` behavior is deliberately unchanged.

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use std::collections::HashSet;

const ERROR_CLASS: &str = "RoutePathEncodingError";

/// Register a catchable error only for the packaged Router entry. The normal
/// throwing-method fixpoint then propagates it through the shared match chain.
pub(super) fn prepare(classes: &mut [LibraryClass]) {
    super::expr::register_error_class(ERROR_CLASS.to_string());
    for class in classes {
        if !class.is_module || class.name.0.as_str() != "ActionDispatch::Router" {
            continue;
        }
        let methods: HashSet<Symbol> = class
            .methods
            .iter()
            .filter(|method| method.receiver == MethodReceiver::Class)
            .map(|method| method.name.clone())
            .collect();
        for method in &mut class.methods {
            if method.receiver != MethodReceiver::Class {
                continue;
            }
            rewrite_raises(&mut method.body);
            qualify_siblings(&mut method.body, &class.name, &methods);
        }
    }
}

/// Module declarations do not carry a class emission context. Qualifying this
/// closed Router's own calls lets the existing exception analysis resolve the
/// registered callee without changing general Swift module-throws semantics.
fn qualify_siblings(expr: &mut Expr, owner: &ClassId, methods: &HashSet<Symbol>) {
    if let ExprNode::Send { recv, method, .. } = expr.node.as_mut() {
        let implicit_self = recv
            .as_ref()
            .is_none_or(|receiver| matches!(*receiver.node, ExprNode::SelfRef));
        if implicit_self && methods.contains(method) {
            let mut receiver = recv
                .take()
                .unwrap_or_else(|| Expr::new(expr.span, ExprNode::SelfRef));
            receiver.node = Box::new(ExprNode::Const {
                path: owner.0.as_str().split("::").map(Symbol::from).collect(),
            });
            receiver.ty = Some(crate::ty::Ty::Class {
                id: owner.clone(),
                args: vec![].into(),
            });
            *recv = Some(receiver);
        }
    }
    expr.node
        .for_each_child_mut(&mut |child| qualify_siblings(child, owner, methods));
}

/// Preserve the exception message and all operands, replacing only the class
/// operand of a Router `raise ArgumentError` in either supported IR spelling.
fn rewrite_raises(expr: &mut Expr) {
    let mut constructed = None;
    match expr.node.as_mut() {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            ..
        } if method.as_str() == "raise" => {
            if args.len() == 1 && block.is_none() {
                if let ExprNode::Send {
                    recv: Some(class),
                    method,
                    block: None,
                    ..
                } = args[0].node.as_mut()
                {
                    if method.as_str() == "new" && rename_class(class) {
                        // Swift's existing constructor-error support consumes
                        // Raise, while Prism also uses a one-argument Send.
                        constructed = Some(args[0].clone());
                    }
                }
            }
            if constructed.is_none() {
                if let Some(class) = args.first_mut() {
                    rename_class(class);
                }
            }
        }
        ExprNode::Raise { value } => match value.node.as_mut() {
            ExprNode::Send {
                recv: Some(class),
                method,
                ..
            } if method.as_str() == "new" => {
                rename_class(class);
            }
            _ => {
                rename_class(value);
            }
        },
        _ => {}
    }
    if let Some(value) = constructed {
        expr.node = Box::new(ExprNode::Raise { value });
    }
    expr.node.for_each_child_mut(&mut rewrite_raises);
}

/// Do not relabel unrelated exception classes or arbitrary constant reads.
fn rename_class(expr: &mut Expr) -> bool {
    if let ExprNode::Const { path } = expr.node.as_mut() {
        if path.len() == 1 && path[0].as_str() == "ArgumentError" {
            path[0] = Symbol::from(ERROR_CLASS);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Modifier raises and constructed errors retain their operands, while
    /// ordinary constant reads and other error classes stay untouched.
    #[test]
    fn router_bridge_preserves_operands_and_unrelated_exceptions() {
        let source = r#"
module ActionDispatch
module Router
  def self.guarded(value)
    raise ArgumentError, value if value.empty?
  end
  def self.constructed(value)
    raise ArgumentError.new(value)
  end
  def self.constant
    ArgumentError
  end
  def self.other
    raise TypeError, "other"
  end
  def self.overridden(value)
    raise ArgumentError.new(value), "override"
  end
end
end
"#;
        let expected_source = source
            .replace(
                "raise ArgumentError, value",
                "raise RoutePathEncodingError, value",
            )
            .replace(
                "raise ArgumentError.new(value)\n",
                "raise RoutePathEncodingError.new(value)\n",
            );
        let mut classes =
            crate::ingest::ingest_library_classes(source.as_bytes(), "router.rb").unwrap();
        let original = classes.clone();
        let expected =
            crate::ingest::ingest_library_classes(expected_source.as_bytes(), "expected.rb")
                .unwrap();
        super::super::expr::reset_registries();
        prepare(&mut classes);
        assert_eq!(classes.len(), expected.len());
        for (actual, wanted) in classes.iter().zip(&expected) {
            assert_eq!(actual.methods.len(), wanted.methods.len());
            for (actual, wanted) in actual.methods.iter().zip(&wanted.methods) {
                assert_eq!(actual.name, wanted.name);
                let mut wanted_body = wanted.body.clone();
                if actual.name.as_str() == "constructed" {
                    // The bridge canonicalizes this one exception constructor
                    // to the emitter's already supported terminal Raise form.
                    let ExprNode::Send { args, .. } = wanted_body.node.as_mut() else {
                        panic!("expected constructor raise Send");
                    };
                    let value = args[0].clone();
                    wanted_body.node = Box::new(ExprNode::Raise { value });
                }
                assert_eq!(actual.body, wanted_body);
                let throws = super::super::expr::body_throws(&actual.body, "Router");
                assert_eq!(
                    throws,
                    matches!(actual.name.as_str(), "guarded" | "constructed")
                );
            }
        }
        for class in &original {
            for method in &class.methods {
                assert!(!super::super::expr::body_throws(&method.body, "Unrelated"));
            }
        }
    }

    /// Qualification is limited to Router sibling class-method dispatch and
    /// retains effectful arguments, blocks and call annotations unchanged.
    #[test]
    fn router_bridge_qualifies_only_its_own_implicit_sends() {
        let source = br#"
module ActionDispatch
  module Router
    def self.reject(value)
      raise ArgumentError, value
    end
    def self.bare(value)
      reject(touch(value)) { value }
    end
    def self.explicit(value)
      self.reject(value)
    end
    def self.other(value)
      value.reject(value)
    end
    def self.unknown(value)
      touch(value)
    end
  end
end
module Other
  def self.bare(value)
    reject(value)
  end
end
"#;
        let mut classes = crate::ingest::ingest_library_classes(source, "router_calls.rb").unwrap();
        let before = classes.clone();
        prepare(&mut classes);
        for (actual_class, original_class) in classes.iter().zip(&before) {
            for (actual, original) in actual_class.methods.iter().zip(&original_class.methods) {
                if actual_class.name.0.as_str() != "ActionDispatch::Router"
                    || matches!(actual.name.as_str(), "other" | "unknown")
                {
                    assert_eq!(actual.body, original.body);
                    continue;
                }
                if !matches!(actual.name.as_str(), "bare" | "explicit") {
                    continue;
                }
                let ExprNode::Send {
                    recv: Some(receiver),
                    args,
                    block,
                    ..
                } = &*actual.body.node
                else {
                    panic!("expected qualified sibling call");
                };
                assert!(matches!(&*receiver.node, ExprNode::Const { path }
                    if path.iter().map(|part| part.as_str()).collect::<Vec<_>>() == ["ActionDispatch", "Router"]));
                let ExprNode::Send {
                    args: original_args,
                    block: original_block,
                    ..
                } = &*original.body.node
                else {
                    panic!("expected original sibling call");
                };
                assert_eq!(args, original_args);
                assert_eq!(block, original_block);
                assert_eq!(actual.body.span, original.body.span);
                assert_eq!(actual.body.ty, original.body.ty);
                assert_eq!(actual.body.effects, original.body.effects);
                assert_eq!(actual.body.diagnostic, original.body.diagnostic);
            }
        }
    }
}
