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

use super::expr::{ingest_expr, ingest_expr_strict};
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
pub(in crate::ingest) use enum_constants::EnumConstants;

/// Scan one file for `module <Ns>; def self.table_name_prefix; "<p>"; end`,
/// or the same def inside the module's `class << self`.
/// Deliberately narrow: only a module-level singleton def whose body is a
/// single string literal. A computed prefix would have to run to be known,
/// and nothing in the corpus writes one.
pub fn ingest_table_name_prefixes(source: &[u8], file: &str) -> TablePrefixes {
    fn literal_prefix(def: &ruby_prism::DefNode<'_>) -> Option<String> {
        if constant_id_str(&def.name()) != "table_name_prefix" {
            return None;
        }
        let stmts = flatten_statements(def.body()?);
        if stmts.len() != 1 {
            return None;
        }
        string_value(&stmts[0])
    }
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
        let stmts = flatten_statements(body);
        // `extend self` makes every instance method a singleton one too, so a
        // nested module can override an outer `ledger_` with a plain
        // `def table_name_prefix = ""`.
        let extends_self = stmts.iter().any(|s| {
            s.as_call_node().is_some_and(|c| {
                c.receiver().is_none()
                    && constant_id_str(&c.name()) == "extend"
                    && c.arguments().is_some_and(|a| {
                        let args: Vec<_> = a.arguments().iter().collect();
                        args.len() == 1 && args[0].as_self_node().is_some()
                    })
            })
        });
        for stmt in stmts {
            if let Some(def) = stmt.as_def_node() {
                let singleton = match def.receiver() {
                    Some(r) => r.as_self_node().is_some(),
                    None => extends_self,
                };
                if !singleton {
                    continue;
                }
                if let Some(prefix) = literal_prefix(&def) {
                    out.insert(full.join("::"), prefix);
                }
            } else if let Some(sclass) = stmt.as_singleton_class_node() {
                if sclass.expression().as_self_node().is_none() {
                    continue;
                }
                let Some(sbody) = sclass.body() else { continue };
                for inner in flatten_statements(sbody) {
                    let Some(def) = inner.as_def_node() else { continue };
                    if def.receiver().is_some() {
                        continue;
                    }
                    if let Some(prefix) = literal_prefix(&def) {
                        out.insert(full.join("::"), prefix);
                    }
                }
            }
        }
    }
    out
}

/// `self.table_name_prefix = "three_d_secure_"` in a model body: the
/// class's own prefix, which wins over any namespace module's.
fn explicit_table_prefix(body: ruby_prism::Node<'_>) -> Option<String> {
    explicit_class_setting(body, "table_name_prefix=")
}

fn explicit_class_setting(body: ruby_prism::Node<'_>, setter: &str) -> Option<String> {
    for stmt in flatten_statements(body) {
        let Some(call) = stmt.as_call_node() else { continue };
        if constant_id_str(&call.name()) != setter {
            continue;
        }
        if call.receiver().and_then(|r| r.as_self_node()).is_none() {
            continue;
        }
        let args = call.arguments()?;
        let first = args.arguments().iter().next()?;
        if let Some(name) = string_value(&first).or_else(|| symbol_value(&first)) {
            return Some(name);
        }
    }
    None
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
    let bases = super::library_class::ModelBases::new();
    ingest_model_with_enum_constants(source, file, schema, prefixes, &constants, &bases)
}

pub(super) fn ingest_model_with_enum_constants(
    source: &[u8],
    file: &str,
    schema: &Schema,
    prefixes: &TablePrefixes,
    enum_constants: &EnumConstants,
    model_bases: &super::library_class::ModelBases,
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
    let mut name_path = scope.clone();
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
        let mut prefix = class.body().and_then(explicit_table_prefix).unwrap_or_default();
        while prefix.is_empty() && !segments.is_empty() {
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
    let mut class_attr_defaults: IndexMap<Symbol, Expr> = IndexMap::new();
    let mut primary_key: Option<Symbol> = None;
    let visibility = Visibility::resolve(class.body().as_ref(), file, None)?;
    if let Some(class_body) = class.body() {
        let mut prev_end: Option<usize> = None;
        // Constants the class body assigns, for `enum :x, CONST` and for
        // labels spelled with a constant (`NAME => NAME`).
        let stmts = flatten_statements(class_body);
        let class_consts = ClassConsts::collect(&stmts, file);
        let resolve_constant = |node: &Node<'_>| enum_constants.resolve(node, &enum_owners);
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
                match expand_class_body_dsl(&call, file, &leading, &class_consts, &resolve_constant) {
                    Ok(Some(ClassBodyExpansion::DelegatedType(expanded))) => {
                        let mut blank = leading_blank;
                        for mut item in expanded {
                            item.set_leading_blank_line(std::mem::take(&mut blank));
                            body.push(item);
                        }
                        prev_end = Some(stmt.location().end_offset());
                        continue;
                    }
                    Ok(Some(ClassBodyExpansion::Enum(expanded))) => {
                        if let Some(d) = expanded.default {
                            enum_defaults.insert(expanded.column.clone(), d);
                        }
                        if let Some(mapping) = expanded.mapping {
                            enums.insert(expanded.column, mapping);
                        }
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
            //
            // `alias_method :to, :from` of an instance method this body
            // already defined copies that def, as the library walk does
            // (`library_class::alias_source`), and the copy takes the
            // visibility Ruby gives the alias: the original's
            // (`Visibility`'s walk). Not one whose body calls `super`:
            // Ruby's alias reaches the original name's super method and a
            // renamed copy would reach its own. Anything else stays an
            // unknown item.
            if let Some(mut copy) = stmt.as_call_node().and_then(|call| model_alias_method_copy(&call, &body)) {
                visibility.apply(&statement, &mut copy);
                body.push(ModelBodyItem::Method {
                    method: copy,
                    leading_comments: leading,
                    leading_blank_line: leading_blank,
                });
                prev_end = Some(stmt.location().end_offset());
                continue;
            }
            if let Some(alias) = stmt.as_alias_method_node() {
                let to = super::library_class::alias_keyword_name(&alias.new_name());
                let from = super::library_class::alias_keyword_name(&alias.old_name());
                let copied = to.zip(from).and_then(|(to, from)| {
                    body.iter().rev().find_map(|item| match item {
                        ModelBodyItem::Method { method, .. }
                            if method.name.as_str() == from
                                && method.receiver == crate::dialect::MethodReceiver::Instance =>
                        {
                            let mut copy = method.clone();
                            copy.name = crate::ident::Symbol::from(to.as_str());
                            Some(copy)
                        }
                        _ => None,
                    })
                });
                if let Some(method) = copied {
                    body.push(ModelBodyItem::Method {
                        method,
                        leading_comments: leading,
                        leading_blank_line: leading_blank,
                    });
                    prev_end = Some(stmt.location().end_offset());
                    continue;
                }
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "alias names a method this body has not defined".into(),
                });
            }
            if let Some(sc) = stmt.as_singleton_class_node() {
                match ingest_singleton_class_body(&sc, &owner, file, &visibility) {
                    Ok(singleton) => {
                        let mut leading = leading;
                        let mut blank = leading_blank;
                        let items = singleton
                            .methods
                            .into_iter()
                            .map(|method| ModelBodyItem::Method {
                                method,
                                leading_comments: Vec::new(),
                                leading_blank_line: false,
                            })
                            .chain(singleton.class_body.into_iter().map(|expr| {
                                ModelBodyItem::Unknown {
                                    expr,
                                    leading_comments: Vec::new(),
                                    leading_blank_line: false,
                                }
                            }));
                        for mut item in items {
                            *item.leading_comments_mut() = std::mem::take(&mut leading);
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
            //
            // Exception: a nested `class JSON` / `module JSON` shadows
            // bare `JSON` for `serialize …, coder: JSON` — leave a
            // synthetic Const-assign marker so lower::serialize can see
            // the shadow without treating the namespace as a body item.
            if let Some(path) = stmt
                .as_class_node()
                .and_then(|c| constant_path_of(&c.constant_path()))
                .or_else(|| {
                    stmt.as_module_node()
                        .and_then(|m| constant_path_of(&m.constant_path()))
                })
            {
                let path_syms: Vec<Symbol> = path.iter().map(|s| Symbol::from(s.as_str())).collect();
                if crate::lower::serialize::const_path_shadows_bare_json(
                    &path_syms,
                    owner.0.as_str(),
                ) {
                    body.push(ModelBodyItem::Unknown {
                        expr: Expr::new(
                            Span::synthetic(),
                            ExprNode::Assign {
                                target: crate::expr::LValue::Const {
                                    path: vec![Symbol::from("JSON")],
                                },
                                value: Expr::new(
                                    Span::synthetic(),
                                    ExprNode::Lit {
                                        value: crate::expr::Literal::Nil,
                                    },
                                ),
                            },
                        ),
                        leading_comments: Vec::new(),
                        leading_blank_line: false,
                    });
                }
                prev_end = Some(stmt.location().end_offset());
                continue;
            }
            let items = match ingest_model_body_items(
                &stmt,
                &owner,
                file,
                leading,
                Some(&mut class_attr_defaults),
            ) {
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
                    if let Err(err) = visibility.check_model_item(&statement, file) {
                        super::survey::continue_or_fail(err)?;
                        continue;
                    }
                }
                item.set_leading_blank_line(leading_blank && i == 0);
                body.push(item);
            }
            prev_end = Some(stmt.location().end_offset());
        }
    }

    let parent_span = class
        .superclass()
        .map(|n| super::util::node_span(&n, file))
        .unwrap_or_default();
    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| {
            let resolved = model_bases.resolve_superclass(&scope, &p);
            ClassId(Symbol::from(model_bases.emit_superclass(&resolved)))
        })
    });

    let class_loc = class.location();
    Ok(Some(Model {
        name: owner,
        parent,
        parent_span,
        sti_subclass_names: Vec::new(),
        table: TableRef(Symbol::from(table_name)),
        primary_key,
        attributes,
        body,
        enums,
        enum_defaults,
        class_attr_defaults,
        lexical_json_shadow: enum_constants.shadows_bare_json(&enum_owners),
        span: Span {
            file: super::sources::file_id(file),
            start: class_loc.start_offset() as u32,
            end: class_loc.end_offset() as u32,
        },
    }))
}

/// `alias_method :to, :from` (literal names) of the last instance def of
/// `from` already in `body`, renamed `to`. None when the def calls
/// `super`.
fn model_alias_method_copy(call: &ruby_prism::CallNode<'_>, body: &[ModelBodyItem]) -> Option<crate::dialect::MethodDef> {
    fn calls_super(expr: &Expr) -> bool {
        let mut found = matches!(&*expr.node, ExprNode::Super { .. });
        expr.node.for_each_child(&mut |child| found = found || calls_super(child));
        found
    }
    if call.receiver().is_some() || call.block().is_some() || constant_id_str(&call.name()) != "alias_method" {
        return None;
    }
    let args: Vec<_> = call.arguments()?.arguments().iter().collect();
    let [to, from] = args.as_slice() else { return None };
    let (to, from) = (symbol_value(to)?, symbol_value(from)?);
    let source = body.iter().rev().find_map(|item| match item {
        ModelBodyItem::Method { method, .. }
            if method.name.as_str() == from && method.receiver == crate::dialect::MethodReceiver::Instance =>
        {
            Some(method)
        }
        _ => None,
    })?;
    if calls_super(&source.body) {
        return None;
    }
    let mut copy = source.clone();
    copy.name = Symbol::from(to.as_str());
    Some(copy)
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
    class_attr_defaults: Option<&mut IndexMap<Symbol, Expr>>,
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
            // `cattr_*` / `mattr_*` — same class-attr expansion library
            // ingest applies. Plain `attr_*` stays Unknown: concern
            // `included` blocks share this walker, and expanding those
            // into Method items breaks `included_has_accessor` /
            // visibility gating for `private;` inside `included`.
            if matches!(
                name.as_str(),
                "cattr_reader"
                    | "cattr_writer"
                    | "cattr_accessor"
                    | "mattr_reader"
                    | "mattr_writer"
                    | "mattr_accessor"
            ) {
                match expand_mattr_cattr(
                    &call,
                    owner,
                    file,
                    leading_comments.clone(),
                    class_attr_defaults,
                )? {
                    Some(out) => return Ok(out),
                    None => {} // unsupported options — fall through Unknown
                }
            }
        }
    }
    Ok(vec![ingest_model_body_item(stmt, owner, file, leading_comments)?])
}

/// `mattr_*` / `cattr_*` with documented Rails defaults: `default:` and a
/// parameterless block. Other kwargs (`instance_reader:`, splats, …) stay
/// unexpanded so a partial reader cannot drop the initializer.
fn expand_mattr_cattr(
    call: &ruby_prism::CallNode<'_>,
    owner: &ClassId,
    file: &str,
    leading_comments: Vec<Comment>,
    class_attr_defaults: Option<&mut IndexMap<Symbol, Expr>>,
) -> IngestResult<Option<Vec<ModelBodyItem>>> {
    let name = constant_id_str(&call.name());
    let mut names: Vec<Symbol> = Vec::new();
    let mut default: Option<Expr> = None;
    let mut unsupported = false;
    if let Some(args) = call.arguments() {
        for arg in args.arguments().iter() {
            if let Some(s) = symbol_value(&arg) {
                names.push(Symbol::from(s));
                continue;
            }
            if let Some(hash) = arg.as_keyword_hash_node() {
                for element in hash.elements().iter() {
                    let Some(assoc) = element.as_assoc_node() else {
                        unsupported = true;
                        break;
                    };
                    match symbol_value(&assoc.key()).as_deref() {
                        Some("default") if default.is_none() => {
                            // Strict top-level + nested survey recovery:
                            // `ingest_expr_strict` still recurses through
                            // `ingest_expr`, which can substitute nil under
                            // survey — reject if the collector grew.
                            let before = super::survey::recorded().len();
                            match ingest_expr_strict(&assoc.value(), file) {
                                Ok(expr)
                                    if !super::survey::is_active()
                                        || super::survey::recorded().len() == before =>
                                {
                                    default = Some(expr);
                                }
                                Ok(_) => unsupported = true,
                                Err(err) => {
                                    if super::survey::is_active() {
                                        super::survey::record(&err);
                                    }
                                    unsupported = true;
                                }
                            }
                        }
                        _ => unsupported = true,
                    }
                }
                continue;
            }
            unsupported = true;
        }
    }
    if let Some(block) = call.block() {
        let Some(block_node) = block.as_block_node() else {
            return Ok(None);
        };
        if block_node.parameters().is_some() || default.is_some() {
            return Ok(None);
        }
        let Some(body) = block_node.body() else {
            return Ok(None);
        };
        let before = super::survey::recorded().len();
        match ingest_expr_strict(&body, file) {
            Ok(expr)
                if !super::survey::is_active()
                    || super::survey::recorded().len() == before =>
            {
                default = Some(expr);
            }
            Ok(_) => return Ok(None),
            Err(err) => {
                if super::survey::is_active() {
                    super::survey::record(&err);
                }
                return Ok(None);
            }
        }
    }
    if unsupported || names.is_empty() {
        return Ok(None);
    }
    // A non-nil / block default needs a home on the model; concern
    // collectors that cannot carry class-variable seeds leave the
    // declaration unexpanded. Plain mattr/cattr still expand accessors.
    if default.is_some() && class_attr_defaults.is_none() {
        return Ok(None);
    }
    if let Some(defaults) = class_attr_defaults {
        // Rails `class_variable_set`: nil when no default, else the value.
        // `@@` storage requires an explicit seed (unlike `@ivar`).
        let expr = default.unwrap_or_else(|| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: Literal::Nil,
                },
            )
        });
        for attr in &names {
            defaults.insert(attr.clone(), expr.clone());
        }
    }
    let want_reader = name.ends_with("_reader") || name.ends_with("_accessor");
    let want_writer = name.ends_with("_writer") || name.ends_with("_accessor");
    let mut out = Vec::new();
    for (i, attr) in names.iter().enumerate() {
        let lead = if i == 0 {
            leading_comments.clone()
        } else {
            Vec::new()
        };
        // Class side owns `@@attr` (shared across the hierarchy). Instance
        // side delegates to `self.class`, matching Rails' mattr/cattr
        // copies without placing a per-instance ivar.
        let mut first_method = true;
        let mut push = |method: crate::dialect::MethodDef| {
            out.push(ModelBodyItem::Method {
                method,
                leading_comments: if first_method {
                    lead.clone()
                } else {
                    Vec::new()
                },
                leading_blank_line: false,
            });
            first_method = false;
        };
        if want_reader {
            push(super::library_class::synth_mattr_reader(
                owner,
                attr,
                crate::dialect::MethodReceiver::Class,
            ));
            push(synth_mattr_instance_reader(owner, attr));
        }
        if want_writer {
            push(super::library_class::synth_mattr_writer(
                owner,
                attr,
                crate::dialect::MethodReceiver::Class,
            ));
            push(synth_mattr_instance_writer(owner, attr));
        }
    }
    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

fn synth_mattr_instance_reader(owner: &ClassId, name: &Symbol) -> crate::dialect::MethodDef {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility};
    MethodDef {
        name_span: Span::synthetic(),
        name: name.clone(),
        receiver: MethodReceiver::Instance,
        visibility: MethodVisibility::Public,
        params: vec![],
        unsupported_formals: None,
        has_anonymous_block: false,
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                        method: Symbol::from("class"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                )),
                method: name.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn synth_mattr_instance_writer(owner: &ClassId, name: &Symbol) -> crate::dialect::MethodDef {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility, Param};
    use crate::ident::VarId;
    let value = Symbol::from("value");
    MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from(format!("{}=", name.as_str())),
        receiver: MethodReceiver::Instance,
        visibility: MethodVisibility::Public,
        params: vec![Param::positional(value.clone())],
        unsupported_formals: None,
        has_anonymous_block: false,
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                        method: Symbol::from("class"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                )),
                method: Symbol::from(format!("{}=", name.as_str())),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Var {
                        id: VarId(0),
                        name: value,
                    },
                )],
                block: None,
                parenthesized: false,
            },
        ),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeWriter,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
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
        // Bare symbols, plus `if:`/`unless:` naming a predicate (`validate
        // :no_overlap, if: :validate_overlap?` was dropped, so the check
        // never ran). `validate :x, on: :update` runs on one persistence
        // context, and running it unconditionally would reject records
        // Rails accepts: that keeps today's behaviour (the call falls
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
            // `if: :pred` / `unless: :pred` (Symbol conditions only) ride
            // along on the rule; any other option — `on:`, a lambda —
            // keeps the call out, as before.
            let mut if_method: Option<Symbol> = None;
            let mut unless_method: Option<Symbol> = None;
            let mut options_ok = true;
            let mut option_args = 0usize;
            if let Some(args) = call.arguments() {
                for a in args.arguments().iter() {
                    let Some(kw) = a.as_keyword_hash_node() else { continue };
                    option_args += 1;
                    for el in kw.elements().iter() {
                        let Some(assoc) = el.as_assoc_node() else {
                            options_ok = false;
                            continue;
                        };
                        let key = symbol_value(&assoc.key());
                        let value = symbol_value(&assoc.value()).map(|v| Symbol::from(v.as_str()));
                        match (key.as_deref(), value) {
                            (Some("if"), Some(v)) => if_method = Some(v),
                            (Some("unless"), Some(v)) => unless_method = Some(v),
                            _ => options_ok = false,
                        }
                    }
                }
            }
            if options_ok && !symbols.is_empty() && symbols.len() + option_args == arg_count {
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
                            .map(|m| crate::dialect::ValidationRule::Custom {
                                method: m,
                                if_method: if_method.clone(),
                                unless_method: unless_method.clone(),
                            })
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
        if let Some(callback) = parse_callback(&call, &method, file) {
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
    /// Label → stored value, in declaration order. `None` when some
    /// stored value is only known at runtime (`Status::ARCHIVED` from
    /// another file): the scopes and predicates still carry it as an
    /// expression, but the label → literal table other passes rewrite
    /// hand-written sites with (`where(status: :archived)`) must be
    /// complete or absent, so a partial one is never recorded.
    pub mapping: Option<Vec<(String, Literal)>>,
    pub default: Option<Literal>,
    pub items: Vec<ModelBodyItem>,
}

/// Table payload of [`EnumExpansion`] without the generated items.
/// Captured from a concern `included do` so the splice can fold the
/// mapping into each includer's `enums` / `enum_defaults`.
#[derive(Clone, Debug)]
pub struct ConcernEnumDecl {
    pub column: Symbol,
    pub mapping: Vec<(String, Literal)>,
    pub default: Option<Literal>,
}

/// One class-body DSL expansion — `delegated_type` then `enum`. Both
/// ingest walks call [`expand_class_body_dsl`] instead of growing a
/// third parallel `match`.
pub(super) enum ClassBodyExpansion {
    DelegatedType(Vec<ModelBodyItem>),
    Enum(EnumExpansion),
}

pub(super) fn expand_class_body_dsl(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    leading_comments: &[Comment],
    class_consts: &ClassConsts,
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> IngestResult<Option<ClassBodyExpansion>> {
    match super::delegated_type::expand_delegated_type_decl(
        call,
        file,
        leading_comments,
        resolve_constant,
    )? {
        Some(items) => return Ok(Some(ClassBodyExpansion::DelegatedType(items))),
        None => {}
    }
    match expand_enum_decl(call, file, leading_comments, class_consts, resolve_constant)? {
        Some(exp) => Ok(Some(ClassBodyExpansion::Enum(exp))),
        None => Ok(None),
    }
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
    /// Rails `scopes:` / `instance_methods:` (default true). Named
    /// fields so a new option cannot vanish the way `default:` did.
    scopes: bool,
    instance_methods: bool,
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
    let (column, mapping_node, prefix, suffix, default_label, scopes, instance_methods) =
        match symbol_value(&first) {
            Some(col) => {
                let column: String = col;
                let mapping = iter.next();
                let opts = iter.next();
                match opts.as_ref().and_then(|o| o.as_keyword_hash_node()) {
                    Some(kh) => {
                        let (p, s) = enum_affixes(&kh.elements(), &column);
                        (
                            column,
                            mapping,
                            p,
                            s,
                            enum_default_label(&kh.elements()),
                            enum_bool_option(&kh.elements(), "scopes", true),
                            enum_bool_option(&kh.elements(), "instance_methods", true),
                        )
                    }
                    None => (column, mapping, String::new(), String::new(), None, true, true),
                }
            }
            None => {
                let kh = first.as_keyword_hash_node()?;
                let elements = kh.elements();
                let pair = elements.iter().next()?.as_assoc_node()?;
                let column = symbol_value(&pair.key())?;
                let (prefix, suffix) = enum_affixes(&elements, &column);
                (
                    column,
                    Some(pair.value()),
                    prefix,
                    suffix,
                    enum_default_label(&elements),
                    enum_bool_option(&elements, "scopes", true),
                    enum_bool_option(&elements, "instance_methods", true),
                )
            }
        };
    Some(EnumDeclaration {
        column,
        mapping: mapping_node?,
        prefix,
        suffix,
        default_label,
        scopes,
        instance_methods,
    })
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
    class_consts: &ClassConsts,
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> IngestResult<Option<EnumExpansion>> {
    use crate::dialect::{MethodDef, MethodReceiver, Scope};
    use crate::effect::EffectSet;

    let Some(EnumDeclaration {
        column,
        mapping: mapping_node,
        prefix,
        suffix,
        default_label,
        scopes,
        instance_methods,
    }) = enum_declaration(call)
    else {
        return Ok(None)
    };
    // `enum :status, STATUSES` — the mapping named by a constant the class
    // body assigned above (`STATUSES = %i[…].freeze`).
    let labels = if let Some(receiver) = serialized_enum_receiver(&mapping_node) {
        resolve_constant(&receiver).map(|mapping| mapping.into_iter().map(|(label, value)| (label, EnumStored::Lit(value))).collect())
    } else { match mapping_node.as_constant_read_node() {
        Some(cr) => class_consts.mappings.get(constant_id_str(&cr.name())).cloned(),
        None => enum_label_values(&mapping_node, file, class_consts, resolve_constant)?,
    }}
    .ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "enum :{} mapping must be an array or hash literal (or `%w[…].index_by(&:itself)`)",
            column
        ),
    })?;
    let all_labels = labels.clone();
    let default = default_label.and_then(|d| labels.iter().find(|(l, _)| *l == d).and_then(|(_, v)| {
        match v { EnumStored::Lit(value) => Some(value.clone()), EnumStored::Expr(_) => None }
    }));
    let literal_mapping = all_labels.iter().map(|(label, stored)| match stored {
        EnumStored::Lit(value) => Some((label.clone(), value.clone())),
        EnumStored::Expr(_) => None,
    }).collect::<Option<Vec<_>>>();
    let reads_label = literal_mapping.as_ref().is_some_and(|m| crate::dialect::enum_mapping_reads_label(m));
    // A label that is not a Ruby identifier (`32bits`, `64bits`) has no
    // predicate, scope or bang writer Ruby could name: Rails reaches them
    // through `send`, which the emit has no equivalent of. Skipped.
    let labels: Vec<(String, EnumStored)> = labels
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
    for (label, stored) in labels.iter().cloned() {
        let base = format!("{prefix}{label}{suffix}");
        let stored_expr = || match &stored {
            EnumStored::Lit(value) => Expr::new(span, ExprNode::Lit { value: value.clone() }),
            EnumStored::Expr(e) => e.clone(),
        };
        let pair = Expr::new(
            span,
            ExprNode::Hash {
                entries: vec![(sym(&column), stored_expr())],
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
        let method_def = |name: String, body: Expr, comments: Vec<crate::dialect::Comment>| {
            ModelBodyItem::Method {
                method: MethodDef {
                    name_span: crate::span::Span::synthetic(),
                    name: Symbol::from(name),
                    receiver: MethodReceiver::Instance,
                    visibility: crate::dialect::MethodVisibility::Public,
                    params: Vec::new(),
                    unsupported_formals: None,
                    has_anonymous_block: false,
                    block_param: None,
                    body,
                    signature: None,
                    effects: EffectSet::pure(),
                    enclosing_class: None,
                    kind: crate::dialect::AccessorKind::Method,
                    is_async: false,
                    mutates_self: false,
                },
                leading_comments: comments,
                leading_blank_line: false,
            }
        };

        if scopes {
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
        }
        if instance_methods {
            // Aliases share their stored value and therefore their
            // canonical reader label. Every alias predicate is true
            // for that value, even though the reader names only the first.
            let predicate_label = literal_mapping.as_ref().and_then(|mapping| {
                let EnumStored::Lit(value) = &stored else { return None };
                mapping.iter().find(|(_, v)| v == value).map(|(label, _)| label.clone())
            }).unwrap_or_else(|| label.clone());
            items.push(method_def(
                format!("{base}?"),
                Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(column_read()),
                        method: Symbol::from("=="),
                        args: vec![if reads_label {
                            Expr::new(span, ExprNode::Lit { value: Literal::Str { value: predicate_label } })
                        } else { stored_expr() }],
                        block: None,
                        parenthesized: false,
                    },
                ),
                if items.is_empty() {
                    leading_comments.to_vec()
                } else {
                    Vec::new()
                },
            ));
            items.push(method_def(
                format!("{base}!"),
                call_with_pair("update!"),
                Vec::new(),
            ));
        }
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
                        match value { EnumStored::Lit(value) => Expr::new(span, ExprNode::Lit { value: value.clone() }), EnumStored::Expr(expr) => expr.clone() },
                    )
                })
                .collect(),
            kwargs: false,
        },
    );
    let mapping_comments = if items.is_empty() {
        leading_comments.to_vec()
    } else {
        Vec::new()
    };
    items.push(ModelBodyItem::Method {
        method: MethodDef {
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from(crate::naming::pluralize_snake(&column)),
            receiver: MethodReceiver::Class,
            visibility: crate::dialect::MethodVisibility::Public,
            params: Vec::new(),
            unsupported_formals: None,
            has_anonymous_block: false,
            block_param: None,
            body: mapping_hash,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: crate::dialect::AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        },
        leading_comments: mapping_comments,
        leading_blank_line: false,
    });
    let mapping = all_labels
        .iter()
        .map(|(l, v)| match v {
            EnumStored::Lit(lit) => Some((l.clone(), lit.clone())),
            EnumStored::Expr(_) => None,
        })
        .collect::<Option<Vec<_>>>();
    let writes = mapping_constant_writes(&mapping_node, file)?;
    // The declaration's comments ride the first item, so with constant
    // writes ahead of the generated items they move to the first write.
    let mut doc_comments = if writes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(items[0].leading_comments_mut())
    };
    items.splice(
        0..0,
        writes.into_iter().map(|expr| ModelBodyItem::Unknown {
            expr,
            leading_comments: std::mem::take(&mut doc_comments),
            leading_blank_line: false,
        }),
    );
    Ok(Some(EnumExpansion { column: Symbol::from(column.as_str()), mapping, default, items }))
}

/// The stored value of one enum label. A literal is what Rails' own
/// table holds and what other passes rewrite hand-written sites with; an
/// expression (`Status::ARCHIVED`, `Eligibility::Paused.serialize`)
/// is a value only the running program knows. The generated scope and
/// predicate use it either way — they compare against the column, not
/// against a constant folded at ingest.
#[derive(Clone)]
enum EnumStored {
    Lit(Literal),
    Expr(Expr),
}

/// Constants a class body assigns that an `enum` declaration below them
/// may name: the mapping itself (`enum :status, STATUSES`), a label
/// (`NAME => NAME`, `"#{PENDING}": 1`) or a stored value
/// (`active: ACTIVE`). Collected in source order so an alias
/// (`ROLE_B = ROLE_A`) resolves through the constant it names.
#[derive(Default)]
pub(super) struct ClassConsts {
    /// `NAME = "x"`, `NAME = :x`, `NAME = 3`, `NAME = OTHER`.
    scalars: std::collections::HashMap<String, Literal>,
    /// `NAME = %i[…]` / `{ … }`, as the mapping an enum would build.
    mappings: std::collections::HashMap<String, Vec<(String, EnumStored)>>,
    /// `NAME = %i[…]` / `%w[…]` — the bare labels, for the idioms that
    /// derive a mapping from them (`NAME.index_by(&:to_s)`).
    labels: std::collections::HashMap<String, Vec<String>>,
}

impl ClassConsts {
    pub(super) fn collect(stmts: &[Node<'_>], file: &str) -> Self {
        let mut out = ClassConsts::default();
        for stmt in stmts {
            let Some(cw) = stmt.as_constant_write_node() else { continue };
            let name = constant_id_str(&cw.name()).to_string();
            let value = cw.value();
            if let Some(lit) = out.scalar(&value) {
                out.scalars.insert(name, lit);
                continue;
            }
            if let Some(labels) = label_list(&value) {
                out.labels.insert(name.clone(), labels);
            }
            // A failure to ingest a value is not this pass's to report:
            // the constant is simply not usable as a mapping, and the
            // enum that names it says so.
            if let Ok(Some(mapping)) = enum_label_values(&value, file, &out, &|_| None) {
                out.mappings.insert(name, mapping);
            }
        }
        out
    }

    /// A scalar the class body can fold: a string, a symbol (stored as
    /// its string, which is what a string column holds), an integer,
    /// or a constant already folded. `.freeze` is transparent.
    fn scalar(&self, node: &Node<'_>) -> Option<Literal> {
        if let Some(call) = node.as_call_node() {
            if constant_id_str(&call.name()) == "freeze" && call.arguments().is_none() {
                return self.scalar(&call.receiver()?);
            }
            return None;
        }
        if let Some(s) = string_value(node).or_else(|| symbol_value(node)) {
            return Some(Literal::Str { value: s });
        }
        if let Some(i) = node.as_integer_node() {
            return Some(Literal::Int { value: super::util::integer_i64(&i.value())? });
        }
        if let Some(cr) = node.as_constant_read_node() {
            return self.scalars.get(constant_id_str(&cr.name())).cloned();
        }
        None
    }

    /// A label spelled as a symbol, a string, a folded constant, or an
    /// interpolation of folded constants (`"#{UNSET_STATUS}": 0`).
    fn label(&self, node: &Node<'_>) -> Option<String> {
        if let Some(Literal::Str { value }) = self.scalar(node) {
            return Some(value);
        }
        let parts: Vec<Node<'_>> = if let Some(d) = node.as_interpolated_symbol_node() {
            d.parts().iter().collect()
        } else if let Some(d) = node.as_interpolated_string_node() {
            d.parts().iter().collect()
        } else {
            return None;
        };
        let mut out = String::new();
        for part in parts {
            if let Some(s) = string_value(&part) {
                out.push_str(&s);
            } else if let Some(e) = part.as_embedded_statements_node() {
                let body: Vec<Node<'_>> = e.statements()?.body().iter().collect();
                let [only] = body.as_slice() else { return None };
                match self.scalar(only)? {
                    Literal::Str { value } => out.push_str(&value),
                    Literal::Int { value } => out.push_str(&value.to_string()),
                    _ => return None,
                }
            } else {
                return None;
            }
        }
        Some(out)
    }
}

/// The bare labels of a `%i[…]` / `%w[…]` array (optionally `.freeze`d).
fn label_list(node: &Node<'_>) -> Option<Vec<String>> {
    if let Some(call) = node.as_call_node() {
        if constant_id_str(&call.name()) == "freeze" && call.arguments().is_none() {
            return label_list(&call.receiver()?);
        }
    }
    node.as_array_node()?
        .elements()
        .iter()
        .map(|el| symbol_value(&el).or_else(|| string_value(&el)))
        .collect()
}

/// Label → stored value for an `enum` mapping. An array literal maps by
/// index the way Rails does (`%i[active deactivated]` → 0, 1); a hash
/// literal carries its own values; `%w[…].index_by(&:itself)` — the
/// idiom for a string-backed column — maps each label to itself.
///
/// A hash label may be a symbol, a string, or a class-body constant
/// folded through `consts`; a stored value that is not a foldable scalar
/// is kept as an expression. `Ok(None)` for anything whose LABELS are not
/// known at ingest (a computed hash, a constant from another file),
/// which the caller reports as a gap rather than guessing at names.
fn enum_label_values(
    node: &Node<'_>,
    file: &str,
    consts: &ClassConsts,
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> IngestResult<Option<Vec<(String, EnumStored)>>> {
    if node.as_constant_path_node().is_some() {
        return Ok(resolve_constant(node).map(|pairs| pairs.into_iter().map(|(label, value)| (label, EnumStored::Lit(value))).collect()));
    }

    // `%i[…].freeze` / `{ … }.freeze` — the literal is the receiver.
    if let Some(call) = node.as_call_node() {
        if constant_id_str(&call.name()) == "freeze" && call.arguments().is_none() && call.block().is_none() {
            if let Some(recv) = call.receiver() {
                return enum_label_values(&recv, file, consts, resolve_constant);
            }
        }
    }
    if let Some(arr) = node.as_array_node() {
        let Some(labels) = label_list(node) else { return Ok(None) };
        let _ = arr;
        return Ok(Some(
            labels
                .into_iter()
                .enumerate()
                .map(|(i, label)| (label, EnumStored::Lit(Literal::Int { value: i as i64 })))
                .collect(),
        ));
    }
    if let Some(hash) = node.as_hash_node() {
        return enum_label_pairs(hash.elements().iter(), file, consts);
    }
    // `enum :status, processing: 'processing', ready: 'ready'` — Rails 7's
    // dominant spelling. A trailing bare-hash argument (no braces) parses
    // as a `KeywordHashNode`, not a `HashNode`; its elements are the same
    // `AssocNode`s either way, so the extraction logic is shared.
    if let Some(kwhash) = node.as_keyword_hash_node() {
        return enum_label_pairs(kwhash.elements().iter(), file, consts);
    }
    let Some(call) = node.as_call_node() else { return Ok(None) };
    let Some(recv) = call.receiver() else { return Ok(None) };
    // The labels a derived mapping starts from: a literal array, or a
    // class-body constant naming one.
    let labels = || match recv.as_constant_read_node() {
        Some(cr) => consts.labels.get(constant_id_str(&cr.name())).cloned(),
        None => label_list(&recv).or_else(|| resolve_constant(&recv).map(|pairs| pairs.into_iter().map(|(label, _)| label).collect())),
    };
    let identity = |labels: Vec<String>| {
        labels
            .into_iter()
            .map(|l| (l.clone(), EnumStored::Lit(Literal::Str { value: l })))
            .collect::<Vec<_>>()
    };
    match constant_id_str(&call.name()) {
        // `%w[ invisible nothing ].index_by(&:itself)`,
        // `STATUSES.index_with(&:itself)` — the labels ARE the stored
        // strings. (`&:to_s`/`&:to_sym` on symbols or strings name the
        // same label.)
        "index_by" | "index_with" if is_identity_proc(&call) => {
            Ok(labels().map(identity))
        }
        // `STATUSES.to_h { |status| [status, status.to_s] }`
        "to_h" if is_identity_pair_block(&call) => Ok(labels().map(identity)),
        // The same mapping written as `STATUSES.map { |s| [s, s.to_s] }.to_h`.
        "to_h" if call.arguments().is_none() && call.block().is_none() => {
            let Some(map) = recv.as_call_node() else { return Ok(None) };
            if constant_id_str(&map.name()) != "map" || !is_identity_pair_block(&map) {
                return Ok(None);
            }
            let Some(source) = map.receiver() else { return Ok(None) };
            let labels = match source.as_constant_read_node() {
                Some(cr) => consts.labels.get(constant_id_str(&cr.name())).cloned(),
                None => label_list(&source).or_else(|| resolve_constant(&source).map(|pairs| pairs.into_iter().map(|(label, _)| label).collect())),
            };
            Ok(labels.map(identity))
        }
        _ => Ok(None),
    }
}

/// `&:itself` / `&:to_s` / `&:to_sym` as the only argument: the block maps
/// a label to itself. (`["draft", "active"].index_by(&:to_sym)` keys by
/// symbol and stores the string; the label text and the stored value are
/// the same either way.)
fn is_identity_proc(call: &ruby_prism::CallNode<'_>) -> bool {
    let Some(block) = call.block().and_then(|b| b.as_block_argument_node()) else {
        return false;
    };
    block
        .expression()
        .and_then(|e| symbol_value(&e))
        .is_some_and(|s| s == "itself" || s == "to_s" || s == "to_sym")
}

/// `{ |x| [x, x.to_s] }` / `{ |x| [x, x] }`: a `to_h` block pairing each
/// label with its own string.
fn is_identity_pair_block(call: &ruby_prism::CallNode<'_>) -> bool {
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else { return false };
    let Some(params) = block.parameters().and_then(|p| p.as_block_parameters_node()) else {
        return false;
    };
    let Some(pn) = params.parameters() else { return false };
    let reqs: Vec<_> = pn.requireds().iter().collect();
    let [only] = reqs.as_slice() else { return false };
    let Some(rp) = only.as_required_parameter_node() else { return false };
    let param = constant_id_str(&rp.name()).to_string();
    let Some(body) = block.body() else { return false };
    let stmts: Vec<Node<'_>> = flatten_statements(body);
    let [stmt] = stmts.as_slice() else { return false };
    let Some(arr) = stmt.as_array_node() else { return false };
    let els: Vec<Node<'_>> = arr.elements().iter().collect();
    let [k, v] = els.as_slice() else { return false };
    let is_param = |n: &Node<'_>| {
        n.as_local_variable_read_node().is_some_and(|l| constant_id_str(&l.name()) == param)
    };
    is_param(k)
        && (is_param(v)
            || v.as_call_node().is_some_and(|c| {
                constant_id_str(&c.name()) == "to_s"
                    && c.arguments().is_none()
                    && c.receiver().is_some_and(|r| is_param(&r))
            }))
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

/// Shared `label => value` extraction for both a braced `HashNode` and a
/// bare trailing `KeywordHashNode` — same element shape (`AssocNode`s),
/// different Prism wrapper type depending on whether the source wrote
/// `{ … }` or left the braces off.
fn enum_label_pairs<'a>(
    elements: impl Iterator<Item = Node<'a>>,
    file: &str,
    consts: &ClassConsts,
) -> IngestResult<Option<Vec<(String, EnumStored)>>> {
    let mut out = Vec::new();
    for el in elements {
        let Some(assoc) = el.as_assoc_node() else { return Ok(None) };
        let Some(label) = consts.label(&assoc.key()) else { return Ok(None) };
        let value = assoc.value();
        // `pending: PENDING = "pending"` stores the written value and
        // defines the constant, which `mapping_constant_writes` hoists
        // into the class body. A literal value is stored as that literal;
        // only a non-literal one makes the generated methods read the
        // constant.
        let stored = match value.as_constant_write_node() {
            Some(cw) => match consts.scalar(&cw.value()) {
                Some(lit) => EnumStored::Lit(lit),
                None => EnumStored::Expr(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from(constant_id_str(&cw.name()))] },
                )),
            },
            None => match consts.scalar(&value) {
                Some(lit) => EnumStored::Lit(lit),
                None => EnumStored::Expr(ingest_expr(&value, file)?),
            },
        };
        out.push((label, stored));
    }
    Ok(Some(out))
}

/// The constants an enum's hash mapping assigns as it is built
/// (`pending: PENDING = "pending"`), as class-body writes. Ruby runs
/// them when the class body evaluates the `enum` call's arguments; left
/// inside the mapping they would be copied into the generated methods,
/// where a constant assignment does not parse.
fn mapping_constant_writes(node: &Node<'_>, file: &str) -> IngestResult<Vec<Expr>> {
    if let Some(call) = node.as_call_node() {
        if constant_id_str(&call.name()) == "freeze" && call.arguments().is_none() && call.block().is_none() {
            if let Some(recv) = call.receiver() {
                return mapping_constant_writes(&recv, file);
            }
        }
        return Ok(Vec::new());
    }
    let elements: Vec<Node<'_>> = if let Some(hash) = node.as_hash_node() {
        hash.elements().iter().collect()
    } else if let Some(kwhash) = node.as_keyword_hash_node() {
        kwhash.elements().iter().collect()
    } else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for el in elements {
        let Some(assoc) = el.as_assoc_node() else { continue };
        let value = assoc.value();
        if value.as_constant_write_node().is_some() {
            out.push(ingest_expr(&value, file)?);
        }
    }
    Ok(out)
}

/// Rails `scopes:` / `instance_methods:` — only the unprefixed
/// spellings; `_scopes` / `_instance_methods` are rejected by Rails 7+.
fn enum_bool_option(elements: &ruby_prism::NodeList<'_>, name: &str, default: bool) -> bool {
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else { continue };
        let Some(key) = symbol_value(&assoc.key()) else { continue };
        // `_scopes:` / `_instance_methods:` are the pre-Rails-7 spellings.
        if key.trim_start_matches('_') != name {
            continue;
        }
        let value = assoc.value();
        if value.as_false_node().is_some() {
            return false;
        }
        if value.as_true_node().is_some() {
            return true;
        }
    }
    default
}

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

/// Expand the singleton body while retaining the enclosing visibility decisions.
fn ingest_singleton_class_body(
    sc: &ruby_prism::SingletonClassNode<'_>,
    owner: &ClassId,
    file: &str,
    visibility: &Visibility,
) -> IngestResult<super::singleton_class::SingletonBody> {
    super::singleton_class::ingest_singleton_body(sc, owner, file, &|def| {
        let mut method = ingest_method(def, file)?;
        visibility.apply(&def.as_node(), &mut method);
        Ok(method)
    })
}

pub(super) fn ingest_method(
    def: &ruby_prism::DefNode<'_>,
    file: &str,
) -> IngestResult<crate::dialect::MethodDef> {
    use crate::dialect::{MethodDef, MethodReceiver};

    let formals = super::forwarding::parse(def);
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
            } else if let Some(name) = &formals.anonymous_rest_name {
                // An unforwarded `*`: see `forwarding`'s module doc.
                params.push(crate::dialect::Param::rest(name.clone()));
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
        //
        // Beside an earlier keyword (`def notification(badge: …,
        // **params)` — campfire preview) flattening to `params = {}`
        // also does not parse: optional positionals cannot follow
        // keywords. Keep a real keyword-rest there too (same rule
        // `library_class` applies when `keeps_keywords`).
        if let Some(krest) = pn.keyword_rest() {
            if let Some(krp) = krest.as_keyword_rest_parameter_node() {
                if let Some(loc) = krp.name() {
                    let name = Symbol::from(constant_id_str(&loc));
                    let has_keywords = params.iter().any(|p| p.keyword || p.from_keyword);
                    // Match `library_class`: `from_kwrest` marks the
                    // *flattened* `params = {}` form only. A real
                    // keyword-rest kept beside keywords / `*rest` must
                    // not carry the marker — forwarding analysis treats
                    // `from_kwrest` as "flattened keyword ABI" and would
                    // reject valid `**` / full-arg forwards into it.
                    let p = if params.iter().any(|p| p.rest) || has_keywords {
                        let mut p = crate::dialect::Param::keyword(name, None);
                        p.rest = true;
                        p
                    } else {
                        let mut p = crate::dialect::Param::with_default(
                            name,
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Hash { entries: vec![], kwargs: false },
                            ),
                        );
                        p.from_kwrest = true;
                        p
                    };
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

    // Only full `...` or nameless `**` enters this canonical seam.
    // Named rest/keyword-rest above and the separate block slot stay
    // source-owned; no forwarding packet is expanded into local names.
    params.extend(formals.anonymous.map(super::forwarding::AnonymousFormal::into_param));

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
        unsupported_formals: formals.unsupported,
        has_anonymous_block: formals.has_anonymous_block,
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
    file: &str,
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
    let mut if_cond: Option<Expr> = None;
    let mut unless_cond: Option<Expr> = None;
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
                    // `if:` / `unless:` — a zero-arity lambda/proc body
                    // (Rails `instance_exec`s it with `self` the record)
                    // or a Symbol naming a predicate method. Lowered to a
                    // guard around the callback body, so the callback runs
                    // in exactly Rails' circumstances. Dropping the
                    // declaration instead would run it in no circumstance
                    // at all, which is the wrong answer too.
                    "if" => if_cond = Some(callback_condition(&assoc.value(), file)?),
                    "unless" => unless_cond = Some(callback_condition(&assoc.value(), file)?),
                    // `prepend:` and anything else: not modeled — reject
                    // rather than run the callback in the wrong place.
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

    let condition = match (if_cond, unless_cond) {
        (None, None) => None,
        (Some(c), None) => Some(c),
        (None, Some(c)) => Some(negate_condition(c)),
        (Some(a), Some(b)) => Some(and_condition(a, negate_condition(b))),
    };

    Some(Callback { hook, targets, on, condition })
}

/// A callback's `if:`/`unless:` value → the condition expression that
/// guards the callback body. A zero-arity lambda/proc contributes its
/// body (Rails `instance_exec`s it with `self` the record, which is what
/// a spliced body sees). A Symbol is the predicate method, called on the
/// record. Anything else (a lambda with parameters, an array of
/// conditions) is unmodeled and declines.
fn callback_condition(value: &ruby_prism::Node<'_>, file: &str) -> Option<Expr> {
    if let Some(lambda) = value.as_lambda_node() {
        let body = simple_condition_body(lambda.parameters(), lambda.body())?;
        return ingest_expr(&body, file).ok();
    }
    if let Some(call) = value.as_call_node() {
        if call.receiver().is_none() {
            let name = constant_id_str(&call.name());
            if name == "proc" || name == "lambda" {
                let block = call.block()?.as_block_node()?;
                let body = simple_condition_body(block.parameters(), block.body())?;
                return ingest_expr(&body, file).ok();
            }
        }
    }
    let sym = symbol_value(value)?;
    Some(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(sym.as_str()),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    ))
}

/// The one expression a lambda/proc callback condition may splice into the
/// hook as its guard. The guard runs inline in the callback, with `self`
/// as the record and no frame of its own, so only a body that is the same
/// expression there is accepted:
///
/// * no parameters (`-> {}`, `->() {}`, `proc { }`, `proc { || }`) — a
///   `->(r) { r.title… }` would splice `r` unbound (numbered params and
///   `it` are parameters too);
/// * exactly one statement, with no `return`/`next`/`break`/`redo`/
///   `retry` — inside the hook a `return` exits the whole callback chain;
/// * no local writes — they would leak into the hook's scope. That
///   includes a local bound through a target: a multi-write
///   `(a, b = …)`, a `=> t` match-write, a `rescue => e`; and any
///   multi-write declines, its targets being locals or not.
///
/// Anything else declines (`None`), and the callback falls back to the
/// unsupported-DSL warning, as it did before conditions were modelled.
fn simple_condition_body<'pr>(
    params: Option<ruby_prism::Node<'pr>>,
    body: Option<ruby_prism::Node<'pr>>,
) -> Option<ruby_prism::Node<'pr>> {
    if let Some(p) = params {
        let bp = p.as_block_parameters_node()?;
        if bp.parameters().is_some() || bp.locals().iter().next().is_some() {
            return None;
        }
    }
    let stmts = body?.as_statements_node()?;
    let mut it = stmts.body().iter();
    let only = it.next()?;
    if it.next().is_some() {
        return None;
    }

    struct Escapes(bool);
    impl<'pr> ruby_prism::Visit<'pr> for Escapes {
        fn visit_return_node(&mut self, _: &ruby_prism::ReturnNode<'pr>) { self.0 = true; }
        fn visit_next_node(&mut self, _: &ruby_prism::NextNode<'pr>) { self.0 = true; }
        fn visit_break_node(&mut self, _: &ruby_prism::BreakNode<'pr>) { self.0 = true; }
        fn visit_redo_node(&mut self, _: &ruby_prism::RedoNode<'pr>) { self.0 = true; }
        fn visit_retry_node(&mut self, _: &ruby_prism::RetryNode<'pr>) { self.0 = true; }
        fn visit_local_variable_write_node(&mut self, _: &ruby_prism::LocalVariableWriteNode<'pr>) {
            self.0 = true;
        }
        fn visit_local_variable_operator_write_node(
            &mut self,
            _: &ruby_prism::LocalVariableOperatorWriteNode<'pr>,
        ) {
            self.0 = true;
        }
        fn visit_local_variable_or_write_node(&mut self, _: &ruby_prism::LocalVariableOrWriteNode<'pr>) {
            self.0 = true;
        }
        fn visit_local_variable_and_write_node(&mut self, _: &ruby_prism::LocalVariableAndWriteNode<'pr>) {
            self.0 = true;
        }
        fn visit_local_variable_target_node(&mut self, _: &ruby_prism::LocalVariableTargetNode<'pr>) {
            self.0 = true;
        }
        fn visit_multi_write_node(&mut self, _: &ruby_prism::MultiWriteNode<'pr>) {
            self.0 = true;
        }
    }
    let mut v = Escapes(false);
    ruby_prism::Visit::visit(&mut v, &only);
    if v.0 { None } else { Some(only) }
}

fn negate_condition(cond: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(cond),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn and_condition(left: Expr, right: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::Symbol,
            left,
            right,
        },
    )
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

    let mut body = match lambda_body {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };
    // The body becomes a class method, where the lambda's own `next v`
    // (its early return) is `return v`; `next` in a def does not parse.
    // Rails runs the body as `instance_exec(*args, &body) || self`
    // (`ActiveRecord::Relation#_exec_scope`), so a falsy `next` answers
    // the relation: a bare `next`, `next nil` or `next false` returns
    // `all`, and any other value `v` returns `v || all` unless it is a
    // bare relation chain (`all`, `none`, `where(...)`), which is never
    // falsy.
    super::sql_functions::next_to_return_with(&mut body, &scope_next_value);

    Ok(Some(Scope { name, params, body }))
}

/// The value a scope body's `next` returns from the class method it
/// becomes; see `parse_scope`.
fn scope_next_value(value: Option<Expr>, span: Span) -> Expr {
    let all = || {
        Expr::new(
            span,
            ExprNode::Send {
                recv: None,
                method: Symbol::from("all"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };
    let Some(value) = value else { return all() };
    if matches!(
        &*value.node,
        ExprNode::Lit { value: crate::expr::Literal::Nil | crate::expr::Literal::Bool { value: false } }
    ) {
        return all();
    }
    if bare_chain_root(&value) {
        return value;
    }
    Expr::new(
        span,
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: value,
            right: all(),
        },
    )
}

/// `all`, `none`, `where(...).order(...)`: a chain whose root is a
/// receiverless call, which in a scope body is a relation query method.
fn bare_chain_root(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { recv: None, .. } => true,
        ExprNode::Send { recv: Some(r), .. } => bare_chain_root(r),
        _ => false,
    }
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
    let mut autosave: Option<bool> = None;

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
                // `"::Portal"` names the top-level class `Portal`; kept as written it is a class no one defines.
                "class_name" => class_name = string_value(&value).map(|s| s.trim_start_matches("::").to_string()),
                "foreign_key" => {
                    foreign_key = string_value(&value).or_else(|| symbol_value(&value))
                }
                "through" => through = symbol_value(&value),
                "source" => source = symbol_value(&value),
                "source_type" => source_type = string_value(&value).map(|s| s.trim_start_matches("::").to_string()),
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
                "autosave" => autosave = bool_value(&value),
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

    let foreign_key_explicit = foreign_key.is_some();

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
            foreign_key_explicit,
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
            foreign_key_explicit,
            dependent: dependent.unwrap_or_default(),
            as_interface: as_interface.as_deref().map(Symbol::from),
            scope,
            autosave: autosave.unwrap_or(false),
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
            foreign_type: None,
            primary_key: None,
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
                    .filter(|name| bare_table_identifier(name));
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
        Ok(table_name_reader_override(&body))
    } else if collector.writes.len() == 1 && collector.writes[0].is_some() {
        Ok(collector.writes.pop().unwrap())
    } else {
        Err(IngestError::Unsupported {
            file: file.into(),
            message: "table_name binding requires one direct self.table_name assignment to a literal string or symbol naming a safe bare SQL identifier".into(),
        })
    }
}

/// Shared DDL/DML currently emits bare table names. Refuse names
/// needing qualification or SQL quoting.
fn bare_table_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !crate::naming::is_sqlite_keyword(name)
}

/// The other way to name the table: a class-side reader answering one
/// literal, `def self.table_name = "x"` or `class << self; def
/// table_name; "x"; end; end` directly in the body (a
/// `Catalog::Item::Detail`). Rails reads the table through that
/// method, so it binds the schema row. The direct `def self.` form is
/// consumed like the setter (lowering writes `def self.table_name` from
/// `Model::table`); the `class << self` form stays, saying the same name.
/// Any other shape, or more than one reader, keeps the convention.
fn table_name_reader_override(body: &Node<'_>) -> Option<(String, usize)> {
    fn literal_reader(def: &ruby_prism::DefNode<'_>) -> Option<String> {
        if constant_id_str(&def.name()) != "table_name" || def.parameters().is_some() {
            return None;
        }
        let body = def.body()?;
        let value = match body.as_statements_node() {
            Some(stmts) if stmts.body().len() == 1 => stmts.body().iter().next()?,
            Some(_) => return None,
            None => body,
        };
        symbol_or_string_value(&value).filter(|name| bare_table_identifier(name))
    }
    fn statements<'pr>(node: &Node<'pr>) -> Vec<Node<'pr>> {
        match node.as_statements_node() {
            Some(stmts) => stmts.body().iter().collect(),
            None => vec![],
        }
    }
    let mut found = Vec::new();
    for stmt in statements(body) {
        if let Some(def) = stmt.as_def_node() {
            if def.receiver().is_some_and(|r| r.as_self_node().is_some()) {
                if let Some(name) = literal_reader(&def) {
                    found.push((name, stmt.location().start_offset()));
                } else if constant_id_str(&def.name()) == "table_name" {
                    return None;
                }
            }
        } else if let Some(sclass) = stmt.as_singleton_class_node() {
            if sclass.expression().as_self_node().is_none() {
                continue;
            }
            for inner in sclass.body().map(|b| statements(&b)).unwrap_or_default() {
                let Some(def) = inner.as_def_node() else { continue };
                if def.receiver().is_some() {
                    continue;
                }
                if let Some(name) = literal_reader(&def) {
                    // Not a body statement: nothing is consumed.
                    found.push((name, usize::MAX));
                } else if constant_id_str(&def.name()) == "table_name" {
                    return None;
                }
            }
        }
    }
    (found.len() == 1).then(|| found.pop()).flatten()
}

pub(crate) fn dependent_from_sym(s: &str) -> Option<crate::dialect::Dependent> {
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
        ColumnType::Json | ColumnType::Jsonb => Ty::Untyped,
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
             class << self\n    memoize_everything :cache\n  end\nend\n",
        );
        assert!(err.is_err(), "an unmodeled singleton statement is still an error: {err:?}");
    }
}
