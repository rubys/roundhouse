//! Specialize concern macros that define instance methods from symbols.
//!
//! Writebook's `positioned_within` closes over three symbol arguments in
//! parameterless `define_method` blocks, then marks the helpers private.
//! Turn that compile-time work into ordinary methods BEFORE inference,
//! once for every target. This is deliberately not a Ruby evaluator:
//! mutable captures, block parameters, nested blocks, control flow,
//! side effects outside the definitions and redefinitions stay unexpanded.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{MethodDef, MethodReceiver, MethodVisibility, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::SourceFile;

pub(crate) fn expand_model_macros(app: &mut App) {
    if app.concern_spliced_class_methods.is_empty() {
        return;
    }
    let mut params_specs =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    params_specs.mark_file_fields(&app.models);
    for model_index in 0..app.models.len() {
        let model = &app.models[model_index];
        let Some(origins) = app.concern_spliced_class_methods.get(&model.name) else {
            continue;
        };
        let macros: HashMap<_, _> = model
            .methods()
            .filter(|m| {
                m.receiver == MethodReceiver::Class
                    && origins.contains_key(&m.name)
                    && has_definition(&m.body)
            })
            .map(|m| (m.name.clone(), m.clone()))
            .collect();
        let mut candidates = Vec::new();
        let mut calls = HashMap::<Symbol, usize>::new();
        let mut included = HashSet::new();
        for (index, item) in model.body.iter().enumerate() {
            let ModelBodyItem::Unknown { expr, .. } = item else {
                continue;
            };
            let mut occurrences = Vec::new();
            macro_calls(expr, &macros, &mut occurrences);
            for name in &occurrences {
                *calls.entry(name.clone()).or_default() += 1;
            }
            let ExprNode::Send {
                recv,
                method,
                args,
                block,
                ..
            } = &*expr.node
            else {
                candidates.extend(occurrences.into_iter().map(|name| (index, name, None)));
                continue;
            };
            // Root calls are the only admitted placement. Calls nested
            // in conditionals/blocks must still poison partial expansion.
            let root_macro = macros.contains_key(method)
                && recv
                    .as_ref()
                    .is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
            if root_macro {
                occurrences.remove(0);
            }
            candidates.extend(occurrences.into_iter().map(|name| (index, name, None)));
            if recv.is_none() && method.as_str() == "include" && block.is_none() {
                for arg in args {
                    if let ExprNode::Const { path } = &*arg.node {
                        include_closure(
                            app,
                            ClassId(Symbol::from(
                                path.iter()
                                    .map(|p| p.as_str())
                                    .collect::<Vec<_>>()
                                    .join("::"),
                            )),
                            &mut included,
                        );
                    }
                }
            }
            // Count opaque self/block invocations too: otherwise a valid
            // first invocation could hide a redefinition in the second.
            if recv
                .as_ref()
                .is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef))
            {
                continue;
            }
            let Some(def) = macros.get(method) else {
                continue;
            };
            let origin = &origins[method];
            let providers = app
                .library_classes
                .iter()
                .filter(|c| included.contains(&c.name))
                .flat_map(|c| &c.methods)
                .filter(|m| m.receiver == MethodReceiver::Class && &m.name == method)
                .count();
            let methods = (recv.is_none()
                && block.is_none()
                && included.contains(origin)
                && providers == 1
                && ["define_method", "private", "protected", "public"]
                    .iter()
                    .all(|name| {
                        !crate::lower::scope_chain::app_method(
                            app,
                            &model.name,
                            &Symbol::from(*name),
                            MethodReceiver::Class,
                        )
                    })
                && supported_source_signature(def, &app.sources)
                && supported_source_call(expr, &app.sources))
            .then(|| expand(def, args, &app.sources))
            .flatten();
            candidates.push((index, method.clone(), methods));
        }
        if candidates.is_empty() {
            continue;
        }
        // Reuse the actual target-neutral synthesis inventory, without
        // typing bodies. A second list of Rails APIs would miss private
        // hooks such as validate and silently lose a generated helper.
        let synthesized = crate::lower::model_to_library::build_methods(
            model,
            &app.models,
            &app.schema,
            &params_specs,
        );
        let mut reserved: HashSet<_> = crate::lower::model_to_library::build_class_info(
            model,
            &synthesized,
            app.schema.tables.get(&model.table.0),
        )
        .instance_methods
        .into_keys()
        .collect();
        reserved.extend(
            synthesized
                .iter()
                .filter(|m| m.receiver == MethodReceiver::Instance)
                .map(|m| m.name.clone()),
        );
        // Reject BOTH declarations of a repeated macro or helper name:
        // the model lowerer currently keeps the first definition, whereas
        // Ruby keeps the last. An invalid second call must not let the
        // first one become a misleading partial expansion either.
        let mut names = HashMap::<Symbol, usize>::new();
        for (_, _, methods) in &candidates {
            let Some(methods) = methods else { continue };
            for m in methods {
                *names.entry(m.name.clone()).or_default() += 1;
            }
        }
        let mut expansions = HashMap::new();
        // An opaque macro could define any of the same names. Do not
        // invent a partial class when one candidate's effects are unknown.
        let opaque = candidates.iter().any(|(_, _, methods)| methods.is_none());
        for (index, name, methods) in candidates {
            let methods = methods.filter(|methods| {
                !opaque
                    && calls[&name] == 1
                    && methods.iter().all(|m| {
                        names[&m.name] == 1
                            && !reserved.contains(&m.name)
                            && !crate::lower::scope_chain::app_method(
                                app,
                                &model.name,
                                &m.name,
                                MethodReceiver::Instance,
                            )
                    })
            });
            if let Some(methods) = methods {
                expansions.insert(index, methods);
            } else {
                let span = model.body[index].span();
                let file = span
                    .file
                    .0
                    .checked_sub(1)
                    .and_then(|i| app.sources.get(i as usize))
                    .map(|s| s.path.clone())
                    .unwrap_or_else(|| model.name.to_string());
                super::survey::record(&super::IngestError::Unsupported {
                    file,
                    message: format!(
                        "model macro `{name}` not expanded: requires distinct parameterless definitions, symbol bindings and named visibility on those definitions"
                    ),
                });
            }
        }
        if expansions.is_empty() {
            continue;
        }
        let model = &mut app.models[model_index];
        let mut body = Vec::new();
        for (index, item) in std::mem::take(&mut model.body).into_iter().enumerate() {
            let Some(methods) = expansions.remove(&index) else {
                body.push(item);
                continue;
            };
            let ModelBodyItem::Unknown {
                mut leading_comments,
                mut leading_blank_line,
                ..
            } = item
            else {
                unreachable!()
            };
            for mut method in methods {
                method.enclosing_class = Some(model.name.0.clone());
                body.push(ModelBodyItem::Method {
                    method,
                    leading_comments: std::mem::take(&mut leading_comments),
                    leading_blank_line: std::mem::take(&mut leading_blank_line),
                });
            }
        }
        model.body = body;
    }
}

fn include_closure(app: &App, id: ClassId, included: &mut HashSet<ClassId>) {
    if !included.insert(id.clone()) {
        return;
    }
    if let Some(class) = app.library_classes.iter().find(|c| c.name == id) {
        for nested in &class.includes {
            include_closure(app, nested.clone(), included);
        }
    }
}

/// Library IR intentionally flattens some Ruby parameters. Expansion
/// needs the original signature: destructuring and anonymous rest may
/// otherwise disappear altogether and look like a simpler valid macro.
fn supported_source_signature(def: &MethodDef, sources: &[SourceFile]) -> bool {
    let Some(source) = def
        .name_span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))
    else {
        return false;
    };
    struct Signature {
        offset: usize,
        valid: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Signature {
        fn visit_def_node(&mut self, def: &ruby_prism::DefNode<'pr>) {
            if def.name_loc().start_offset() != self.offset {
                return;
            }
            self.valid = def.parameters().is_none_or(|params| {
                params.rest().is_none()
                    && params.keyword_rest().is_none()
                    && params.block().is_none()
                    && params.optionals().iter().next().is_none()
                    && params.posts().iter().next().is_none()
                    && params
                        .requireds()
                        .iter()
                        .all(|p| p.as_required_parameter_node().is_some())
                    && params.keywords().iter().all(|p| {
                        p.as_required_keyword_parameter_node().is_some()
                            || p.as_optional_keyword_parameter_node()
                                .is_some_and(|p| p.value().as_symbol_node().is_some())
                    })
            });
        }
    }
    let parsed = ruby_prism::parse(source.text.as_bytes());
    let mut signature = Signature {
        offset: def.name_span.start as usize,
        valid: false,
    };
    ruby_prism::Visit::visit(&mut signature, &parsed.node());
    signature.valid && parsed.errors().next().is_none()
}

/// In particular, `install(**:title)` must not become `install(:title)`
/// merely because expression ingest erased the keyword splat wrapper.
fn supported_source_call(expr: &Expr, sources: &[SourceFile]) -> bool {
    let Some(source) = expr
        .span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))
    else {
        return false;
    };
    let Some(bytes) = source
        .text
        .as_bytes()
        .get(expr.span.start as usize..expr.span.end as usize)
    else {
        return false;
    };
    let parsed = ruby_prism::parse(bytes);
    if parsed.errors().next().is_some() {
        return false;
    }
    let root = parsed.node();
    let Some(program) = root.as_program_node() else {
        return false;
    };
    let nodes: Vec<_> = program.statements().body().iter().collect();
    let [node] = nodes.as_slice() else {
        return false;
    };
    let Some(call) = node.as_call_node() else {
        return false;
    };
    call.receiver().is_none()
        && call.block().is_none()
        && call.arguments().is_none_or(|args| {
            args.arguments().iter().all(|arg| {
                arg.as_symbol_node().is_some()
                    || arg.as_keyword_hash_node().is_some_and(|hash| {
                        hash.elements().iter().all(|entry| {
                            entry.as_assoc_node().is_some_and(|entry| {
                                entry.key().as_symbol_node().is_some()
                                    && entry.value().as_symbol_node().is_some()
                            })
                        })
                    })
            })
        })
}

fn has_definition(body: &Expr) -> bool {
    if matches!(&*body.node, ExprNode::Send { method, .. } if method.as_str() == "define_method") {
        return true;
    }
    let mut found = false;
    body.node
        .for_each_child(&mut |child| found |= has_definition(child));
    found
}

fn macro_calls(expr: &Expr, macros: &HashMap<Symbol, MethodDef>, out: &mut Vec<Symbol>) {
    if let ExprNode::Send { recv, method, .. } = &*expr.node {
        if macros.contains_key(method)
            && recv
                .as_ref()
                .is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
        {
            out.push(method.clone());
        }
    }
    expr.node
        .for_each_child(&mut |child| macro_calls(child, macros, out));
}

fn symbol(expr: &Expr) -> Option<&Symbol> {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value),
        _ => None,
    }
}

/// Required positionals plus required/optional keywords. Optional
/// positionals, rest and forwarding need a fuller Ruby argument binder.
/// Keywords retain their SOURCE kind even where library ingest flattened
/// an optional keyword into a positional (`from_keyword`).
fn bindings(def: &MethodDef, args: &[Expr]) -> Option<HashMap<Symbol, Expr>> {
    if def.block_param.is_some()
        || def.params.iter().any(|p| {
            p.rest || p.from_kwrest || (!p.keyword && !p.from_keyword && p.default.is_some())
        })
    {
        return None;
    }
    let mut positional = args;
    let mut keywords = HashMap::new();
    if let Some(Expr { node, .. }) = args.last() {
        if let ExprNode::Hash {
            entries,
            kwargs: true,
        } = &**node
        {
            positional = &args[..args.len() - 1];
            for (key, value) in entries {
                if keywords
                    .insert(symbol(key)?.clone(), value.clone())
                    .is_some()
                {
                    return None;
                }
            }
        }
    }
    let mut positional = positional.iter();
    let mut out = HashMap::new();
    for param in &def.params {
        let value = if param.keyword || param.from_keyword {
            keywords
                .remove(&param.name)
                .or_else(|| param.default.clone())?
        } else {
            positional.next()?.clone()
        };
        // Symbols are immutable/interned: replacing their reads cannot
        // change capture identity or turn a shared mutable value into a
        // fresh allocation on each method call.
        symbol(&value)?;
        out.insert(param.name.clone(), value);
    }
    (positional.next().is_none() && keywords.is_empty()).then_some(out)
}

fn expand(def: &MethodDef, args: &[Expr], sources: &[SourceFile]) -> Option<Vec<MethodDef>> {
    let bindings = bindings(def, args)?;
    let statements = match &*def.body.node {
        ExprNode::Seq { exprs } => exprs.as_slice(),
        _ => std::slice::from_ref(&def.body),
    };
    let mut methods: Vec<MethodDef> = Vec::new();
    for statement in statements {
        let ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            ..
        } = &*statement.node
        else {
            return None;
        };
        if method.as_str() == "define_method" {
            let [name] = args.as_slice() else { return None };
            let name = substitute(name.clone(), &bindings)?;
            let name = symbol(&name)?.clone();
            // Ruby accepts arbitrary symbols in define_method, but an
            // emitted `def` must have a syntactically valid method name.
            let header = format!("def {name}\nend\n");
            let parsed = ruby_prism::parse(header.as_bytes());
            let node = parsed.node();
            let program = node.as_program_node()?;
            let nodes: Vec<_> = program.statements().body().iter().collect();
            let [node] = nodes.as_slice() else {
                return None;
            };
            let method_node = node.as_def_node()?;
            if parsed.errors().next().is_some()
                || method_node.receiver().is_some()
                || method_node.name().as_slice() != name.as_str().as_bytes()
                || methods.iter().any(|m| m.name == name)
            {
                return None;
            }
            let ExprNode::Lambda {
                params,
                rest_param: None,
                block_param: None,
                body,
                ..
            } = &*block.as_ref()?.node
            else {
                return None;
            };
            if !params.is_empty()
                || !supported_source_statement(statement, sources, true, &bindings)
            {
                return None;
            }
            let mut generated = def.clone();
            generated.name = name;
            generated.name_span = args[0].span;
            generated.receiver = MethodReceiver::Instance;
            generated.visibility = if matches!(
                generated.name.as_str(),
                "initialize" | "initialize_copy" | "initialize_dup" | "initialize_clone"
            ) {
                MethodVisibility::Private
            } else {
                MethodVisibility::Public
            };
            generated.params.clear();
            generated.block_param = None;
            generated.signature = None;
            generated.body = substitute(body.clone(), &bindings)?;
            methods.push(generated);
        } else {
            let visibility = match method.as_str() {
                "private" => MethodVisibility::Private,
                "protected" => MethodVisibility::Protected,
                "public" => MethodVisibility::Public,
                _ => return None,
            };
            if block.is_some()
                || args.is_empty()
                || !supported_source_statement(statement, sources, false, &bindings)
            {
                return None;
            }
            for arg in args {
                let name = substitute(arg.clone(), &bindings)?;
                let name = symbol(&name)?;
                methods.iter_mut().find(|m| &m.name == name)?.visibility = visibility;
            }
        }
    }
    (!methods.is_empty()).then_some(methods)
}

/// Lambda IR erases optional/keyword parameters and block-local names.
/// Consult the real source rather than mistaking an erased header for a
/// zero-argument block. Missing source means no proof, not permission.
fn supported_source_statement(
    statement: &Expr,
    sources: &[SourceFile],
    defining: bool,
    bindings: &HashMap<Symbol, Expr>,
) -> bool {
    let Some(source) = statement
        .span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))
    else {
        return false;
    };
    let Some(text) = source
        .text
        .as_bytes()
        .get(statement.span.start as usize..statement.span.end as usize)
    else {
        return false;
    };
    let parsed = ruby_prism::parse(text);
    if parsed.errors().next().is_some() {
        return false;
    }
    let node = parsed.node();
    struct ErasedSyntax(bool);
    impl<'pr> ruby_prism::Visit<'pr> for ErasedSyntax {
        fn visit_branch_node_enter(&mut self, node: ruby_prism::Node<'pr>) {
            self.0 |= node.as_assoc_splat_node().is_some()
                || node.as_splat_node().is_some()
                || node.as_def_node().is_some()
                || node.as_defined_node().is_some();
        }
    }
    let mut erased = ErasedSyntax(false);
    ruby_prism::Visit::visit(&mut erased, &node);
    if erased.0 {
        return false;
    }
    let Some(program) = node.as_program_node() else {
        return false;
    };
    let nodes: Vec<_> = program.statements().body().iter().collect();
    let [node] = nodes.as_slice() else {
        return false;
    };
    let Some(call) = node.as_call_node() else {
        return false;
    };
    if call.receiver().is_some() || !call.arguments().is_some_and(|args| {
        args.arguments().iter().all(|arg| {
            arg.as_symbol_node().is_some()
                || arg.as_local_variable_read_node().is_some()
                // This slice is parsed outside the enclosing def, so a
                // bound local name may parse as a bare receiverless call.
                || arg.as_call_node().is_some_and(|read| read.receiver().is_none()
                    && read.arguments().is_none() && read.block().is_none()
                    && std::str::from_utf8(read.name().as_slice()).is_ok_and(|name| bindings.contains_key(&Symbol::from(name))))
        })
    }) { return false; }
    if defining {
        call.block()
            .and_then(|block| block.as_block_node())
            .is_some_and(|block| block.parameters().is_none())
    } else {
        call.block().is_none()
    }
}

/// The supported body grammar has no local writes/bindings or nested
/// scopes. Never substitute by name through an arbitrary closure: ingest
/// has not assigned distinct VarIds to shadowed bindings yet.
fn substitute(mut expr: Expr, bindings: &HashMap<Symbol, Expr>) -> Option<Expr> {
    match &mut *expr.node {
        ExprNode::Var { name, .. } => return bindings.get(name).cloned(),
        // Moving a constant reference into the includer changes its
        // lexical binding (even String may be shadowed there).
        ExprNode::Lit { .. } | ExprNode::SelfRef | ExprNode::Ivar { .. } => {}
        ExprNode::Send { block: None, .. } | ExprNode::Seq { .. } => {
            let mut valid = true;
            expr.node.for_each_child_mut(&mut |child| {
                if let Some(replaced) = substitute(child.clone(), bindings) {
                    *child = replaced;
                } else {
                    valid = false;
                }
            });
            if !valid {
                return None;
            }
        }
        _ => return None,
    }
    Some(expr)
}
