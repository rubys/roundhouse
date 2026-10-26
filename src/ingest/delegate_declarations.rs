//! Parse and validate `delegate` declarations before forwarder synthesis.

use super::*;
use crate::dialect::MethodVisibility;
use crate::expr::{ExprNode, Literal};
use crate::ident::Symbol;
use std::collections::{HashMap, HashSet};

/// One expanded `delegate` entry: `<name>` forwards to `<target>.<method>`.
pub(super) struct Delegation {
    pub(super) method: Symbol,
    pub(super) target: Symbol,
    pub(super) name: String,
    /// The real app file the `delegate` call was declared in, when the
    /// source registry can still resolve it. Empty when it can't.
    pub(super) file: String,
    pub(super) declaration_span: crate::span::Span,
    pub(super) visibility: MethodVisibility,
    /// `allow_nil: true`: the forwarder answers nil for a nil target
    /// (`delegate::synthesized_source`).
    pub(super) allow_nil: bool,
}

#[derive(Default)]
pub(super) struct CallShapes {
    positional_arities: HashSet<usize>,
    has_block: bool,
}

pub(super) type CallsWithArguments = HashMap<String, CallShapes>;

struct ParsedDeclaration {
    names: Vec<Symbol>,
    target: Symbol,
    prefix: DelegatePrefix,
    allow_nil: bool,
    has_unsupported_option: bool,
}

enum DelegatePrefix {
    None,
    Target,
    Explicit(String),
}

pub(super) struct DelegateEligibility<'a> {
    methods: &'a [MethodDef],
    called_with_args: &'a CallsWithArguments,
    blocked_names: &'a HashSet<String>,
    supported_target_methods: Option<&'a HashSet<(String, String, usize)>>,
    /// Targets the forwarder calls as a method of the declaring class
    /// itself, whose answer only analysis can type: exempt from
    /// `supported_target_methods`, which knows association targets only.
    method_targets: &'a HashSet<String>,
    sources: &'a [crate::span::SourceFile],
    visibility_cache: HashMap<crate::span::FileId, HashMap<usize, MethodVisibility>>,
}

impl<'a> DelegateEligibility<'a> {
    pub(super) fn new(
        methods: &'a [MethodDef],
        called_with_args: &'a CallsWithArguments,
        blocked_names: &'a HashSet<String>,
        supported_target_methods: Option<&'a HashSet<(String, String, usize)>>,
        method_targets: &'a HashSet<String>,
        sources: &'a [crate::span::SourceFile],
    ) -> Self {
        Self {
            methods,
            called_with_args,
            blocked_names,
            supported_target_methods,
            method_targets,
            sources,
            visibility_cache: HashMap::new(),
        }
    }

    fn eligible_entries(
        &mut self,
        call: &Expr,
        declaration: &ParsedDeclaration,
    ) -> Option<Vec<Delegation>> {
        if declaration.has_unsupported_option {
            return None;
        }
        let file = super::super::sources::path_of(call.span.file).unwrap_or_default();
        let visibility_defaults =
            self.visibility_cache
                .entry(call.span.file)
                .or_insert_with(|| {
                    // Clone text first — `declaration_defaults` re-parses via
                    // `prism::parse_silent`, which mutates the sources
                    // registry. `with_text` would hold an immutable borrow
                    // across that call and panic (`RefCell already borrowed`).
                    super::super::sources::text_of(&file)
                        .map(|source| {
                            super::super::visibility::Visibility::declaration_defaults(
                                &source, &file,
                            )
                        })
                        .or_else(|| {
                            call.span
                                .file
                                .0
                                .checked_sub(1)
                                .and_then(|index| self.sources.get(index as usize))
                                .map(|source| {
                                    super::super::visibility::Visibility::declaration_defaults(
                                        &source.text,
                                        &source.path,
                                    )
                                })
                        })
                        .unwrap_or_default()
                });
        let Some(&visibility) = visibility_defaults.get(&(call.span.start as usize)) else {
            super::super::survey::record(&crate::ingest::IngestError::Unsupported {
                file,
                message: "delegate visibility could not be resolved from its source".into(),
            });
            return None;
        };

        let mut entries = Vec::new();
        for method in &declaration.names {
            // Rails' allow_nil forwarder is `_ = target; _.m if !_.nil? ||
            // nil.respond_to?(:m)` (activesupport 8.1.4): a nil guard
            // exactly, unless nil answers `m` itself (`to_s`, `blank?`, …),
            // which Rails calls on nil. A setter or an operator is not
            // reproduced with allow_nil either.
            if declaration.allow_nil
                && (NIL_PUBLIC_METHODS.contains(&method.as_str())
                    || method.as_str().ends_with('=')
                    || super::operator_forwarder("", method.as_str()).is_some())
            {
                return None;
            }
            let name = generated_name(
                method.as_str(),
                &declaration.prefix,
                declaration.target.as_str(),
            );
            let earlier_local_definition = self.methods.iter().any(|definition| {
                definition.receiver == MethodReceiver::Instance
                    && definition.name.as_str() == name
                    && definition.name_span.file == call.span.file
                    && definition.name_span.start < call.span.start
            });
            let later_local_definition = self.methods.iter().any(|definition| {
                definition.receiver == MethodReceiver::Instance
                    && definition.name.as_str() == name
                    && definition.name_span.file == call.span.file
                    && definition.name_span.start > call.span.start
            });
            if self.blocked_names.contains(&name)
                && (!earlier_local_definition || later_local_definition)
            {
                return None;
            }
            if !method.as_str().ends_with('=') {
                if let Some(calls) = self.called_with_args.get(&name) {
                    let forwardable_operator = super::operator_forwarder("", method.as_str())
                        .map(|(params, _)| params.split(", ").count())
                        .is_some_and(|arity| {
                            !calls.has_block
                                && calls
                                    .positional_arities
                                    .iter()
                                    .all(|call_arity| *call_arity == arity)
                        });
                    if !forwardable_operator {
                        return None;
                    }
                }
            }
            entries.push(Delegation {
                method: method.clone(),
                target: declaration.target.clone(),
                name,
                file: file.clone(),
                declaration_span: call.span,
                visibility,
                allow_nil: declaration.allow_nil,
            });
        }
        if self.supported_target_methods.is_some_and(|supported| {
            entries.iter().any(|entry| {
                !self.method_targets.contains(entry.target.as_str())
                    && !supported.contains(&(
                    entry.target.as_str().to_string(),
                    entry.method.as_str().to_string(),
                    synthesized_arity(entry.method.as_str()),
                ))
            })
        }) {
            return None;
        }
        Some(entries)
    }
}

/// Consume declarations this pass can reproduce exactly, leaving other
/// shapes in `unknown_calls` rather than half-expanding them.
pub(super) fn take_delegate_decls_from_calls(
    unknown_calls: &mut Vec<Expr>,
    eligibility: &mut DelegateEligibility<'_>,
) -> Vec<Delegation> {
    let mut out: Vec<Delegation> = Vec::new();
    unknown_calls.retain(|call| {
        let Some(parsed) = parse_declaration(call) else {
            return true;
        };
        if parsed
            .names
            .iter()
            .any(|name| !valid_delegate_method(name.as_str()))
        {
            let file = super::super::sources::path_of(call.span.file).unwrap_or_default();
            let names = parsed
                .names
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            super::super::survey::record(&crate::ingest::IngestError::Unsupported {
                file,
                message: format!(
                    "delegate forwarder for `{names}` could not be synthesized: invalid method name"
                ),
            });
            return true;
        }
        let generated_names: Vec<_> = parsed
            .names
            .iter()
            .map(|method| generated_name(method.as_str(), &parsed.prefix, parsed.target.as_str()))
            .collect();
        if !matches!(&parsed.prefix, DelegatePrefix::None)
            && generated_names
                .iter()
                .any(|name| !valid_prefixed_method_name(name))
        {
            return true;
        }
        // An unsupported later declaration still replaces an earlier method
        // with the same generated name in Ruby's source order.
        out.retain(|existing| !generated_names.contains(&existing.name));
        let Some(entries) = eligibility.eligible_entries(call, &parsed) else {
            return true;
        };
        out.extend(entries);
        false
    });
    out
}

fn parse_declaration(call: &Expr) -> Option<ParsedDeclaration> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        ..
    } = &*call.node
    else {
        return None;
    };
    if call.diagnostic().is_some() || method.as_str() != "delegate" {
        return None;
    }

    let mut names = Vec::new();
    let mut target = None;
    let mut prefix = DelegatePrefix::None;
    let mut allow_nil = false;
    let mut has_unsupported_option = false;
    for arg in args {
        match &*arg.node {
            ExprNode::Lit {
                value: Literal::Sym { value },
            } => names.push(value.clone()),
            ExprNode::Hash { entries, .. } => {
                for (key, value) in entries {
                    let ExprNode::Lit {
                        value: Literal::Sym { value: key },
                    } = &*key.node
                    else {
                        has_unsupported_option = true;
                        continue;
                    };
                    match key.as_str() {
                        "to" => {
                            if let ExprNode::Lit {
                                value: Literal::Sym { value },
                            } = &*value.node
                            {
                                target = Some(value.clone());
                            } else {
                                has_unsupported_option = true;
                            }
                        }
                        "prefix" => match &*value.node {
                            ExprNode::Lit {
                                value: Literal::Bool { value: true },
                            } => prefix = DelegatePrefix::Target,
                            ExprNode::Lit {
                                value: Literal::Bool { value: false },
                            } => prefix = DelegatePrefix::None,
                            ExprNode::Lit {
                                value: Literal::Sym { value },
                            } => prefix = DelegatePrefix::Explicit(value.as_str().to_string()),
                            ExprNode::Lit {
                                value: Literal::Str { value },
                            } => prefix = DelegatePrefix::Explicit(value.clone()),
                            _ => has_unsupported_option = true,
                        },
                        // Rails tests respond_to? before returning nil for
                        // allow_nil, so a plain nil guard is equivalent only
                        // for a method nil does not answer
                        // (`eligible_entries`, `NIL_PUBLIC_METHODS`).
                        "allow_nil" => match &*value.node {
                            ExprNode::Lit {
                                value: Literal::Bool { value: false },
                            } => allow_nil = false,
                            ExprNode::Lit {
                                value: Literal::Bool { value: true },
                            } => allow_nil = true,
                            _ => has_unsupported_option = true,
                        },
                        _ => has_unsupported_option = true,
                    }
                }
            }
            _ => has_unsupported_option = true,
        }
    }
    let target = target?;
    if names.is_empty() || !valid_delegate_target(target.as_str()) {
        return None;
    }
    Some(ParsedDeclaration {
        names,
        target,
        prefix,
        allow_nil,
        has_unsupported_option,
    })
}

/// `nil.public_methods` in a Rails 8.1.4 process on Ruby 4.0.5
/// (ActiveSupport's core extensions included: `blank?`, `presence`,
/// `try`, …; oracle `nil_public_methods.out`). Re-verify on a Rails or
/// Ruby upgrade.
const NIL_PUBLIC_METHODS: &[&str] = &["!", "!=", "!~", "&", "<=>", "==", "===", "=~", "^", "__id__", "__send__", "acts_like?", "as_json", "blank?", "class", "clone", "deep_dup", "define_singleton_method", "display", "dup", "duplicable?", "enum_for", "eql?", "equal?", "extend", "freeze", "frozen?", "hash", "html_safe?", "in?", "inspect", "instance_eval", "instance_exec", "instance_of?", "instance_values", "instance_variable_defined?", "instance_variable_get", "instance_variable_names", "instance_variable_set", "instance_variables", "is_a?", "itself", "kind_of?", "method", "methods", "nil?", "object_id", "presence", "presence_in", "present?", "private_methods", "protected_methods", "public_method", "public_methods", "public_send", "rationalize", "remove_instance_variable", "respond_to?", "send", "singleton_class", "singleton_method", "singleton_methods", "tap", "then", "to_a", "to_c", "to_d", "to_enum", "to_f", "to_h", "to_i", "to_json", "to_param", "to_query", "to_r", "to_s", "to_yaml", "try", "try!", "with_options", "yield_self", "|"];

fn generated_name(method: &str, prefix: &DelegatePrefix, target: &str) -> String {
    match prefix {
        DelegatePrefix::None => method.to_string(),
        DelegatePrefix::Target => format!("{target}_{method}"),
        DelegatePrefix::Explicit(prefix) => format!("{prefix}_{method}"),
    }
}

pub(super) fn names_called_with_arguments(
    methods: &[MethodDef],
    additional_method_bodies: &[Expr],
) -> CallsWithArguments {
    let mut out = CallsWithArguments::new();
    // Class-side bodies cannot call these instance delegates without an
    // explicit receiver, so only instance methods constrain forwarding.
    for method in methods
        .iter()
        .filter(|method| method.receiver == crate::dialect::MethodReceiver::Instance)
    {
        collect_calls_with_args(&method.body, &mut out);
    }
    for expr in additional_method_bodies {
        collect_calls_with_args(expr, &mut out);
    }
    out
}

fn valid_prefixed_method_name(name: &str) -> bool {
    let base = name
        .strip_suffix('?')
        .or_else(|| name.strip_suffix('!'))
        .or_else(|| name.strip_suffix('='))
        .unwrap_or(name);
    let mut chars = base.chars();
    matches!(chars.next(), Some('_' | 'a'..='z' | 'A'..='Z'))
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn valid_delegate_target(target: &str) -> bool {
    let name = target.strip_prefix('@').unwrap_or(target);
    valid_identifier(name)
}

fn valid_delegate_method(name: &str) -> bool {
    matches!(name, "!" | "~" | "+@" | "-@")
        || super::operator_forwarder("", name).is_some()
        || valid_identifier(
            name.strip_suffix('?')
                .or_else(|| name.strip_suffix('!'))
                .or_else(|| name.strip_suffix('='))
                .unwrap_or(name),
        )
}

fn synthesized_arity(method: &str) -> usize {
    if let Some((params, _)) = super::operator_forwarder("", method) {
        return params.split(", ").count();
    }
    usize::from(method.ends_with('='))
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('_' | 'a'..='z' | 'A'..='Z'))
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn collect_calls_with_args(expr: &Expr, out: &mut CallsWithArguments) {
    expr.node
        .for_each_child(&mut |child| collect_calls_with_args(child, out));
    let ExprNode::Send {
        recv,
        method,
        args,
        block,
        ..
    } = &*expr.node
    else {
        return;
    };
    let instance_call = recv
        .as_ref()
        .is_none_or(|recv| matches!(&*recv.node, ExprNode::SelfRef));
    if instance_call && (!args.is_empty() || block.is_some()) {
        let calls = out.entry(method.as_str().to_string()).or_default();
        calls.positional_arities.insert(args.len());
        calls.has_block |= block.is_some();
    }
}
