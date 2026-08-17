//! ActiveRecord model ingestion — parses one `app/models/*.rb` into
//! a `Model`, including validations, associations, callbacks, scopes,
//! and methods.

use indexmap::IndexMap;
use ruby_prism::Node;

use crate::dialect::{Comment, Model, ModelBodyItem};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::naming::{camelize, singularize_camelize, snake_case};
use crate::schema::{ColumnType, Schema, Table};
use crate::span::Span;
use crate::ty::{Row, Ty};
use crate::{ClassId, Symbol, TableRef};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, collect_comments, constant_id_str, constant_path_of, drain_comments_before,
    find_first_class, flatten_statements, source_has_blank_line, string_value, symbol_value,
};
use super::{IngestError, IngestResult};

/// Namespace module → the `table_name_prefix` it declares. Rails walks a
/// model's `module_parents` looking for this and prepends what it finds,
/// which is the ONLY thing that puts a namespace into a table name (the
/// name itself is demodulized — see `naming::rails_table_name`).
///
/// campfire's `app/models/push.rb` is four lines and exists solely for
/// this: without it `Push::Subscription` reads `subscriptions`, and its
/// table is `push_subscriptions`.
pub type TablePrefixes = std::collections::HashMap<String, String>;

/// Scan one file for `module <Ns>; def self.table_name_prefix; "<p>"; end`.
/// Deliberately narrow: only a module-level `self.` def whose body is a
/// single string literal. A computed prefix would have to run to be known,
/// and nothing in the corpus writes one.
pub fn ingest_table_name_prefixes(source: &[u8], file: &str) -> TablePrefixes {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let mut out = TablePrefixes::new();
    for (scope, module) in super::util::find_all_modules_with_scope(&root) {
        let Some(name_path) = super::util::module_name_path(&module) else {
            continue;
        };
        let mut full = scope;
        full.extend(name_path);
        let Some(body) = module.body() else { continue };
        for stmt in flatten_statements(body) {
            let Some(def) = stmt.as_def_node() else { continue };
            if def.receiver().and_then(|r| r.as_self_node()).is_none() {
                continue;
            }
            if constant_id_str(&def.name()) != "table_name_prefix" {
                continue;
            }
            let Some(def_body) = def.body() else { continue };
            let stmts = flatten_statements(def_body);
            if stmts.len() != 1 {
                continue;
            }
            if let Some(prefix) = string_value(&stmts[0]) {
                out.insert(full.join("::"), prefix);
            }
        }
    }
    out
}

/// Parse a single model file. The first class definition is treated as the
/// model; any schema-derived attributes are filled in from `schema`.
pub fn ingest_model(
    source: &[u8],
    file: &str,
    schema: &Schema,
    prefixes: &TablePrefixes,
) -> IngestResult<Option<Model>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    // Scope-aware: a model declared as `module Admin; class Report`
    // must ingest as `Admin::Report` (the compound `class Admin::Report`
    // spelling already carries its path). Falls back to the scopeless
    // finder for shapes the scoped walk doesn't cover.
    let (scope, class) = match super::util::find_all_classes_with_scope(&root).into_iter().next()
    {
        Some((s, c)) => (s, Some(c)),
        None => (Vec::new(), find_first_class(&root)),
    };
    let Some(class) = class else {
        return Ok(None);
    };

    let mut name_path = scope;
    name_path.extend(class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "model class name must be a simple constant or path".into(),
    })?);
    let class_name = Symbol::from(name_path.join("::"));
    let owner = ClassId(class_name.clone());
    // Rails: `full_table_name_prefix + undecorated_table_name`. The
    // prefix comes from the nearest module parent that declares one,
    // searched innermost-out the way `module_parents` walks.
    let table_name = {
        let mut segments: Vec<&str> = class_name.as_str().split("::").collect();
        segments.pop();
        let mut prefix = String::new();
        while !segments.is_empty() {
            if let Some(p) = prefixes.get(&segments.join("::")) {
                prefix = p.clone();
                break;
            }
            segments.pop();
        }
        format!("{prefix}{}", crate::naming::rails_table_name(class_name.as_str()))
    };

    let attributes = if let Some(table) = schema.tables.get(&Symbol::from(table_name.as_str())) {
        row_from_table(table)
    } else {
        Row::closed()
    };

    let mut comments = collect_comments(&result);
    // Discard comments that precede the `class` keyword — file-level
    // magic pragmas, doc blocks. We'll attach those to `Model` itself
    // when a fixture forces it. Comments inside the class body (after
    // `class Foo` but before its first statement) are preserved and
    // naturally attach to the first body item below.
    drain_comments_before(&mut comments, class.location().start_offset());
    let mut body: Vec<ModelBodyItem> = Vec::new();
    let mut enums: IndexMap<Symbol, Vec<(String, Literal)>> = IndexMap::new();
    let mut primary_key: Option<Symbol> = None;
    if let Some(class_body) = class.body() {
        let mut prev_end: Option<usize> = None;
        for stmt in flatten_statements(class_body) {
            // `self.primary_key = "key"` is recognized into
            // `Model::primary_key` instead of being kept as a body item:
            // the lowering synthesizes a reader from it, and re-emitting
            // the assignment verbatim would call a writer no target's
            // runtime defines. Comments above it fall through to the
            // next statement.
            if let Some(pk) = parse_primary_key_decl(&stmt) {
                primary_key = Some(pk);
                prev_end = Some(stmt.location().end_offset());
                continue;
            }
            let stmt_start = stmt.location().start_offset();
            let leading_area_start =
                comments.first().map(|(off, _)| *off).filter(|off| *off < stmt_start)
                    .unwrap_or(stmt_start);
            let leading = drain_comments_before(&mut comments, stmt_start);
            let leading_blank = prev_end
                .map(|pe| source_has_blank_line(source, pe, leading_area_start))
                .unwrap_or(false);
            // `enum :status, %i[…]` — one statement standing for a
            // scope + predicate + bang writer per label, so it expands
            // in the walk loop for the same reason `class << self` does.
            if let Some(call) = stmt.as_call_node() {
                match expand_enum_decl(&call, file, &leading) {
                    Ok(Some(expanded)) => {
                        enums.insert(expanded.column, expanded.mapping);
                        let mut blank = leading_blank;
                        for mut item in expanded.items {
                            item.set_leading_blank_line(std::mem::take(&mut blank));
                            body.push(item);
                        }
                        prev_end = Some(stmt.location().end_offset());
                        continue;
                    }
                    Ok(None) => {}
                    Err(err) if super::survey::is_active() => {
                        super::survey::record(&err);
                        prev_end = Some(stmt.location().end_offset());
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            }
            // `class << self … end` — the singleton block's defs are
            // class methods of the model (`Room.create_for`). A model's
            // IR body is one item per statement, so expand the block in
            // place; `ingest_model_body_item` returns a single item and
            // can't. Library classes get the same treatment one level
            // down, in `walk_decl_body`.
            if let Some(sc) = stmt.as_singleton_class_node() {
                match ingest_singleton_class_methods(&sc, file) {
                    Ok(methods) => {
                        let mut leading = leading;
                        let mut blank = leading_blank;
                        for method in methods {
                            let mut item = ModelBodyItem::Method {
                                method,
                                leading_comments: std::mem::take(&mut leading),
                                leading_blank_line: false,
                            };
                            item.set_leading_blank_line(std::mem::take(&mut blank));
                            body.push(item);
                        }
                        prev_end = Some(stmt.location().end_offset());
                        continue;
                    }
                    Err(err) if super::survey::is_active() => {
                        super::survey::record(&err);
                        prev_end = Some(stmt.location().end_offset());
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            }
            // Survey mode: an unsupported *item* (an exotic scope form, a
            // DSL shape the classifier rejects) costs itself, not the
            // whole class — record the gap and keep walking, mirroring
            // the expr-level recovery inside `ingest_expr`. Before this
            // gate, one such item silently dropped the entire model
            // (Mastodon lost `Status` to a single spelled-out scope
            // lambda). Strict mode still aborts.
            let mut item = match ingest_model_body_item(&stmt, &owner, file, leading) {
                Ok(item) => item,
                Err(err) if super::survey::is_active() => {
                    super::survey::record(&err);
                    prev_end = Some(stmt.location().end_offset());
                    continue;
                }
                Err(err) => return Err(err),
            };
            item.set_leading_blank_line(leading_blank);
            body.push(item);
            prev_end = Some(stmt.location().end_offset());
        }
    }

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    let class_loc = class.location();
    Ok(Some(Model {
        name: owner,
        parent,
        table: TableRef(Symbol::from(table_name)),
        primary_key,
        attributes,
        body,
        enums,
        span: Span {
            file: super::sources::file_id(file),
            start: class_loc.start_offset() as u32,
            end: class_loc.end_offset() as u32,
        },
    }))
}

/// Classify one class-body statement into its `ModelBodyItem` variant.
/// `leading_comments` is attached regardless of variant so every item
/// keeps its inline docs.
pub(super) fn ingest_model_body_item(
    stmt: &Node<'_>,
    owner: &ClassId,
    file: &str,
    leading_comments: Vec<Comment>,
) -> IngestResult<ModelBodyItem> {
    // Span of the whole statement — the typed variants (Association /
    // Validation / Callback) drop the source Expr during recognition,
    // so the declaration's location rides the ModelBodyItem wrapper.
    let span = Span {
        file: super::sources::file_id(file),
        start: stmt.location().start_offset() as u32,
        end: stmt.location().end_offset() as u32,
    };
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_some() {
            return Ok(ModelBodyItem::Unknown {
                expr: ingest_expr(stmt, file)?,
                leading_comments,
                leading_blank_line: false,
            });
        }
        let method = constant_id_str(&call.name()).to_string();
        if let Some(assoc) = parse_association(&call, owner, &method, file) {
            return Ok(ModelBodyItem::Association { assoc, leading_blank_line: false, leading_comments, span });
        }
        if method == "validates" {
            let mut parsed = parse_validates(&call);
            if let Some(first) = parsed.first().cloned() {
                // `validates :attr` with multiple rules is one call; we
                // only see one Validation per call today. If the call
                // expanded to multiple (the multi-attribute form), they
                // share leading comments only on the first.
                let mut items = Vec::with_capacity(parsed.len());
                items.push(ModelBodyItem::Validation {
                    validation: first,
                    leading_comments,
                    leading_blank_line: false,
                    span,
                });
                for v in parsed.drain(1..) {
                    items.push(ModelBodyItem::Validation {
                        validation: v,
                        leading_comments: Vec::new(),
                        leading_blank_line: false,
                        span,
                    });
                }
                // Degenerate: the caller expects ONE item. If parse_validates
                // returned multiple, merge-ingest is a bit lossy — return
                // the first and drop the tail (no real fixture triggers
                // this yet; multi-attr validates is usually
                // `validates :a, :b, rule: ...` and our current shape is
                // one-Validation-per-attribute).
                return Ok(items.into_iter().next().unwrap());
            }
            // No validation extracted — treat as Unknown so we don't lose it.
            return Ok(ModelBodyItem::Unknown {
                expr: ingest_expr(stmt, file)?,
                leading_comments,
                leading_blank_line: false,
            });
        }
        if method == "scope" {
            if let Some(scope) = parse_scope(&call, file)? {
                return Ok(ModelBodyItem::Scope { scope, leading_blank_line: false, leading_comments });
            }
        }
        if let Some(callback) = parse_callback(&call, &method) {
            return Ok(ModelBodyItem::Callback { callback, leading_blank_line: false, leading_comments, span });
        }
        return Ok(ModelBodyItem::Unknown {
            expr: ingest_expr(stmt, file)?,
            leading_comments,
            leading_blank_line: false,
        });
    }
    if let Some(def) = stmt.as_def_node() {
        return Ok(ModelBodyItem::Method {
            method: ingest_method(&def, file)?,
            leading_comments,
            leading_blank_line: false,
        });
    }
    Ok(ModelBodyItem::Unknown {
        expr: ingest_expr(stmt, file)?,
        leading_comments,
        leading_blank_line: false,
    })
}

/// Expand `enum :status, %i[active deactivated banned]` into the DSL it
/// stands for: one scope, one predicate and one bang writer per label.
///
/// Returns `None` when the statement isn't an `enum` call, so callers
/// can fall through to the normal classifier.
///
/// Desugaring into `Scope` + `Method` items — rather than adding a
/// `ModelBodyItem::Enum` and teaching thirteen emitters to expand it —
/// buys the whole existing pipeline for free: scopes already lower to
/// relation-returning class methods (with `__scope_` delegates), and
/// predicate bodies are ordinary column comparisons.
///
/// **Stored values, not labels.** The generated bodies compare and
/// query against what the column holds (`status == 0`), which is what
/// makes them correct without an enum type at runtime. The divergence
/// from Rails is the attribute reader: `user.status` yields `0` here
/// and `"active"` there. Rails' own `enum` maps at every boundary; that
/// mapping (for hand-written `where(role: :bot)` and `update!(status:
/// :deactivated)` sites) is a separate, type-aware pass.
pub(super) struct EnumExpansion {
    pub column: Symbol,
    /// Label → stored value, in declaration order.
    pub mapping: Vec<(String, Literal)>,
    pub items: Vec<ModelBodyItem>,
}

pub(super) fn expand_enum_decl(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    leading_comments: &[crate::dialect::Comment],
) -> IngestResult<Option<EnumExpansion>> {
    use crate::dialect::{MethodDef, MethodReceiver, Scope};
    use crate::effect::EffectSet;

    if call.receiver().is_some() || constant_id_str(&call.name()) != "enum" {
        return Ok(None);
    }
    let Some(args) = call.arguments() else { return Ok(None) };
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let Some(first) = iter.next() else { return Ok(None) };

    // Two spellings: `enum :status, <mapping>, **opts` (Rails 7) and the
    // older `enum status: <mapping>, **opts`, where the column and its
    // mapping are the first pair of one keyword hash.
    let (column, mapping_node, prefix, suffix) = match symbol_value(&first) {
        Some(col) => {
            let column: String = col;
            let mapping = iter.next();
            let opts = iter.next();
            let (prefix, suffix) = match opts.as_ref().and_then(|o| o.as_keyword_hash_node()) {
                Some(kh) => enum_affixes(&kh.elements(), &column),
                None => (String::new(), String::new()),
            };
            (column, mapping, prefix, suffix)
        }
        None => {
            let Some(kh) = first.as_keyword_hash_node() else { return Ok(None) };
            let elements = kh.elements();
            let Some(pair) = elements.iter().next().and_then(|e| e.as_assoc_node()) else {
                return Ok(None);
            };
            let Some(column) = symbol_value(&pair.key()) else { return Ok(None) };
            let (prefix, suffix) = enum_affixes(&elements, &column);
            (column, Some(pair.value()), prefix, suffix)
        }
    };
    let Some(mapping_node) = mapping_node else { return Ok(None) };

    let labels = enum_label_values(&mapping_node).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "enum :{} mapping must be an array or hash literal (or `%w[…].index_by(&:itself)`)",
            column
        ),
    })?;

    let span = Span::synthetic();
    let sym = |s: &str| Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } });
    let column_read = || {
        Expr::new(
            span,
            ExprNode::Send {
                recv: None,
                method: Symbol::from(column.as_str()),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };
    let mut items = Vec::new();
    for (label, value) in labels.iter().cloned() {
        let base = format!("{prefix}{label}{suffix}");
        let pair = Expr::new(
            span,
            ExprNode::Hash {
                entries: vec![(sym(&column), Expr::new(span, ExprNode::Lit { value: value.clone() }))],
                kwargs: true,
            },
        );
        let call_with_pair = |method: &str| {
            Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from(method),
                    args: vec![pair.clone()],
                    block: None,
                    parenthesized: true,
                },
            )
        };
        let method_def = |name: String, body: Expr| ModelBodyItem::Method {
            method: MethodDef {
                name: Symbol::from(name),
                receiver: MethodReceiver::Instance,
                params: Vec::new(),
                block_param: None,
                body,
                signature: None,
                effects: EffectSet::pure(),
                enclosing_class: None,
                kind: crate::dialect::AccessorKind::Method,
                is_async: false,
                mutates_self: false,
            },
            leading_comments: Vec::new(),
            leading_blank_line: false,
        };

        items.push(ModelBodyItem::Scope {
            scope: Scope {
                name: Symbol::from(base.as_str()),
                params: Vec::new(),
                body: call_with_pair("where"),
            },
            // The declaration's own comments ride the first item it
            // expands to, so a documented `enum` keeps its docs.
            leading_comments: if items.is_empty() {
                leading_comments.to_vec()
            } else {
                Vec::new()
            },
            leading_blank_line: false,
        });
        items.push(method_def(
            format!("{base}?"),
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(column_read()),
                    method: Symbol::from("=="),
                    args: vec![Expr::new(span, ExprNode::Lit { value })],
                    block: None,
                    parenthesized: false,
                },
            ),
        ));
        items.push(method_def(format!("{base}!"), call_with_pair("update!")));
    }
    Ok(Some(EnumExpansion { column: Symbol::from(column.as_str()), mapping: labels, items }))
}

/// Label → stored value for an `enum` mapping. An array literal maps by
/// index the way Rails does (`%i[active deactivated]` → 0, 1); a hash
/// literal carries its own values; `%w[…].index_by(&:itself)` — the
/// idiom for a string-backed column — maps each label to itself.
/// `None` for anything else (a constant reference, a computed hash),
/// which the caller reports as a gap rather than guessing at storage.
fn enum_label_values(node: &Node<'_>) -> Option<Vec<(String, Literal)>> {
    if let Some(arr) = node.as_array_node() {
        return arr
            .elements()
            .iter()
            .enumerate()
            .map(|(i, el)| {
                symbol_value(&el)
                    .or_else(|| string_value(&el))
                    .map(|label| (label, Literal::Int { value: i as i64 }))
            })
            .collect();
    }
    if let Some(hash) = node.as_hash_node() {
        return hash
            .elements()
            .iter()
            .map(|el| {
                let assoc = el.as_assoc_node()?;
                let label = symbol_value(&assoc.key()).or_else(|| string_value(&assoc.key()))?;
                let value = assoc.value();
                let lit = if let Some(s) = string_value(&value) {
                    Literal::Str { value: s }
                } else {
                    let raw = value.as_integer_node()?;
                    let v: i32 = raw.value().try_into().ok()?;
                    Literal::Int { value: v as i64 }
                };
                Some((label, lit))
            })
            .collect();
    }
    // `%w[ invisible nothing mentions ].index_by(&:itself)` — the labels
    // ARE the stored strings.
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) != "index_by" {
        return None;
    }
    let arr = call.receiver()?;
    let arr = arr.as_array_node()?;
    arr.elements()
        .iter()
        .map(|el| {
            let label = symbol_value(&el).or_else(|| string_value(&el))?;
            Some((label.clone(), Literal::Str { value: label }))
        })
        .collect()
}

/// `prefix:`/`suffix:` from an `enum`'s option hash. `true` means "use
/// the column name" (Rails' own convention); a symbol or string names
/// the affix directly. Returns the strings to splice around each label,
/// already carrying their separating underscore.
fn enum_affixes(elements: &ruby_prism::NodeList<'_>, column: &str) -> (String, String) {
    let mut prefix = String::new();
    let mut suffix = String::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else { continue };
        let Some(key) = symbol_value(&assoc.key()) else { continue };
        // `_prefix`/`_suffix` are the pre-Rails-7 spellings.
        let which = key.trim_start_matches('_');
        if which != "prefix" && which != "suffix" {
            continue;
        }
        let value = assoc.value();
        let affix = if value.as_true_node().is_some() {
            column.to_string()
        } else if let Some(s) = symbol_value(&value).or_else(|| string_value(&value)) {
            s
        } else {
            continue;
        };
        if which == "prefix" {
            prefix = format!("{affix}_");
        } else {
            suffix = format!("_{affix}");
        }
    }
    (prefix, suffix)
}

/// Expand a model's `class << self … end` into the class methods it
/// declares. Only `def`s are recognized: a visibility marker or an
/// `attr_accessor` in there means something about the *singleton*
/// scope that a flattened list of methods can't carry, so refuse it
/// loudly rather than silently apply it to the instance side.
fn ingest_singleton_class_methods(
    sc: &ruby_prism::SingletonClassNode<'_>,
    file: &str,
) -> IngestResult<Vec<crate::dialect::MethodDef>> {
    use crate::dialect::MethodReceiver;

    let Some(body) = sc.body() else { return Ok(Vec::new()) };
    let mut methods = Vec::new();
    for stmt in super::util::flatten_statements(body) {
        let Some(def) = stmt.as_def_node() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported statement inside `class << self`: {stmt:?}"),
            });
        };
        let mut method = ingest_method(&def, file)?;
        method.receiver = MethodReceiver::Class;
        methods.push(method);
    }
    Ok(methods)
}

pub(super) fn ingest_method(
    def: &ruby_prism::DefNode<'_>,
    file: &str,
) -> IngestResult<crate::dialect::MethodDef> {
    use crate::dialect::{MethodDef, MethodReceiver};

    let name = Symbol::from(constant_id_str(&def.name()));
    // `def self.foo` / `def Post.foo` have explicit receivers; plain `def foo`
    // is an instance method.
    let receiver = if def.receiver().is_some() {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };

    // Collect required positional params, then optional-with-default
    // params (`def avatar_path(size = 100)`) carrying their default expr so
    // the emitted method reproduces the arity — dropping the optional left
    // `def avatar_path` with a body still reading `size`, an ArgumentError
    // at every call site that passes one. Keyword/rest/block params are
    // rarer on model methods and still fall through unrecorded.
    let mut params: Vec<crate::dialect::Param> = Vec::new();
    if let Some(pn) = def.parameters() {
        for req in pn.requireds().iter() {
            if let Some(rp) = req.as_required_parameter_node() {
                params.push(crate::dialect::Param::positional(Symbol::from(
                    constant_id_str(&rp.name()),
                )));
            }
        }
        for opt in pn.optionals().iter() {
            if let Some(op) = opt.as_optional_parameter_node() {
                let default = ingest_expr(&op.value(), file)?;
                params.push(crate::dialect::Param::with_default(
                    Symbol::from(constant_id_str(&op.name())),
                    default,
                ));
            }
        }
        // Keyword params (`def recent_threads(amount, for_user: nil)`),
        // required (`k:`) and optional (`k: default`) alike. Dropping
        // them left `def recent_threads(amount)` with a body reading
        // `for_user` — an ArgumentError at every kwarg call site.
        // Rest/block params still fall through unrecorded.
        for kw in pn.keywords().iter() {
            if let Some(okw) = kw.as_optional_keyword_parameter_node() {
                let default = ingest_expr(&okw.value(), file)?;
                params.push(crate::dialect::Param::keyword(
                    Symbol::from(constant_id_str(&okw.name())),
                    Some(default),
                ));
            } else if let Some(rkw) = kw.as_required_keyword_parameter_node() {
                params.push(crate::dialect::Param::keyword(
                    Symbol::from(constant_id_str(&rkw.name())),
                    None,
                ));
            }
        }
    }

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name,
        receiver,
        params,
        body,
        signature: None,
        effects: EffectSet::pure(),
        // Rails model methods carry their owner on the surrounding
        // Model struct (model.name); no need to duplicate here.
        enclosing_class: None,
        // Source-defined `def` in a Rails model — Method by default.
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    })
}

fn parse_callback(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
) -> Option<crate::dialect::Callback> {
    use crate::dialect::{Callback, CallbackHook, CallbackOn};

    let hook = match method {
        "before_validation" => CallbackHook::BeforeValidation,
        "after_validation" => CallbackHook::AfterValidation,
        "before_save" => CallbackHook::BeforeSave,
        "after_save" => CallbackHook::AfterSave,
        "before_create" => CallbackHook::BeforeCreate,
        "after_create" => CallbackHook::AfterCreate,
        "before_update" => CallbackHook::BeforeUpdate,
        "after_update" => CallbackHook::AfterUpdate,
        "before_destroy" => CallbackHook::BeforeDestroy,
        "after_destroy" => CallbackHook::AfterDestroy,
        "after_commit" => CallbackHook::AfterCommit,
        "after_rollback" => CallbackHook::AfterRollback,
        _ => return None,
    };

    let args = call.arguments()?;
    let mut targets: Vec<Symbol> = Vec::new();
    let mut on: Option<CallbackOn> = None;
    for arg in args.arguments().iter() {
        if let Some(sym) = symbol_value(&arg) {
            targets.push(Symbol::from(sym.as_str()));
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let assoc = el.as_assoc_node()?;
                let key = symbol_value(&assoc.key())?;
                match key.as_str() {
                    "on" => {
                        on = Some(match symbol_value(&assoc.value())?.as_str() {
                            "create" => CallbackOn::Create,
                            "update" => CallbackOn::Update,
                            "destroy" => CallbackOn::Destroy,
                            // `on: [:create, :update]` array form and
                            // unknown values: not modeled — reject.
                            _ => return None,
                        });
                    }
                    // `if:` / `unless:` / `prepend:` — lowering the
                    // callback while dropping these would run it in the
                    // wrong circumstances, which is worse than dropping
                    // the declaration with a warning. Reject: the item
                    // falls through to Unknown and the ledger reports it.
                    _ => return None,
                }
            }
        } else {
            // Lambda / block-pass target (`after_create -> { … }`):
            // stays an Unknown item (block-form bodies are handled by
            // `push_callback_methods`; lambda args are still a gap).
            return None;
        }
    }
    if targets.is_empty() {
        return None;
    }
    // `on:` is only expressible where the runtime hook surface can
    // carry it — validation hooks (new_record? guard) and after_commit
    // (mapped onto the per-lifecycle `after_*_commit` hooks). Rails
    // doesn't accept `on:` for the save/create/update/destroy hooks,
    // so anything else here is either source that doesn't run in
    // Rails or a shape we can't lower faithfully.
    if on.is_some()
        && !matches!(
            hook,
            CallbackHook::BeforeValidation
                | CallbackHook::AfterValidation
                | CallbackHook::AfterCommit
        )
    {
        return None;
    }

    Some(Callback { hook, targets, on, condition: None })
}

fn parse_scope(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<Option<crate::dialect::Scope>> {
    use crate::dialect::Scope;

    let Some(args) = call.arguments() else { return Ok(None) };
    let all_args = args.arguments();
    let mut iter = all_args.iter();

    let Some(name_node) = iter.next() else { return Ok(None) };
    let Some(name_str) = symbol_value(&name_node) else { return Ok(None) };
    let name = Symbol::from(name_str.as_str());

    let Some(body_node) = iter.next() else { return Ok(None) };
    // A scope body is a lambda in one of two spellings: the arrow form
    // `->(x) { ... }` (a LambdaNode) or the spelled-out `lambda { |x| … }`
    // / `proc { |x| … }` (a receiverless CallNode whose block carries the
    // same parameters + body — Mastodon's multi-line scopes use this).
    let (param_node, lambda_body) = if let Some(lambda) = body_node.as_lambda_node() {
        (lambda.parameters(), lambda.body())
    } else if let Some((params, body)) = spelled_lambda_parts(&body_node) {
        (params, body)
    } else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "scope :{name} body must be a lambda (`-> {{ ... }}` or `lambda {{ ... }}`)"
            ),
        });
    };

    // Lambda parameters: required (`->(user)`), optional-with-default
    // (`->(user = nil)`), and keywords (`->(user, unmerged: true)` —
    // lobsters' base/recent scopes). Defaults are carried so the
    // lowered class method reproduces the signature; the lowerer
    // inserts the trailing relation parameter before any keywords.
    // Block/splat scope params still fall through unrecorded.
    let mut params: Vec<crate::dialect::Param> = Vec::new();
    if let Some(pn) = param_node
        .and_then(|p| p.as_block_parameters_node().and_then(|bpn| bpn.parameters()))
    {
        for req in pn.requireds().iter() {
            if let Some(rp) = req.as_required_parameter_node() {
                params.push(crate::dialect::Param::positional(Symbol::from(
                    constant_id_str(&rp.name()),
                )));
            }
        }
        for opt in pn.optionals().iter() {
            if let Some(op) = opt.as_optional_parameter_node() {
                let default = ingest_expr(&op.value(), file)?;
                params.push(crate::dialect::Param::with_default(
                    Symbol::from(constant_id_str(&op.name())),
                    default,
                ));
            }
        }
        for kw in pn.keywords().iter() {
            if let Some(okw) = kw.as_optional_keyword_parameter_node() {
                let default = ingest_expr(&okw.value(), file)?;
                params.push(crate::dialect::Param::keyword(
                    Symbol::from(constant_id_str(&okw.name())),
                    Some(default),
                ));
            } else if let Some(rkw) = kw.as_required_keyword_parameter_node() {
                params.push(crate::dialect::Param::keyword(
                    Symbol::from(constant_id_str(&rkw.name())),
                    None,
                ));
            }
        }
    }

    let body = match lambda_body {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(Some(Scope { name, params, body }))
}

/// `lambda { |x| … }` / `proc { |x| … }` — a receiverless call whose
/// braces-block carries the parameters and body. Returns the same
/// `(parameters, body)` pair a `LambdaNode` exposes so `parse_scope`
/// treats both spellings identically. `None` for anything else
/// (including block-pass `lambda(&blk)`, which has no BlockNode).
fn spelled_lambda_parts<'a>(
    node: &Node<'a>,
) -> Option<(Option<Node<'a>>, Option<Node<'a>>)> {
    let call = node.as_call_node()?;
    if call.receiver().is_some() {
        return None;
    }
    if !matches!(constant_id_str(&call.name()), "lambda" | "proc") {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    Some((block.parameters(), block.body()))
}

fn parse_validates(call: &ruby_prism::CallNode<'_>) -> Vec<crate::dialect::Validation> {
    use crate::dialect::{Validation, ValidationRule};
    let Some(args) = call.arguments() else { return vec![] };
    let all_args = args.arguments();

    let mut attrs: Vec<Symbol> = Vec::new();
    let mut rules: Vec<ValidationRule> = Vec::new();
    let mut allow_blank = false;

    for arg in all_args.iter() {
        if let Some(sym) = symbol_value(&arg) {
            attrs.push(Symbol::from(sym.as_str()));
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = assoc.value();
                if key.as_str() == "allow_blank" {
                    allow_blank = super::util::bool_value(&value).unwrap_or(false);
                } else if let Some(rule) = validation_rule_from_kv(&key, &value) {
                    rules.push(rule);
                }
            }
        }
    }

    // `presence: true, allow_blank: true` is a dead check: presence
    // fails only when the value is blank, and allow_blank skips every
    // validator on blank values — so it can never fire (Rails runs
    // validations BEFORE before_save, so e.g. lobsters' `validates
    // :session_token, allow_blank: true, presence: true` relies on
    // this: the token is generated in a before_save). Dropping it here
    // fixes both lowering paths at once. The remaining parsed rule
    // kinds are blank-safe as emitted (MaxLength trivially passes on
    // blank; Absence wants blank); when MinLength/Format/Uniqueness
    // gain allow_blank fixtures they'll need a skip-when-blank guard
    // on the check instead of a drop.
    if allow_blank {
        rules.retain(|r| !matches!(r, ValidationRule::Presence));
    }

    let mut out = Vec::new();
    for attr in attrs {
        out.push(Validation { attribute: attr, rules: rules.clone() });
    }
    out
}

fn validation_rule_from_kv(
    key: &str,
    value: &ruby_prism::Node<'_>,
) -> Option<crate::dialect::ValidationRule> {
    use super::util::bool_value;
    use crate::dialect::ValidationRule;
    match key {
        "presence" => bool_value(value).filter(|b| *b).map(|_| ValidationRule::Presence),
        "absence" => bool_value(value).filter(|b| *b).map(|_| ValidationRule::Absence),
        "length" => parse_length_rule(value),
        _ => None,
    }
}

/// `length: { minimum: N, maximum: M }`. Either bound may be absent;
/// the hash-value shape is the only one we accept today. The shorthand
/// `length: 5` (exact length) isn't in any fixture yet and drops.
fn parse_length_rule(value: &ruby_prism::Node<'_>) -> Option<crate::dialect::ValidationRule> {
    use super::util::integer_value;
    use crate::dialect::ValidationRule;
    let hash = value.as_hash_node().or_else(|| {
        // Rails idiomatically uses `{ ... }`, but a bare keyword-args
        // shape (`length: { … }` parses as HashNode inside the kwargs,
        // not KeywordHashNode). Keep KeywordHashNode as a fallback for
        // defensive parsing.
        None
    });
    let elements = if let Some(h) = hash {
        h.elements()
    } else if let Some(kh) = value.as_keyword_hash_node() {
        kh.elements()
    } else {
        return None;
    };

    let mut min: Option<u32> = None;
    let mut max: Option<u32> = None;
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else { continue };
        let Some(key) = symbol_value(&assoc.key()) else { continue };
        let Some(n) = integer_value(&assoc.value()) else { continue };
        if n < 0 {
            continue;
        }
        match key.as_str() {
            "minimum" => min = Some(n as u32),
            "maximum" => max = Some(n as u32),
            // `is:` (exact), `in:` (range), `within:` land when a
            // fixture demands them.
            _ => {}
        }
    }

    if min.is_none() && max.is_none() {
        None
    } else {
        Some(ValidationRule::Length { min, max, message: None })
    }
}

fn parse_association(
    call: &ruby_prism::CallNode<'_>,
    owner: &ClassId,
    method: &str,
    file: &str,
) -> Option<crate::dialect::Association> {
    use super::util::{bool_value, string_value};
    use crate::dialect::{Association, Dependent};

    let args = call.arguments()?;
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;
    let name_str = symbol_value(&first)?;
    let name = Symbol::from(name_str.as_str());

    let mut class_name: Option<String> = None;
    let mut foreign_key: Option<String> = None;
    let mut through: Option<String> = None;
    let mut source: Option<String> = None;
    let mut source_type: Option<String> = None;
    let mut dependent: Option<Dependent> = None;
    let mut optional: Option<bool> = None;
    let mut join_table: Option<String> = None;
    let mut scope: Option<crate::expr::Expr> = None;
    let mut polymorphic: Option<bool> = None;
    let mut as_interface: Option<String> = None;

    for arg in iter {
        // Positional lambda between name and kwargs — the association
        // scope (`has_many :x, -> { where(...) }, through: :y`).
        // Recorded as its raw body Expr; the reader synthesis grafts it
        // onto the relation seed. Param-taking lambdas (rare
        // owner-dependent scopes) are skipped — they need the owner
        // threaded and no exercised corpus does this yet.
        if let Some(lambda) = arg.as_lambda_node() {
            if scope.is_none() {
                let param_free = lambda
                    .parameters()
                    .and_then(|p| p.as_block_parameters_node().and_then(|b| b.parameters()))
                    .map(|pn| pn.requireds().iter().next().is_none())
                    .unwrap_or(true);
                if param_free {
                    scope = lambda.body().and_then(|b| ingest_expr(&b, file).ok());
                }
            }
            continue;
        }
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "class_name" => class_name = string_value(&value),
                "foreign_key" => {
                    foreign_key = string_value(&value).or_else(|| symbol_value(&value))
                }
                "through" => through = symbol_value(&value),
                "source" => source = symbol_value(&value),
                "source_type" => source_type = string_value(&value),
                "dependent" => {
                    dependent = symbol_value(&value).and_then(|s| dependent_from_sym(&s))
                }
                "optional" => optional = bool_value(&value),
                "join_table" => join_table = string_value(&value),
                "polymorphic" => polymorphic = bool_value(&value),
                "as" => as_interface = symbol_value(&value),
                _ => {}
            }
        }
    }

    let owner_snake = snake_case(owner.0.as_str());

    // Association-extension block: `has_many :memberships do def
    // grant_to(users) … end end`. Only `def`s are collected — a block
    // body that does anything else is not an extension module and is
    // left where it is rather than half-read.
    let extension: Vec<crate::dialect::MethodDef> = call
        .block()
        .and_then(|b| b.as_block_node())
        .and_then(|b| b.body())
        .and_then(|b| b.as_statements_node())
        .map(|stmts| {
            stmts
                .body()
                .iter()
                .filter_map(|s| s.as_def_node())
                .filter_map(|d| ingest_method(&d, file).ok())
                .collect()
        })
        .unwrap_or_default();

    match method {
        "has_many" => Some(Association::HasMany {
            name: name.clone(),
            extension,
            // `source:` names the association on the `through:` model
            // that supplies the rows (`has_many :upvoted_stories,
            // through: :votes, source: :story` → Story, not the
            // assoc-name-derived "UpvotedStory" phantom). class_name
            // still wins when both are given, per Rails.
            //
            // `source_type:` disambiguates a *polymorphic* source
            // reflection, naming the concrete class directly
            // (`has_many :comment_references, through: :mod_mail_references,
            // source: :reference, source_type: "Comment"` → Comment). It
            // takes precedence over the camelized `source` name, which
            // would otherwise be the polymorphic association name
            // ("Reference") — a phantom class. Already CamelCase, so no
            // transform.
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .or_else(|| source_type.map(|s| ClassId(Symbol::from(s.as_str()))))
                .or_else(|| source.map(|s| ClassId(Symbol::from(camelize(s.as_str())))))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str())))),
            // `as: :notifiable` — the rows point back through the
            // interface columns, not an owner-named key.
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| match &as_interface {
                    Some(intf) => Symbol::from(format!("{intf}_id")),
                    None => Symbol::from(format!("{owner_snake}_id")),
                }),
            through: through.map(|s| Symbol::from(s.as_str())),
            dependent: dependent.unwrap_or_default(),
            as_interface: as_interface.as_deref().map(Symbol::from),
            scope,
        }),
        "has_one" => Some(Association::HasOne {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| match &as_interface {
                    Some(intf) => Symbol::from(format!("{intf}_id")),
                    None => Symbol::from(format!("{owner_snake}_id")),
                }),
            dependent: dependent.unwrap_or_default(),
            as_interface: as_interface.as_deref().map(Symbol::from),
        }),
        "belongs_to" => Some(Association::BelongsTo {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{name_str}_id"))),
            optional: optional.unwrap_or(false),
            polymorphic: polymorphic.unwrap_or(false),
            // Filled by `resolve_polymorphic_targets` once every
            // model's inverse `as:` declarations are ingested.
            polymorphic_targets: Vec::new(),
        }),
        "has_and_belongs_to_many" => Some(Association::HasAndBelongsToMany {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str())))),
            join_table: join_table
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(default_habtm_table(owner, name_str.as_str()))),
        }),
        _ => None,
    }
}

/// `self.primary_key = "key"` / `self.primary_key = :key` — Rails'
/// per-model override of the `id` default. Prism parses it as a call to
/// `primary_key=` on an explicit `self` receiver, which would otherwise
/// land in `ModelBodyItem::Unknown` and be dropped.
fn parse_primary_key_decl(stmt: &Node<'_>) -> Option<Symbol> {
    let call = stmt.as_call_node()?;
    call.receiver()?.as_self_node()?;
    if constant_id_str(&call.name()) != "primary_key=" {
        return None;
    }
    let args = call.arguments()?;
    let first = args.arguments().iter().next()?;
    let name = string_value(&first).or_else(|| symbol_value(&first))?;
    Some(Symbol::from(name.as_str()))
}

fn dependent_from_sym(s: &str) -> Option<crate::dialect::Dependent> {
    use crate::dialect::Dependent;
    Some(match s {
        "destroy" => Dependent::Destroy,
        "destroy_async" => Dependent::DestroyAsync,
        "delete" => Dependent::Delete,
        "delete_all" => Dependent::DeleteAll,
        "nullify" => Dependent::Nullify,
        "restrict_with_exception" | "restrict_with_error" => Dependent::Restrict,
        _ => return None,
    })
}

fn default_habtm_table(owner: &ClassId, target_plural_sym: &str) -> String {
    crate::naming::habtm_join_table(owner.0.as_str(), target_plural_sym)
}

pub(super) fn row_from_table(table: &Table) -> Row {
    let mut fields = IndexMap::new();
    for col in &table.columns {
        fields.insert(col.name.clone(), ty_of_column_slot(col));
    }
    Row { fields, rest: None }
}

/// The attributes-row type for a column: `ty_of_column` widened with
/// `Nil` where the schema says the column is nullable. Rails stores
/// NULL there until something sets it, so a read genuinely can be nil
/// — validations and analysis both need to see that. The primary key
/// is excluded (the INSERT assigns it and every hydration path treats
/// it as present). Twin of `lower::model_to_library::ty_of_column_slot`;
/// keep them in sync, same as the `ty_of_column` pair below.
fn ty_of_column_slot(col: &crate::schema::Column) -> Ty {
    let base = ty_of_column(&col.col_type);
    if col.nullable && !col.primary_key {
        Ty::Union { variants: vec![base, Ty::Nil] }
    } else {
        base
    }
}

fn ty_of_column(t: &ColumnType) -> Ty {
    // A datetime column is a `Time` at the Ruby source level (every use
    // in practice is `created_at.strftime` / `.to_i` / `.after?` /
    // `Time.current` assignment / passed to a Time helper — never a
    // String), so the analyzer sees the first-class `Ty::Time` for those
    // calls to resolve. The runtime *stores* them as ISO-8601 strings,
    // which is the target's column-seam concern (hydration/serialization),
    // not the type. The EMIT-side `lower::model_to_library::ty_of_column`
    // agrees on `Ty::Time` — a target with no native datetime type wired
    // yet surfaces the honest not-supported gap there.
    match t {
        ColumnType::Integer | ColumnType::BigInt => Ty::Int,
        ColumnType::Float | ColumnType::Decimal { .. } => Ty::Float,
        ColumnType::String { .. } | ColumnType::Text => Ty::Str,
        ColumnType::Boolean => Ty::Bool,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Time => Ty::Time,
        ColumnType::Binary => Ty::Str,
        // A `json` column is stored TEXT and nothing parses it: the
        // Row field, hydration, `[]`, `attributes` and the adapter's
        // escape all move the serialized string. `Hash[String, String]`
        // was a declaration no synthesized path implemented. What gives
        // such a column STRUCTURE is a `has_json` declaration, and that
        // is modeled as typed per-key accessors over this text
        // (`lower::has_json`), not as a Hash the whole column decodes to.
        ColumnType::Json => Ty::Str,
        ColumnType::Reference { .. } => Ty::Int,
    }
}
