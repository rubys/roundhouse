//! Interpolatable `class_eval` string/heredoc expansion for model macros.
//!
//! Isolated from the `define_method` IR-substitution engine: this path
//! interpolates `#{param}`, rewrites remaining bound locals to symbol
//! literals by byte offset, re-parses, then re-ingests through
//! [`crate::ingest::model::ingest_model_body_items`]. Dynamic
//! `class_eval` and unknown interpolations stay unexpanded.

use std::collections::{HashMap, HashSet};

use crate::dialect::{MethodDef, ModelBodyItem};
use crate::ident::ClassId;
use crate::ingest::model::{ClassConsts, ingest_model_body_items};
use crate::ingest::prism::parse_silent;
use crate::ingest::util::constant_id_str;
use crate::span::SourceFile;

use super::{Expansion, bindings, interned_name};

use crate::ingest::prism_api::PrismLocation;
pub(super) fn expand(
    def: &MethodDef,
    args: &[crate::expr::Expr],
    sources: &[SourceFile],
    owner: &ClassId,
) -> Option<Expansion> {
    let bindings = bindings(def, args)?;
    let mut idents = HashMap::new();
    for (k, v) in &bindings {
        idents.insert(
            k.as_str().to_string(),
            interned_name(v)?.as_str().to_string(),
        );
    }
    // Optional non-symbol kwargs are omitted from `bindings`. A later
    // receiverless read of that name must decline, not drop the option.
    let param_names: HashSet<String> = def
        .params
        .iter()
        .map(|p| p.name.as_str().to_string())
        .collect();
    let source = def
        .name_span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))?;
    let parsed = crate::ingest::prism::parse_silent(source.text.as_bytes());
    if parsed.errors().next().is_some() {
        return None;
    }
    enum Piece {
        ClassEval(String),
        Stmt(String),
    }
    struct Collect {
        offset: usize,
        idents: HashMap<String, String>,
        pieces: Vec<Piece>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Collect {
        fn visit_def_node(&mut self, defn: &ruby_prism::DefNode<'pr>) {
            if defn.name_loc().start_offset() != self.offset {
                return;
            }
            let Some(body) = defn.body() else {
                return;
            };
            for stmt in crate::ingest::util::flatten_statements(body) {
                if let Some(call) = stmt.as_call_node() {
                    if call.receiver().is_none() && constant_id_str(&call.name()) == "class_eval" {
                        if let Some(template) = class_eval_template(&call, &self.idents) {
                            self.pieces.push(Piece::ClassEval(template));
                            continue;
                        }
                    }
                }
                let loc = stmt.location();
                self.pieces.push(Piece::Stmt(
                    String::from_utf8_lossy(loc.as_slice()).into_owned(),
                ));
            }
        }
    }
    let mut collect = Collect {
        offset: def.name_span.start as usize,
        idents: idents.clone(),
        pieces: Vec::new(),
    };
    ruby_prism::Visit::visit(&mut collect, &parsed.node());
    let mut methods = Vec::new();
    let mut items = Vec::new();
    let file = source.path.as_str();
    for piece in collect.pieces {
        let rewritten = match piece {
            Piece::ClassEval(template) => template,
            Piece::Stmt(src) => bind_local_reads(&src, &idents, &param_names)?,
        };
        ingest_rewritten_body(&rewritten, owner, file, &mut methods, &mut items)?;
    }
    // Any leftover Unknown (interpolated association name, `scope`,
    // non-symbol kwarg) declines the *entire* expansion, including
    // already-rewritten class_eval methods. That is fail-closed, not
    // a support claim — pin with a negative overlay.
    if items.iter().any(|item| {
        matches!(item, ModelBodyItem::Unknown { expr, .. } if super::mixin_args(expr).is_none())
    }) {
        return None;
    }
    (!methods.is_empty() || !items.is_empty()).then_some(Expansion { methods, items })
}

fn ingest_rewritten_body(
    src: &str,
    owner: &ClassId,
    file: &str,
    methods: &mut Vec<MethodDef>,
    items: &mut Vec<ModelBodyItem>,
) -> Option<()> {
    let parsed = parse_silent(src.as_bytes());
    if parsed.errors().next().is_some() {
        return None;
    }
    let program = parsed.node().as_program_node()?;
    for stmt in program.statements().body().iter() {
        absorb_items(
            ingest_model_body_items(&stmt, owner, file, Vec::new(), None, &ClassConsts::default()).ok()?,
            methods,
            items,
        );
    }
    Some(())
}

fn absorb_items(
    ingested: Vec<ModelBodyItem>,
    methods: &mut Vec<MethodDef>,
    items: &mut Vec<ModelBodyItem>,
) {
    for item in ingested {
        match item {
            ModelBodyItem::Method { method, .. } => methods.push(method),
            other => items.push(other),
        }
    }
}

fn class_eval_template(
    call: &ruby_prism::CallNode<'_>,
    idents: &HashMap<String, String>,
) -> Option<String> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next()?;
    if let Some(s) = first.as_string_node() {
        return Some(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    interpolate_string_node(&first, idents)
}

fn interpolate_string_node(
    node: &ruby_prism::Node<'_>,
    idents: &HashMap<String, String>,
) -> Option<String> {
    if let Some(s) = node.as_string_node() {
        return Some(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    let interp = node.as_interpolated_string_node()?;
    let mut out = String::new();
    for part in interp.parts().iter() {
        if let Some(s) = part.as_string_node() {
            out.push_str(&String::from_utf8_lossy(s.unescaped()));
        } else if let Some(es) = part.as_embedded_statements_node() {
            let stmts = es.statements()?;
            let nodes: Vec<_> = stmts.body().iter().collect();
            let [only] = nodes.as_slice() else {
                return None;
            };
            let name = only
                .as_local_variable_read_node()
                .map(|n| String::from_utf8_lossy(n.name().as_slice()).into_owned())
                .or_else(|| {
                    only.as_call_node().and_then(|c| {
                        (c.receiver().is_none() && c.arguments().is_none() && c.block().is_none())
                            .then(|| String::from_utf8_lossy(c.name().as_slice()).into_owned())
                    })
                })?;
            out.push_str(idents.get(&name)?);
        } else if part.as_interpolated_string_node().is_some() {
            out.push_str(&interpolate_string_node(&part, idents)?);
        } else {
            return None;
        }
    }
    Some(out)
}

/// After `#{param}` has been substituted, remaining local reads of a
/// bound param become symbol literals (`name` → `:body`). A statement
/// sliced out of its `def` parses those names as receiverless calls,
/// which must get the same rewrite. A `LocalVariableReadNode` of a
/// bound name is slice-local (block/lambda shadow) and declines.
/// Offset surgery stays in this module: the orchestrator never sees
/// the source rewrite.
fn bind_local_reads(
    src: &str,
    idents: &HashMap<String, String>,
    param_names: &HashSet<String>,
) -> Option<String> {
    let parsed = crate::ingest::prism::parse_silent(src.as_bytes());
    if parsed.errors().next().is_some() {
        return None;
    }
    struct Locals {
        hits: Vec<(usize, usize, String)>,
        idents: HashMap<String, String>,
        param_names: HashSet<String>,
        decline: bool,
    }
    impl Locals {
        fn record(&mut self, name: String, loc: ruby_prism::Location<'_>) {
            if self.idents.contains_key(&name) {
                self.hits.push((loc.start_offset(), loc.end_offset(), name));
            } else if self.param_names.contains(&name) {
                self.decline = true;
            }
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Locals {
        fn visit_local_variable_read_node(
            &mut self,
            node: &ruby_prism::LocalVariableReadNode<'pr>,
        ) {
            // Standalone-slice local reads are bound inside the slice
            // (block/lambda param) and shadow the macro parameter.
            let name = String::from_utf8_lossy(node.name().as_slice()).into_owned();
            if self.idents.contains_key(&name) {
                self.decline = true;
            }
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && node.arguments().is_none() && node.block().is_none() {
                self.record(
                    String::from_utf8_lossy(node.name().as_slice()).into_owned(),
                    node.location(),
                );
            }
            if let Some(recv) = node.receiver() {
                self.visit(&recv);
            }
            if let Some(args) = node.arguments() {
                for arg in args.arguments().iter() {
                    self.visit(&arg);
                }
            }
            if let Some(block) = node.block() {
                self.visit(&block);
            }
        }
    }
    let mut locals = Locals {
        hits: Vec::new(),
        idents: idents.clone(),
        param_names: param_names.clone(),
        decline: false,
    };
    ruby_prism::Visit::visit(&mut locals, &parsed.node());
    if locals.decline {
        return None;
    }
    let mut out = src.to_string();
    locals.hits.sort_by_key(|(start, _, _)| *start);
    locals.hits.reverse();
    for (start, end, name) in locals.hits {
        let ident = idents.get(&name)?;
        if start > out.len() || end > out.len() || start > end {
            return None;
        }
        out.replace_range(start..end, &format!(":{ident}"));
    }
    Some(out)
}
