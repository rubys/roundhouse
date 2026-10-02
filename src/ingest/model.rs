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
use super::visibility::{self, Visibility};
use super::util::{
    class_name_path, collect_comments, constant_id_str, constant_path_of, drain_comments_before,
    find_first_class, flatten_statements, source_has_blank_line, string_value, symbol_or_string_value,
    symbol_value,
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

mod enum_constants;
pub(super) use enum_constants::EnumConstants;

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
    let mut constants = EnumConstants::default();
    constants.record(source, file);
    constants.finish();
    ingest_model_with_enum_constants(source, file, schema, prefixes, &constants)
}

pub(super) fn ingest_model_with_enum_constants(
    source: &[u8],
    file: &str,
    schema: &Schema,
    prefixes: &TablePrefixes,
    enum_constants: &EnumConstants,
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

    // Syntactic nesting, not every prefix of the class name: `module
    // Admin::Nested` does not put `Admin` in Ruby's lexical search path.
    let enum_owners = enum_constants.nesting
        .get(&(file.to_string(), class.location().start_offset()))
        .cloned().unwrap_or_default();
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
    let table_decl = class.body().map(|body| parse_table_name_decl(body, file)).transpose()?.flatten();
    let table_name = if let Some((name, _)) = &table_decl {
        name.clone()
    } else {
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
    let mut enum_defaults: IndexMap<Symbol, Literal> = IndexMap::new();
    let mut primary_key: Option<Symbol> = None;
    let visibility = Visibility::resolve(class.body().as_ref(), file)?;
    if let Some(class_body) = class.body() {
        let mut prev_end: Option<usize> = None;
        // Constants the class body assigns, for `enum :x, CONST`.
        let stmts = flatten_statements(class_body);
        let mut class_consts: std::collections::HashMap<String, Vec<(String, Literal)>> =
            std::collections::HashMap::new();
        for stmt in &stmts {
            if let Some(cw) = stmt.as_constant_write_node() {
                // Keep the existing class-local folding boundary: qualified
                // cross-file constants are enum inputs, not arbitrary aliases.
                if let Some(labels) = enum_label_values(&cw.value(), &|node| {
                    class_consts.get(constant_id_str(&node.as_constant_read_node()?.name())).cloned()
                }) {
                    class_consts.insert(constant_id_str(&cw.name()).to_string(), labels);
                }
            }
        }
        let resolve_constant = |node: &Node<'_>| {
            if let Some(read) = node.as_constant_read_node() {
                class_consts.get(constant_id_str(&read.name())).cloned()
            } else {
                enum_constants.resolve(node, &enum_owners)
            }
        };
        for statement in stmts {
            let definition = visibility::definition(&statement).map(|d| d.as_node());
            let stmt = definition.as_ref().unwrap_or(&statement);
            if stmt.as_def_node().is_none() && statement.as_call_node().is_some_and(|c| visibility::marker(&c)) {
                prev_end = Some(statement.location().end_offset());
                continue;
            }
            // Explicit names override convention before schema binding.
            // Like primary_key, the setter is consumed: lowering already
            // synthesizes table_name from Model::table for every target.
            if table_decl.as_ref().is_some_and(|(_, offset)| *offset == stmt.location().start_offset()) {
                prev_end = Some(stmt.location().end_offset());
                continue;
            }
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
                match expand_enum_decl(&call, file, &leading, &resolve_constant) {
                    Ok(Some(expanded)) => {
                        if let Some(d) = expanded.default {
                            enum_defaults.insert(expanded.column.clone(), d);
                        }
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
                match ingest_singleton_class_methods(&sc, file, &visibility) {
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
            // A nested class or module is a class of its own, not a body
            // item: `class Row < T::Struct` inside a model or controller
            // is the same declaration it would be in `app/services`, and
            // the library-class pass over this very file registers it
            // under its qualified name. Reaching the expression ingester
            // with it aborted the whole file.
            if stmt.as_class_node().is_some() || stmt.as_module_node().is_some() {
                prev_end = Some(stmt.location().end_offset());
                continue;
            }
            let items = match ingest_model_body_items(&stmt, &owner, file, leading) {
                Ok(items) => items,
                Err(err) if super::survey::is_active() => {
                    super::survey::record(&err);
                    prev_end = Some(stmt.location().end_offset());
                    continue;
                }
                Err(err) => return Err(err),
            };
            // A multi-attribute `validates` expands to one item per
            // attribute; only the first carries the blank line that
            // separated the declaration from what came before it.
            for (i, mut item) in items.into_iter().enumerate() {
                if let ModelBodyItem::Method { method, .. } = &mut item {
                    visibility.apply(&statement, method);
                } else if let ModelBodyItem::Unknown { .. } = &item {
                    visibility.check_model_item(&statement, file)?;
                }
                item.set_leading_blank_line(leading_blank && i == 0);
                body.push(item);
            }
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
        sti_subclass_names: Vec::new(),
        table: TableRef(Symbol::from(table_name)),
        primary_key,
        attributes,
        body,
        enums,
        enum_defaults,
        span: Span {
            file: super::sources::file_id(file),
            start: class_loc.start_offset() as u32,
            end: class_loc.end_offset() as u32,
        },
    }))
}

/// Classify one class-body statement into its `ModelBodyItem` variant.
/// One statement of a model body, as one or MORE items.
///
/// `validates :title, :url, presence: true` declares one Validation per
/// ATTRIBUTE, and `ingest_model_body_item` can only answer with one —
/// its own comment said the tail was dropped and that "no real fixture
/// triggers this yet". campfire's `Opengraph::Metadata` triggers it:
/// `validates_presence_of :title, :url, :description` must fault all
/// three, and the test reads `errors.full_messages` for two of them.
///
/// The `validates_*_of` family is the same declaration in Rails' older
/// spelling — `validates_presence_of :a` IS `validates :a, presence:
/// true` — so it lands here rather than growing a second rule table.
/// BARE SYMBOLS ONLY: an options hash (`allow_nil:`, `if:`, `on:`)
/// narrows or reshapes the check, and the call falls through to the
/// unsupported-DSL ledger rather than being flattened into an
/// unconditional one.
pub(super) fn ingest_model_body_items(
    stmt: &Node<'_>,
    owner: &ClassId,
    file: &str,
    leading_comments: Vec<Comment>,
) -> IngestResult<Vec<ModelBodyItem>> {
    use crate::dialect::{Validation, ValidationRule};
    let span = Span {
        file: super::sources::file_id(file),
        start: stmt.location().start_offset() as u32,
        end: stmt.location().end_offset() as u32,
    };
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_none() {
            let name = constant_id_str(&call.name()).to_string();
            let rule = match name.as_str() {
                "validates_presence_of" => Some(ValidationRule::Presence),
                "validates_absence_of" => Some(ValidationRule::Absence),
                _ => None,
            };
            let parsed: Vec<Validation> = if name == "validates" {
                parse_validates(&call)
            } else if let Some(rule) = rule.clone() {
                let args: Vec<Node<'_>> = call
                    .arguments()
                    .map(|a| a.arguments().iter().collect())
                    .unwrap_or_default();
                let attrs: Vec<Symbol> = args
                    .iter()
                    .filter_map(|a| symbol_value(a).map(|s| Symbol::from(s.as_str())))
                    .collect();
                if attrs.len() == args.len() && !attrs.is_empty() {
                    attrs
                        .into_iter()
                        .map(|attribute| Validation { attribute, rules: vec![rule.clone()] })
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            if parsed.len() > 1 || (rule.is_some() && !parsed.is_empty()) {
                return Ok(parsed
                    .into_iter()
                    .enumerate()
                    .map(|(i, validation)| ModelBodyItem::Validation {
                        validation,
                        // The declaration's comments belong to the
                        // first of the items it expands to.
                        leading_comments: if i == 0 {
                            leading_comments.clone()
                        } else {
                            Vec::new()
                        },
                        leading_blank_line: false,
                        span,
                    })
                    .collect());
            }
        }
    }
    Ok(vec![ingest_model_body_item(stmt, owner, file, leading_comments)?])
}

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
        // `validate :method_name, :other` — the CUSTOM-validator form,
        // where each symbol names an instance method that adds to
        // `errors` itself. Distinct from `validates` (attribute + rules)
        // and carried as `ValidationRule::Custom`, a variant the dialect
        // and `lower::validations` have always had and nothing ever
        // produced.
        //
        // Bare symbols ONLY: `validate :x, on: :update` runs on one
        // persistence context, `if:`/`unless:` on a condition, and
        // running such a check unconditionally would reject records
        // Rails accepts. Those keep today's behaviour (the call falls
        // through to the unsupported-DSL ledger) rather than being
        // silently promoted to an always-on check.
        if method == "validate" {
            let symbols: Vec<Symbol> = call
                .arguments()
                .map(|args| {
                    args.arguments()
                        .iter()
                        .filter_map(|a| symbol_value(&a).map(|s| Symbol::from(s.as_str())))
                        .collect()
                })
                .unwrap_or_default();
            let arg_count = call
                .arguments()
                .map(|args| args.arguments().iter().count())
                .unwrap_or(0);
            if !symbols.is_empty() && symbols.len() == arg_count {
                // ONE Validation carrying one Custom rule per symbol:
                // this function returns a single body item, and
                // `push_validate_method` walks `rules`, so the list
                // rides where it is already read. The attribute names
                // the first method — a custom validator faults whatever
                // field it likes, so there is no attribute to record,
                // and leaving it self-describing beats a placeholder.
                let attribute = symbols[0].clone();
                return Ok(ModelBodyItem::Validation {
                    validation: crate::dialect::Validation {
                        attribute,
                        rules: symbols
                            .into_iter()
                            .map(|m| crate::dialect::ValidationRule::Custom { method: m })
                            .collect(),
                    },
                    leading_comments,
                    leading_blank_line: false,
                    span,
                });
            }
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
        // The same classifier sees model declarations and a concern's
        // `included do`. Record before the latter drops the Unknown,
        // and before ingesting a block that may contain further gaps.
        // Name the DSL shape, not unverified gem ownership.
        if matches!(method.as_str(), "aasm" | "state_machine")
            && call.block().and_then(|b| b.as_block_node()).is_some()
        {
            super::survey::record(&IngestError::Unsupported {
                file: file.into(),
                message: format!("state-machine DSL block `{method}` is not modeled"),
            });
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
    /// The stored value `default:` names.
    pub default: Option<Literal>,
    pub items: Vec<ModelBodyItem>,
}

/// The same syntax contract serves expansion and post-ingest validation.
/// Validation reads declarations only, never reconstructs model IR.
struct EnumDeclaration<'pr> {
    column: String,
    mapping: Node<'pr>,
    prefix: String,
    suffix: String,
    /// The label `default:` names, which seeds a new record over the
    /// column default. Computed here because only this parse sees the
    /// options hash.
    default_label: Option<String>,
}

fn enum_declaration<'pr>(call: &ruby_prism::CallNode<'pr>) -> Option<EnumDeclaration<'pr>> {
    if call.receiver().is_some() || constant_id_str(&call.name()) != "enum" {
        return None;
    }
    let args = call.arguments()?;
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;

    // Two spellings: `enum :status, <mapping>, **opts` (Rails 7) and the
    // older `enum status: <mapping>, **opts`, where the column and its
    // mapping are the first pair of one keyword hash.
    let (column, mapping_node, prefix, suffix, default_label) = match symbol_value(&first) {
        Some(col) => {
            let column: String = col;
            let mapping = iter.next();
            let opts = iter.next();
            let (prefix, suffix, default_label) = match opts.as_ref().and_then(|o| o.as_keyword_hash_node()) {
                Some(kh) => {
                    let (p, s) = enum_affixes(&kh.elements(), &column);
                    (p, s, enum_default_label(&kh.elements()))
                }
                None => (String::new(), String::new(), None),
            };
            (column, mapping, prefix, suffix, default_label)
        }
        None => {
            let kh = first.as_keyword_hash_node()?;
            let elements = kh.elements();
            let pair = elements.iter().next()?.as_assoc_node()?;
            let column = symbol_value(&pair.key())?;
            let (prefix, suffix) = enum_affixes(&elements, &column);
            let default_label = enum_default_label(&elements);
            (column, Some(pair.value()), prefix, suffix, default_label)
        }
    };
    Some(EnumDeclaration { column, mapping: mapping_node?, prefix, suffix, default_label })
}

fn enum_mapping_error(file: &str, column: &str) -> IngestError {
    IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "enum :{} mapping must be an array or hash literal (or `%w[…].index_by(&:itself)`)",
            column
        ),
    }
}

fn validate_sorbet_enum_mappings(
    model: &Model,
    source: &crate::span::SourceFile,
    constants: &EnumConstants,
) -> IngestResult<()> {
    // This source was already parsed by ingest. Do not duplicate parse or
    // unrelated model diagnostics, or rebuild the model to check one mapping.
    let result = ruby_prism::parse(source.text.as_bytes());
    let root = result.node();
    let Some((_, class)) = super::util::find_all_classes_with_scope(&root).into_iter()
        .find(|(_, class)| class.location().start_offset() == model.span.start as usize)
        else { return Ok(()) };
    let Some(body) = class.body() else { return Ok(()) };
    let owners = constants.nesting.get(&(source.path.clone(), model.span.start as usize))
        .cloned().unwrap_or_default();
    for statement in flatten_statements(body) {
        let Some(declaration) = statement.as_call_node().and_then(|call| enum_declaration(&call))
            else { continue };
        if let Some(receiver) = serialized_enum_receiver(&declaration.mapping) {
            if constants.resolve(&receiver, &owners).is_none() {
                super::survey::unwrap_or_record::<()>(Err(enum_mapping_error(
                    &source.path, &declaration.column,
                )))?;
            }
        }
    }
    Ok(())
}

pub(super) fn expand_enum_decl(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    leading_comments: &[crate::dialect::Comment],
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> IngestResult<Option<EnumExpansion>> {
    use crate::dialect::{MethodDef, MethodReceiver, Scope};
    use crate::effect::EffectSet;

    let Some(EnumDeclaration { column, mapping: mapping_node, prefix, suffix, default_label }) =
        enum_declaration(call)
        else { return Ok(None) };
    // `enum :status, STATUSES` — the mapping named by a constant the class
    // body assigned above (`STATUSES = %i[…].freeze`) — and everything
    // else `enum_label_values` resolves, now including a computed
    // mapping over that same constant (`enum :x, STATUSES.map { |s|
    // [s, s.to_s] }.to_h`).
    // A Sorbet serialize-pair returns a HASH, not an array operand for the
    // legacy recursive transformations. Admit it only as a complete mapping.
    let labels = serialized_enum_receiver(&mapping_node)
        .and_then(|receiver| resolve_constant(&receiver))
        .or_else(|| enum_label_values(&mapping_node, resolve_constant))
        .ok_or_else(|| enum_mapping_error(file, &column))?;
    let all_labels = labels.clone();
    let default = default_label.and_then(|d| labels.iter().find(|(l, _)| *l == d).map(|(_, v)| v.clone()));
    // A label that is not a Ruby identifier (`32bits`, `64bits`) has no
    // predicate, scope or bang writer Ruby could name: Rails reaches them
    // through `send`, which the emit has no equivalent of. Skipped.
    let labels: Vec<(String, Literal)> = labels
        .into_iter()
        .filter(|(l, _)| {
            l.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .collect();

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
    // Not the stored value: the reader answers the label, as Rails' does.
    let reads_label = crate::dialect::enum_mapping_reads_label(&labels);
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
                name_span: crate::span::Span::synthetic(),
                name: Symbol::from(name),
                receiver: MethodReceiver::Instance,
                visibility: crate::dialect::MethodVisibility::Public,
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
        // `not_published`: Rails generates the negative scope beside each positive one.
        let where_not = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Send { recv: None, method: Symbol::from("where"), args: vec![], block: None, parenthesized: false },
                )),
                method: Symbol::from("not"),
                args: vec![pair.clone()],
                block: None,
                parenthesized: true,
            },
        );
        items.push(ModelBodyItem::Scope {
            scope: Scope { name: Symbol::from(format!("not_{base}")), params: Vec::new(), body: where_not },
            leading_comments: Vec::new(),
            leading_blank_line: false,
        });
        items.push(method_def(
            format!("{base}?"),
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(column_read()),
                    method: Symbol::from("=="),
                    args: vec![Expr::new(
                        span,
                        ExprNode::Lit {
                            value: if reads_label { Literal::Str { value: label.clone() } } else { value },
                        },
                    )],
                    block: None,
                    parenthesized: false,
                },
            ),
        ));
        items.push(method_def(format!("{base}!"), call_with_pair("update!")));
    }
    // `Model.statuses`: every label, identifier or not, keyed by String as Rails' mapping is.
    let mapping_hash = Expr::new(
        span,
        ExprNode::Hash {
            entries: all_labels
                .iter()
                .map(|(label, value)| {
                    (
                        Expr::new(span, ExprNode::Lit { value: Literal::Str { value: label.clone() } }),
                        Expr::new(span, ExprNode::Lit { value: value.clone() }),
                    )
                })
                .collect(),
            kwargs: false,
        },
    );
    items.push(ModelBodyItem::Method {
        method: MethodDef {
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from(crate::naming::pluralize_snake(&column)),
            receiver: MethodReceiver::Class,
            visibility: crate::dialect::MethodVisibility::Public,
            params: Vec::new(),
            block_param: None,
            body: mapping_hash,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: crate::dialect::AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        },
        leading_comments: Vec::new(),
        leading_blank_line: false,
    });
    Ok(Some(EnumExpansion { column: Symbol::from(column.as_str()), mapping: labels, default, items }))
}

/// Label → stored value for an `enum` mapping. An array literal maps by
/// index the way Rails does (`%i[active deactivated]` → 0, 1); a hash
/// literal carries its own values; `%w[…].index_by(&:itself)` — the
/// idiom for a string-backed column — maps each label to itself; a bare
/// `CONST` resolves through `class_consts` (the class body's own
/// `CONST = %i[…]` assignments, collected before this ever runs); a
/// qualified constant resolves only a cross-file literal string array; and
/// `<array-expr>.map { |v| [v, v.to_s] }.to_h` / `.index_by(&:to_s)` /
/// `.index_with(&:to_s)` recurse into whichever of the above
/// `<array-expr>` already is — Procore's `bid_package.rb` and
/// `potential_change_order.rb` compute their (string-backed) mapping
/// this way from a `CONST` instead of writing the hash out by hand, to
/// keep the column and the constant's allowed values in one place.
/// `None` for anything else (a computed hash, an unresolvable
/// constant), which the caller reports as a gap rather than guessing at
/// storage.
fn enum_label_values(
    node: &Node<'_>,
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> Option<Vec<(String, Literal)>> {
    // `CONST` — folded in here (rather than only at the `enum_label_values`
    // call sites) so a computed mapping's `<array-expr>` can ALSO be a
    // constant, not just the top-level `enum :x, CONST` spelling.
    if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some() {
        return resolve_constant(node);
    }
    // `%i[…].freeze` / `{ … }.freeze` — the literal is the receiver.
    if let Some(call) = node.as_call_node() {
        if constant_id_str(&call.name()) == "freeze" && call.arguments().is_none() && call.block().is_none() {
            if let Some(recv) = call.receiver() {
                return enum_label_values(&recv, resolve_constant);
            }
        }
    }
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
                    Literal::Int { value: super::util::integer_i64(&raw.value())? }
                };
                Some((label, lit))
            })
            .collect();
    }
    let call = node.as_call_node()?;
    let call_name = constant_id_str(&call.name());

    // `<array-expr>.index_by(&:itself)` / `.index_by(&:to_s)` /
    // `.index_with(&:itself)` / `.index_with(&:to_s)` — identity string
    // mappings over the literal/constant input resolved recursively.
    if call_name == "index_by" || call_name == "index_with" {
        // These are identity mappings only for the two explicit symbol
        // procs. Arbitrary blocks (e.g. &:length) must stay ledgered.
        let block = call.block()?.as_block_argument_node()?;
        let proc = symbol_value(&block.expression()?)?;
        if call.arguments().is_some() || !matches!(proc.as_str(), "itself" | "to_s") {
            return None;
        }
        let recv = call.receiver()?;
        let labels = enum_label_values(&recv, resolve_constant)?;
        return Some(
            labels
                .into_iter()
                .map(|(label, _)| (label.clone(), Literal::Str { value: label }))
                .collect(),
        );
    }

    // `<array-expr>.map { |v| [v, v.to_s] }.to_h` — the other spelling
    // of the same identity string mapping. Recognized narrowly: the
    // `map` block takes exactly one parameter, and its body is a
    // single statement — a 2-element array literal `[v, v.to_s]` built
    // from that same parameter. Anything looser (a different second
    // element, extra elements, multiple statements, a differently
    // shaped block) falls through to `None` — the caller's refusal —
    // rather than guessing at storage.
    if call_name == "to_h" && call.arguments().is_none() {
        let map_call = call.receiver()?;
        let map_call = map_call.as_call_node()?;
        if constant_id_str(&map_call.name()) != "map" {
            return None;
        }
        let recv = map_call.receiver()?;
        let labels = enum_label_values(&recv, resolve_constant)?;

        let block = map_call.block()?.as_block_node()?;
        let block_params = block.parameters()?.as_block_parameters_node()?.parameters()?;
        let requireds: Vec<_> = block_params.requireds().iter().collect();
        let [only_param] = &requireds[..] else { return None };
        let only_param = only_param.as_required_parameter_node()?;
        let param_name = constant_id_str(&only_param.name());

        let body_stmts = flatten_statements(block.body()?);
        let [body_stmt] = &body_stmts[..] else { return None };
        let pair = body_stmt.as_array_node()?;
        let elements: Vec<_> = pair.elements().iter().collect();
        let [first, second] = &elements[..] else { return None };

        // First element: a bare read of the block param.
        let lv = first.as_local_variable_read_node()?;
        if constant_id_str(&lv.name()) != param_name {
            return None;
        }
        // Second element: `<same param>.to_s`.
        let to_s_call = second.as_call_node()?;
        if constant_id_str(&to_s_call.name()) != "to_s" || to_s_call.arguments().is_some() {
            return None;
        }
        let to_s_recv = to_s_call.receiver()?.as_local_variable_read_node()?;
        if constant_id_str(&to_s_recv.name()) != param_name {
            return None;
        }

        return Some(
            labels
                .into_iter()
                .map(|(label, _)| (label.clone(), Literal::Str { value: label }))
                .collect(),
        );
    }

    None
}

/// The one admitted consumer of enum-valued reads. Shared with the source
/// guard so a mutating block cannot hide behind a method name like `to_h`.
fn serialized_enum_receiver<'pr>(node: &Node<'pr>) -> Option<Node<'pr>> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) == "freeze"
        && call.arguments().is_none() && call.block().is_none() {
        return serialized_enum_receiver(&call.receiver()?);
    }
    if constant_id_str(&call.name()) != "to_h" || call.arguments().is_some() {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    let parameters = block.parameters()?.as_block_parameters_node()?;
    let params = parameters.parameters()?;
    if parameters.locals().iter().next().is_some() || params.optionals().iter().next().is_some()
        || params.rest().is_some() || params.posts().iter().next().is_some()
        || params.keywords().iter().next().is_some() || params.keyword_rest().is_some()
        || params.block().is_some() {
        return None;
    }
    let requireds: Vec<_> = params.requireds().iter().collect();
    let [parameter] = requireds.as_slice() else { return None };
    let name = parameter.as_required_parameter_node()?.name();
    let statements = flatten_statements(block.body()?);
    let [statement] = statements.as_slice() else { return None };
    let pair = statement.as_array_node()?;
    if pair.elements().iter().count() != 2 || pair.elements().iter().any(|element| {
        let Some(call) = element.as_call_node() else { return true };
        constant_id_str(&call.name()) != "serialize" || call.arguments().is_some()
            || call.block().is_some() || !call.receiver().and_then(|r| r.as_local_variable_read_node())
                .is_some_and(|read| constant_id_str(&read.name()) == constant_id_str(&name))
    }) {
        return None;
    }
    let receiver = call.receiver()?;
    let values = receiver.as_call_node()?;
    (constant_id_str(&values.name()) == "values"
        && values.arguments().is_none() && values.block().is_none()).then_some(receiver)
}

/// `prefix:`/`suffix:` from an `enum`'s option hash. `true` means "use
/// the column name" (Rails' own convention); a symbol or string names
/// the affix directly. Returns the strings to splice around each label,
/// already carrying their separating underscore.
// `_default:` is the pre-Rails-7 spelling, as `_prefix:` is.
fn enum_default_label(elements: &ruby_prism::NodeList<'_>) -> Option<String> {
    elements.iter().find_map(|el| {
        let assoc = el.as_assoc_node()?;
        let key = symbol_value(&assoc.key())?;
        if key.trim_start_matches('_') != "default" {
            return None;
        }
        symbol_value(&assoc.value()).or_else(|| string_value(&assoc.value()))
    })
}

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
/// declares. Visibility has already been resolved in lexical order;
/// unsupported singleton statements still fail rather than disappearing.
fn ingest_singleton_class_methods(
    sc: &ruby_prism::SingletonClassNode<'_>,
    file: &str,
    visibility: &Visibility,
) -> IngestResult<Vec<crate::dialect::MethodDef>> {
    use crate::dialect::MethodReceiver;

    let Some(body) = sc.body() else { return Ok(Vec::new()) };
    let mut methods: Vec<crate::dialect::MethodDef> = Vec::new();
    for statement in super::util::flatten_statements(body) {
        let definition = visibility::definition(&statement).map(|d| d.as_node());
        let stmt = definition.as_ref().unwrap_or(&statement);
        if let Some(call) = stmt.as_call_node() {
            if visibility::marker(&call) {
                continue;
            }
            // `deprecate(name: { message: …, deprecator: … })` — pure
            // call-site metadata (ActiveSupport::Deprecation wraps the
            // method to warn; callers still dispatch through it), no
            // singleton-scope state to carry. `ChangeOrderRequest`,
            // `ChangeOrderPackage`, and `PotentialChangeOrder` all
            // deprecate a class method exactly this way. Dropped like
            // an unknown-call annotation elsewhere, rather than
            // refused — refusing killed the whole model's ingest for
            // one annotation on an otherwise-modeled class method.
            if call.receiver().is_none()
                && call.block().is_none()
                && constant_id_str(&call.name()) == "deprecate"
            {
                continue;
            }
            // `alias_method :new_name, :old_name` — the other call
            // shape the corpus uses here (`PaymentApplicationMarkup
            // LineItem` aliases `vattr` to `virtual_attribute`).
            // Unlike the marker/annotation cases above, this DOES need
            // modeling: `vattr` is called from sibling class methods.
            // Clone the already-ingested target — `alias_method`
            // always follows its target in this corpus — under the
            // new name. A target ingested outside this singleton
            // block, or not found, falls through to the refusal below
            // rather than silently doing nothing.
            if call.receiver().is_none()
                && call.block().is_none()
                && constant_id_str(&call.name()) == "alias_method"
            {
                if let Some(args) = call.arguments() {
                    let args: Vec<_> = args.arguments().iter().collect();
                    if let [new_name, old_name] = &args[..] {
                        if let (Some(new_name), Some(old_name)) =
                            (symbol_value(new_name), symbol_value(old_name))
                        {
                            if let Some(target) =
                                methods.iter().find(|m| m.name.as_str() == old_name)
                            {
                                let mut alias = target.clone();
                                alias.name = Symbol::from(new_name);
                                alias.name_span = Span::synthetic();
                                visibility.apply(&statement, &mut alias);
                                methods.push(alias);
                                continue;
                            }
                        }
                    }
                }
            }
        }
        let Some(def) = stmt.as_def_node() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported statement inside `class << self`: {stmt:?}"),
            });
        };
        let mut method = ingest_method(&def, file)?;
        method.receiver = MethodReceiver::Class;
        visibility.apply(&statement, &mut method);
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
    // at every call site that passes one. Every parameter kind the
    // library-class path records is recorded here too, in Ruby's
    // declaration order, so the `def` keeps the source arity.
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
        // `*rest`, and the required params Ruby allows after it
        // (`def pair(first, *rest)`, `def f(*rest, last)`). Dropping
        // the splat left `def tagged` with a body still reading
        // `labels` — an ArgumentError at every call site that passes
        // one, while the same method on a plain class kept it. An
        // anonymous `*` has no name to bind and is skipped, as the
        // library-class path skips it.
        if let Some(rest) = pn.rest() {
            if let Some(loc) = rest.as_rest_parameter_node().and_then(|rp| rp.name()) {
                params.push(crate::dialect::Param::rest(Symbol::from(constant_id_str(&loc))));
            }
        }
        for post in pn.posts().iter() {
            if let Some(pp) = post.as_required_parameter_node() {
                params.push(crate::dialect::Param::positional(Symbol::from(
                    constant_id_str(&pp.name()),
                )));
            }
        }
        // Keyword params (`def recent_threads(amount, for_user: nil)`),
        // required (`k:`) and optional (`k: default`) alike. Dropping
        // them left `def recent_threads(amount)` with a body reading
        // `for_user` — an ArgumentError at every kwarg call site.
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
        // `**params`: the same trailing positional-defaulting-to-`{}`
        // that `ingest::library_class` records, MARKED `from_kwrest`
        // for `lower::kwrest_forward`. Dropping it left campfire's
        // `Push::Subscription#notification(**params)` as `def
        // notification` with a body still reading `params` — every
        // caller an ArgumentError, and the forward into
        // `WebPush::Notification.new(**params, …)` a bare name.
        //
        // Beside a positional `*rest` the flattening does not parse
        // (`def both(*args, options = {})`), so there the slot stays a
        // real `**kwrest`, as the library-class path keeps it.
        if let Some(krest) = pn.keyword_rest() {
            if let Some(krp) = krest.as_keyword_rest_parameter_node() {
                if let Some(loc) = krp.name() {
                    let name = Symbol::from(constant_id_str(&loc));
                    let mut p = if params.iter().any(|p| p.rest) {
                        let mut p = crate::dialect::Param::keyword(name, None);
                        p.rest = true;
                        p
                    } else {
                        crate::dialect::Param::with_default(
                            name,
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Hash { entries: vec![], kwargs: false },
                            ),
                        )
                    };
                    p.from_kwrest = true;
                    params.push(p);
                }
            }
        }
    }

    // `&blk` rides in `MethodDef.block_param`, not the flat list, as
    // the library-class path records it: it fills the call-site
    // `block:` slot, and the emitter closes the `def` with `&blk` so a
    // body that passes it on (`each(&blk)`) still binds the name.
    // Ruby 3.4's anonymous `&` gets the same synthesized name the
    // library-class path gives it, so bare-`&` forwarding binds.
    let block_param = def.parameters().and_then(|pn| pn.block()).map(|block| {
        let name = block
            .name()
            .and_then(|loc| std::str::from_utf8(loc.as_slice()).ok())
            .unwrap_or("__blk");
        crate::dialect::Param::positional(Symbol::from(name))
    });

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name_span: super::util::def_name_span(def, file),
        name,
        receiver,
        visibility: crate::dialect::MethodVisibility::Public,
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
        block_param,
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
        // Rails' per-lifecycle SUGAR for `after_commit … on: <event>`.
        // Only the block form of these was ever recognized
        // (`BLOCK_CALLBACK_HOOKS`); the symbol form fell through to
        // Unknown, so campfire's `after_create_commit
        // :grant_membership_to_open_rooms` and `Rooms::Open`'s
        // `after_save_commit :grant_access_to_all_users` were DROPPED —
        // the method emitted and nothing ever called it.
        //
        // Each maps to the runtime hook of the same name, which
        // `save_after_validation` / `destroy` already fire in Rails'
        // order. `on:` is not accepted alongside them, exactly as Rails
        // refuses it ("You can't specify on with after_create_commit").
        "after_create_commit" => CallbackHook::AfterCommit,
        "after_update_commit" => CallbackHook::AfterCommit,
        "after_destroy_commit" => CallbackHook::AfterCommit,
        "after_save_commit" => CallbackHook::AfterSaveCommit,
        "after_rollback" => CallbackHook::AfterRollback,
        _ => return None,
    };
    // The lifecycle the sugar spelling pins, which `push_symbol_callback`
    // reads back off `on:` to pick the hook method.
    let sugar_on = match method {
        "after_create_commit" => Some(CallbackOn::Create),
        "after_update_commit" => Some(CallbackOn::Update),
        "after_destroy_commit" => Some(CallbackOn::Destroy),
        _ => None,
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
    // A sugar spelling pins its own lifecycle; an explicit `on:` beside
    // it is not Rails and is not guessed at.
    if sugar_on.is_some() {
        if on.is_some() {
            return None;
        }
        on = sugar_on;
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
    // `scope :for_tools, (lambda do |tools| … end)` — Procore wraps the
    // spelled-out form in its own parens (`reports/app/models/
    // report.rb`'s `for_tools`, `for_data_sets`, `shared`). The parens
    // are surface-only (same treatment `ingest_expr` gives them
    // generally); unwrapping to the single inner statement is what
    // lets the `lambda`/`proc`/`->` checks below see the call or
    // lambda node they expect instead of a `ParenthesesNode`.
    let body_node = unwrap_parenthesized_single_statement(body_node);
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

/// `(expr)` around a single statement — surface-only parens, same as
/// the ones `ingest_expr`'s own `ParenthesesNode` arm strips. Only
/// unwraps when there's exactly one statement inside: `(a; b)`'s two
/// void statements aren't a lambda in disguise, so those are left
/// alone and fail the caller's lambda check same as before.
fn unwrap_parenthesized_single_statement(node: Node<'_>) -> Node<'_> {
    let Some(paren) = node.as_parentheses_node() else { return node };
    let Some(inner) = paren.body() else { return node };
    let mut stmts = flatten_statements(inner);
    if stmts.len() == 1 {
        stmts.pop().unwrap()
    } else {
        node
    }
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
    let mut belongs_to_default: Option<crate::expr::Expr> = None;
    let mut touch: Option<crate::dialect::Touch> = None;

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
                // `touch: true` / `touch: :last_message_at`. A `false`
                // is Rails' explicit opt-out and reads as no touch,
                // which is what `None` already means.
                "touch" => {
                    touch = match bool_value(&value) {
                        Some(true) => Some(crate::dialect::Touch::UpdatedAt),
                        Some(false) => None,
                        None => symbol_value(&value)
                            .map(|s| crate::dialect::Touch::Column(Symbol::from(s.as_str()))),
                    }
                }
                "join_table" => join_table = string_value(&value),
                "polymorphic" => polymorphic = bool_value(&value),
                "as" => as_interface = symbol_value(&value),
                // `default: -> { Current.user }` — the lambda BODY, not
                // the lambda. Rails calls it with `instance_exec`, so
                // the body is already written against the record; a
                // param-taking form would need the record threaded and
                // nothing in the corpus writes one.
                "default" => {
                    belongs_to_default = value
                        .as_lambda_node()
                        .filter(|l| {
                            l.parameters()
                                .and_then(|p| p.as_block_parameters_node().and_then(|b| b.parameters()))
                                .map(|pn| pn.requireds().iter().next().is_none())
                                .unwrap_or(true)
                        })
                        .and_then(|l| l.body())
                        .and_then(|b| ingest_expr(&b, file).ok());
                }
                _ => {}
            }
        }
    }

    // Rails `foreign_key` demodulizes: `Billing::Invoice` → `invoice_id`.
    let owner_snake = snake_case(crate::naming::demodulize(owner.0.as_str()));

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
            // SINGULARIZED, because the source association can be a
            // has_many and then its name is PLURAL: campfire's
            // `has_many :reachable_messages, through: :rooms, source:
            // :messages` means Room#messages, so the class is Message.
            // Camelizing alone produced a `Messages` phantom — and
            // campfire has a `Messages::` controller MODULE by that
            // name, so the reader resolved to it and failed at the
            // `where` instead of at the missing constant. Singular
            // sources are unaffected (Rails' inflector answers
            // "story" for "story"), which is every other corpus use.
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
                .or_else(|| source.map(|s| ClassId(Symbol::from(singularize_camelize(s.as_str())))))
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
            default: belongs_to_default,
            touch,
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

/// Bind one direct literal string/symbol `self.table_name` before reading the schema.
/// Scan the selected class's executable body first: a conditional, compound
/// or subsequent write must not leave a plausible but incorrect row bound.
/// Method and nested namespace bodies are separate scopes; their headers
/// still execute in the enclosing scope and must not hide table writes.
fn parse_table_name_decl(body: Node<'_>, file: &str) -> IngestResult<Option<(String, usize)>> {
    struct Collector {
        direct: Vec<(usize, usize)>,
        writes: Vec<Option<(String, usize)>>,
    }
    impl Collector {
        fn record(&mut self, node: &Node<'_>) {
            if let Some(call) = node.as_call_node() {
                if constant_id_str(&call.name()) != "table_name=" { return; }
                let offset = node.location().start_offset();
                let valid = self.direct.contains(&(offset, node.location().end_offset()))
                    && call.receiver().is_some_and(|r| r.as_self_node().is_some())
                    && !call.is_safe_navigation() && call.block().is_none();
                let name = valid.then_some(())
                    .and_then(|_| call.arguments())
                    .filter(|args| args.arguments().len() == 1)
                    .and_then(|args| symbol_or_string_value(&args.arguments().iter().next()?))
                    // Shared DDL/DML currently emits bare table names.
                    // Refuse names needing qualification or SQL quoting.
                    .filter(|name| {
                        let mut bytes = name.bytes();
                        bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
                            && !crate::naming::is_sqlite_keyword(name)
                    });
                self.writes.push(name.map(|name| (name, offset)));
            } else {
                let name = node.as_call_and_write_node().map(|w| w.write_name())
                    .or_else(|| node.as_call_or_write_node().map(|w| w.write_name()))
                    .or_else(|| node.as_call_operator_write_node().map(|w| w.write_name()))
                    .or_else(|| node.as_call_target_node().map(|w| w.name()));
                if name.is_some_and(|name| constant_id_str(&name) == "table_name=") {
                    self.writes.push(None);
                }
            }
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Collector {
        fn visit_branch_node_enter(&mut self, node: Node<'pr>) { self.record(&node); }
        fn visit_leaf_node_enter(&mut self, node: Node<'pr>) { self.record(&node); }
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            if let Some(receiver) = node.receiver() { self.visit(&receiver); }
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            self.visit(&node.constant_path());
            if let Some(superclass) = node.superclass() { self.visit(&superclass); }
        }
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            self.visit(&node.constant_path());
        }
    }
    let mut collector = Collector {
        direct: body.as_statements_node()
            .map(|stmts| stmts.body().iter().filter_map(|s| s.as_call_node())
                .map(|s| (s.location().start_offset(), s.location().end_offset())).collect())
            .unwrap_or_else(|| vec![(body.location().start_offset(), body.location().end_offset())]),
        writes: Vec::new(),
    };
    ruby_prism::Visit::visit(&mut collector, &body);
    if collector.writes.is_empty() {
        Ok(None)
    } else if collector.writes.len() == 1 && collector.writes[0].is_some() {
        Ok(collector.writes.pop().unwrap())
    } else {
        Err(IngestError::Unsupported {
            file: file.into(),
            message: "table_name binding requires one direct self.table_name assignment to a literal string or symbol naming a safe bare SQL identifier".into(),
        })
    }
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

pub(crate) fn row_from_table(table: &Table) -> Row {
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
        ColumnType::Date => Ty::Date,
        ColumnType::DateTime | ColumnType::Time => Ty::Time,
        ColumnType::Binary => Ty::Str,
        // Rails exposes a schema-less JSON value here: it may be an
        // Array, Hash, scalar, or nil, so neither String nor one fixed
        // container type is honest. The emitted model keeps serialized
        // text in its DB slot and decodes/encodes at the public accessor
        // boundary (`JsonColumn`); analysis uses the deliberate gradual
        // type. A `has_json` declaration adds its stronger per-key schema
        // separately in `lower::has_json`.
        ColumnType::Json => Ty::Untyped,
        ColumnType::Uuid => Ty::Str,
        ColumnType::Reference { .. } => Ty::Int,
    }
}

#[cfg(test)]
mod singleton_visibility_tests {
    use super::*;

    fn ingest(src: &str) -> IngestResult<Option<crate::dialect::Model>> {
        ingest_model(
            src.as_bytes(),
            "app/models/thing.rb",
            &Schema::default(),
            &Default::default(),
        )
    }

    /// A bare `private` inside `class << self` used to abort the whole
    /// file's ingest ("unsupported statement inside `class << self`"),
    /// which drops the model rather than the marker.
    #[test]
    fn a_visibility_marker_in_a_singleton_block_is_resolved_not_refused() {
        let model = ingest(
            "class Thing < ApplicationRecord\n  \
             class << self\n    \
             def visible\n      1\n    end\n\n    \
             private\n      def hidden\n        2\n      end\n  \
             end\nend\n",
        )
        .expect("ingest must not fail on a visibility marker")
        .expect("model");
        let names: Vec<&str> = model
            .body
            .iter()
            .filter_map(|item| match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => Some(method.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["visible", "hidden"], "both singleton methods survive");
        let visibility: Vec<_> = model.body.iter().filter_map(|item| match item {
            crate::dialect::ModelBodyItem::Method { method, .. } => Some(method.visibility),
            _ => None,
        }).collect();
        assert_eq!(visibility, vec![crate::dialect::MethodVisibility::Public, crate::dialect::MethodVisibility::Private]);
    }

    /// A statement the walk genuinely cannot read still refuses — the
    /// skip is for the three markers, not for anything call-shaped.
    #[test]
    fn a_real_statement_in_a_singleton_block_still_refuses() {
        let err = ingest(
            "class Thing < ApplicationRecord\n  \
             class << self\n    attr_accessor :cache\n  end\nend\n",
        );
        assert!(err.is_err(), "an unmodeled singleton statement is still an error: {err:?}");
    }
}
