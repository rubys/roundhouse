//! Sequel model ingestion — parses one `models/*.rb` declaring
//! `class X < Sequel::Model` into the same [`Model`] the Rails
//! front-end produces, per the mapping table in
//! `docs/roda-sequel-plan.md` (issue #67).
//!
//! Normalization happens here, not in a parallel catalog: Sequel's
//! association macros become `Association` records, the imperative
//! `def validate` body's closed `validates_*` vocabulary becomes
//! declarative `Validation`s, and [`normalize_sequel_expr`] rewrites
//! Sequel dataset spellings (`Article[id]`, `eager`, `with_pk`,
//! `set_fields`, …) into their ActiveRecord equivalents so everything
//! downstream of ingest sees one query dialect. The emitted app runs
//! on roundhouse's own AR-shaped framework runtime, so the Sequel
//! surface would be re-lowered to that vocabulary anyway — converging
//! at ingest keeps the post-lowering IR diffable against the Rails
//! fixture (the point of the exemplar).

use ruby_prism::Node;

use crate::dialect::{Association, Dependent, Model, ModelBodyItem, Validation, ValidationRule};
use crate::expr::{Expr, ExprNode, Literal};
use crate::naming::{camelize, pluralize_snake, singularize_camelize, snake_case};
use crate::schema::{ReferentialAction, Schema};
use crate::span::Span;
use crate::ty::Row;
use crate::{ClassId, Symbol, TableRef};

use super::expr::ingest_expr;
use super::model::{ingest_method, row_from_table};
use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_first_class, flatten_statements,
    integer_value, string_value, symbol_value,
};
use super::{IngestError, IngestResult};

/// Parse a single Sequel model file. Returns `Ok(None)` when the file's
/// first class doesn't subclass `Sequel::Model` (a support class — the
/// caller ingests it through the library-class path instead).
pub fn ingest_sequel_model(
    source: &[u8],
    file: &str,
    schema: &Schema,
) -> IngestResult<Option<Model>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };
    let is_sequel_model = class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .is_some_and(|p| p == ["Sequel", "Model"]);
    if !is_sequel_model {
        return Ok(None);
    }

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "model class name must be a simple constant or path".into(),
    })?;
    let class_name = Symbol::from(name_path.join("::"));
    let owner = ClassId(class_name.clone());
    let table_name = pluralize_snake(class_name.as_str());

    let attributes = schema
        .tables
        .get(&Symbol::from(table_name.as_str()))
        .map(row_from_table)
        .unwrap_or_else(Row::closed);

    let mut body: Vec<ModelBodyItem> = Vec::new();
    if let Some(class_body) = class.body() {
        for stmt in flatten_statements(class_body) {
            match ingest_sequel_model_body_item(&stmt, &owner, &table_name, file, schema) {
                Ok(items) => body.extend(items),
                Err(err) if super::survey::is_active() => super::survey::record(&err),
                Err(err) => return Err(err),
            }
        }
    }

    let class_loc = class.location();
    Ok(Some(Model {
        name: owner,
        // The emitted app runs on the AR-shaped framework runtime;
        // `Sequel::Model` plays the role `ApplicationRecord` does there.
        parent: Some(ClassId(Symbol::from("ApplicationRecord"))),
        table: TableRef(Symbol::from(table_name)),
        attributes,
        body,
        span: Span {
            file: super::sources::file_id(file),
            start: class_loc.start_offset() as u32,
            end: class_loc.end_offset() as u32,
        },
    }))
}

fn ingest_sequel_model_body_item(
    stmt: &Node<'_>,
    owner: &ClassId,
    owner_table: &str,
    file: &str,
    schema: &Schema,
) -> IngestResult<Vec<ModelBodyItem>> {
    let span = Span {
        file: super::sources::file_id(file),
        start: stmt.location().start_offset() as u32,
        end: stmt.location().end_offset() as u32,
    };
    if let Some(def) = stmt.as_def_node() {
        // `def validate` — Sequel's imperative validation hook. Its
        // body is a closed vocabulary of validation_helpers calls;
        // each becomes a declarative `Validation`, converging on the
        // Rails `validates` representation.
        if constant_id_str(&def.name()) == "validate" && def.receiver().is_none() {
            return ingest_validate_body(&def, file, span);
        }
        let mut method = ingest_method(&def, file)?;
        normalize_sequel_expr(&mut method.body);
        return Ok(vec![ModelBodyItem::Method {
            method,
            leading_comments: Vec::new(),
            leading_blank_line: false,
        }]);
    }
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_none() {
            let macro_name = constant_id_str(&call.name()).to_string();
            if let Some(assoc) =
                parse_sequel_association(&call, owner, owner_table, &macro_name, file, schema)?
            {
                return Ok(vec![ModelBodyItem::Association {
                    assoc,
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                    span,
                }]);
            }
        }
    }
    // Everything else round-trips verbatim, mirroring the Rails model
    // walk's Unknown fallback.
    Ok(vec![ModelBodyItem::Unknown {
        expr: ingest_expr(stmt, file)?,
        leading_comments: Vec::new(),
        leading_blank_line: false,
    }])
}

/// `one_to_many` / `many_to_one` / `one_to_one` / `many_to_many` →
/// `Association`. Returns `Ok(None)` for non-association macros (the
/// caller falls through to Unknown).
fn parse_sequel_association(
    call: &ruby_prism::CallNode<'_>,
    owner: &ClassId,
    owner_table: &str,
    macro_name: &str,
    file: &str,
    schema: &Schema,
) -> IngestResult<Option<Association>> {
    if !matches!(macro_name, "one_to_many" | "many_to_one" | "one_to_one" | "many_to_many") {
        return Ok(None);
    }
    let Some(args) = call.arguments() else { return Ok(None) };
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let Some(first) = iter.next() else { return Ok(None) };
    let Some(name_str) = symbol_value(&first) else { return Ok(None) };
    let name = Symbol::from(name_str.as_str());

    let mut key: Option<String> = None;
    let mut class_name: Option<String> = None;
    let mut join_table: Option<String> = None;
    let mut order_expr: Option<Expr> = None;
    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(k) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match k.as_str() {
                "key" => key = symbol_value(&value),
                "class" => {
                    class_name = string_value(&value)
                        .or_else(|| constant_path_of(&value).map(|p| p.join("::")))
                }
                "join_table" => join_table = symbol_value(&value),
                "order" => order_expr = Some(sequel_order_to_scope(&value, file)?),
                other => {
                    let err = IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "sequel association option not recognized: {macro_name} :{name_str}, {other}:"
                        ),
                    };
                    if !super::survey::is_active() {
                        return Err(err);
                    }
                    super::survey::record(&err);
                }
            }
        }
    }

    let owner_snake = snake_case(owner.0.as_str());
    Ok(Some(match macro_name {
        "one_to_many" => {
            let target = class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str()))));
            let foreign_key = key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{owner_snake}_id")));
            // Sequel leans on the database for cascade deletes; the
            // AR-shaped runtime leans on `dependent: :destroy`. Fold
            // the target table's ON DELETE CASCADE into the
            // association so both express the same behavior.
            let dependent = if fk_cascades(schema, name_str.as_str(), owner_table, &foreign_key) {
                Dependent::Destroy
            } else {
                Dependent::None
            };
            Association::HasMany {
                name,
                target,
                foreign_key,
                through: None,
                dependent,
                as_interface: None,
                scope: order_expr,
            }
        }
        "one_to_one" => Association::HasOne {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{owner_snake}_id"))),
            dependent: Dependent::None,
            as_interface: None,
        },
        "many_to_one" => Association::BelongsTo {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{name_str}_id"))),
            // Optional exactly when the FK column is nullable — Sequel
            // has no `optional:` kwarg; the schema is the authority.
            optional: fk_nullable(schema, owner_table, &format!("{name_str}_id")),
            polymorphic: false,
            polymorphic_targets: Vec::new(),
        },
        "many_to_many" => Association::HasAndBelongsToMany {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str())))),
            join_table: join_table
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| {
                    Symbol::from(crate::naming::habtm_join_table(
                        owner.0.as_str(),
                        name_str.as_str(),
                    ))
                }),
        },
        _ => unreachable!("matched above"),
    }))
}

/// Does the association target's table carry `ON DELETE CASCADE` back
/// to the owner? (`one_to_many :comments` on Article → does
/// `comments.article_id` cascade against `articles`?)
fn fk_cascades(schema: &Schema, assoc_plural: &str, owner_table: &str, fk: &Symbol) -> bool {
    schema
        .tables
        .get(&Symbol::from(assoc_plural))
        .map(|t| {
            t.foreign_keys.iter().any(|f| {
                f.from_column == *fk
                    && f.to_table.0.as_str() == owner_table
                    && matches!(f.on_delete, ReferentialAction::Cascade)
            })
        })
        .unwrap_or(false)
}

fn fk_nullable(schema: &Schema, owner_table: &str, fk_col: &str) -> bool {
    schema
        .tables
        .get(&Symbol::from(owner_table))
        .and_then(|t| t.columns.iter().find(|c| c.name.as_str() == fk_col))
        .map(|c| c.nullable)
        .unwrap_or(true)
}

/// `order: Sequel.desc(:created_at)` / `order: :created_at` → the
/// association-scope Expr the Rails ingest records for
/// `has_many :x, -> { order(...) }`.
fn sequel_order_to_scope(value: &Node<'_>, file: &str) -> IngestResult<Expr> {
    // `Sequel.desc(:col)` / `Sequel.asc(:col)`
    if let Some(call) = value.as_call_node() {
        let dir = constant_id_str(&call.name()).to_string();
        let recv_is_sequel = call
            .receiver()
            .and_then(|r| r.as_constant_read_node().map(|c| c.name()))
            .is_some_and(|n| constant_id_str(&n) == "Sequel");
        if recv_is_sequel && matches!(dir.as_str(), "desc" | "asc") {
            if let Some(col) = call
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|n| symbol_value(&n))
            {
                return Ok(order_call(hash_kwargs(vec![(
                    sym_lit(&col),
                    sym_lit(&dir),
                )])));
            }
        }
    }
    // Bare `:col` — ascending.
    if let Some(col) = symbol_value(value) {
        return Ok(order_call(sym_lit(&col)));
    }
    Err(IngestError::Unsupported {
        file: file.into(),
        message: "sequel association order: shape not recognized".into(),
    })
}

/// `def validate; super; validates_presence [...]; … end` → one
/// `ModelBodyItem::Validation` per (attribute, rule). Statements
/// outside the recognized vocabulary are ledger entries, per stance
/// (c) of the plan — never silently dropped, never corrupting the IR.
fn ingest_validate_body(
    def: &ruby_prism::DefNode<'_>,
    file: &str,
    span: Span,
) -> IngestResult<Vec<ModelBodyItem>> {
    let mut out: Vec<ModelBodyItem> = Vec::new();
    let Some(body) = def.body() else { return Ok(out) };
    for stmt in flatten_statements(body) {
        // `super` re-runs inherited hooks — the framework's own
        // machinery covers that; nothing to record.
        if stmt.as_super_node().is_some() || stmt.as_forwarding_super_node().is_some() {
            continue;
        }
        let recognized = stmt
            .as_call_node()
            .filter(|c| c.receiver().is_none())
            .and_then(|call| parse_validation_helper(&call));
        match recognized {
            Some(validations) => {
                for validation in validations {
                    out.push(ModelBodyItem::Validation {
                        validation,
                        leading_comments: Vec::new(),
                        leading_blank_line: false,
                        span,
                    });
                }
            }
            None => {
                let err = IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "statement in #validate not in the validates_* vocabulary \
                         (byte offset {})",
                        stmt.location().start_offset()
                    ),
                };
                if !super::survey::is_active() {
                    return Err(err);
                }
                super::survey::record(&err);
            }
        }
    }
    Ok(out)
}

/// One validation_helpers call → its `Validation`s. `None` = not in
/// the vocabulary (caller ledgers it).
fn parse_validation_helper(call: &ruby_prism::CallNode<'_>) -> Option<Vec<Validation>> {
    let helper = constant_id_str(&call.name()).to_string();
    let args: Vec<Node<'_>> = call
        .arguments()
        .map(|a| a.arguments().iter().collect())
        .unwrap_or_default();
    let message = args.iter().find_map(|a| {
        let kh = a.as_keyword_hash_node()?;
        for el in kh.elements().iter() {
            let assoc = el.as_assoc_node()?;
            if symbol_value(&assoc.key()).as_deref() == Some("message") {
                return string_value(&assoc.value());
            }
        }
        None
    });
    // Attribute args may be a single symbol or an array of symbols.
    let attrs_of = |node: &Node<'_>| -> Vec<Symbol> {
        if let Some(arr) = node.as_array_node() {
            arr.elements()
                .iter()
                .filter_map(|e| symbol_value(&e))
                .map(|s| Symbol::from(s.as_str()))
                .collect()
        } else {
            symbol_value(node)
                .map(|s| vec![Symbol::from(s.as_str())])
                .unwrap_or_default()
        }
    };

    match helper.as_str() {
        // `validates_presence [:title, :body]`
        "validates_presence" => {
            // A custom presence message has no IR slot yet; surfacing
            // that gap beats silently using the default text.
            if message.is_some() {
                return None;
            }
            let attrs = attrs_of(args.first()?);
            (!attrs.is_empty()).then(|| {
                attrs
                    .into_iter()
                    .map(|attribute| Validation {
                        attribute,
                        rules: vec![ValidationRule::Presence],
                    })
                    .collect()
            })
        }
        // `validates_min_length 10, :body[, message: "..."]`
        "validates_min_length" | "validates_max_length" => {
            let n = integer_value(args.first()?)?;
            if n < 0 {
                return None;
            }
            let attrs = attrs_of(args.get(1)?);
            let (min, max) = if helper == "validates_min_length" {
                (Some(n as u32), None)
            } else {
                (None, Some(n as u32))
            };
            (!attrs.is_empty()).then(|| {
                attrs
                    .into_iter()
                    .map(|attribute| Validation {
                        attribute,
                        rules: vec![ValidationRule::Length {
                            min,
                            max,
                            message: message.clone(),
                        }],
                    })
                    .collect()
            })
        }
        _ => None,
    }
}

// Sequel-surface → AR-surface expression normalization ------------------

/// Rewrite Sequel dataset/model spellings inside `expr` (recursively)
/// into their ActiveRecord equivalents. Applied to model method
/// bodies, linearized route bodies, and seeds — every Expr channel a
/// Roda + Sequel app feeds into the IR.
///
/// The vocabulary (see docs/roda-sequel-plan.md):
/// - `Article[x]`            → `Article.find_by(id: x)`
/// - `.eager(:a)`            → `.includes(:a)`
/// - `.reverse(:col)`        → `.order(col: :desc)`
/// - `.with_pk(x)`           → `.find_by(id: x)`
/// - `article.comments_dataset` → `article.comments`
/// - `X.dataset.delete`      → `X.delete_all`
/// - `x.add_comment(h)`      → `x.comments.create(h)`
/// - `M.new.set_fields(params[:k], %w[a b])` → `M.new(params.expect(k: [:a, :b]))`
/// - `m.set_fields(params[:k], %w[a b])`     → `m.assign_attributes(params.expect(k: [:a, :b]))`
///
/// Anything shaped differently is left verbatim — analyze's unknown-
/// method diagnostics are the ledger for what this pass didn't cover.
pub(super) fn normalize_sequel_expr(expr: &mut Expr) {
    // Children first, so pattern matches below see normalized subtrees.
    expr.node.for_each_child_mut(&mut normalize_sequel_expr);

    let replacement: Option<ExprNode> = match &*expr.node {
        ExprNode::Send { recv: Some(recv), method, args, block, parenthesized } => {
            match method.as_str() {
                // `Article[id]` — class-level primary-key lookup.
                "[]" if args.len() == 1 && matches!(&*recv.node, ExprNode::Const { .. }) => {
                    Some(ExprNode::Send {
                        recv: Some(recv.clone()),
                        method: Symbol::from("find_by"),
                        args: vec![hash_kwargs(vec![(sym_lit("id"), args[0].clone())])],
                        block: None,
                        parenthesized: true,
                    })
                }
                "eager" => Some(ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: Symbol::from("includes"),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: *parenthesized,
                }),
                // `.reverse(:col)` — Array#reverse takes no args, so the
                // one-symbol form is unambiguously the dataset method.
                "reverse" if args.len() == 1 && sym_of(&args[0]).is_some() => {
                    let col = sym_of(&args[0]).expect("checked");
                    Some(ExprNode::Send {
                        recv: Some(recv.clone()),
                        method: Symbol::from("order"),
                        args: vec![hash_kwargs(vec![(sym_lit(&col), sym_lit("desc"))])],
                        block: None,
                        parenthesized: true,
                    })
                }
                // Sequel's terminal `.all` materializes a dataset to an
                // Array; the AR-shaped runtime's chains already
                // materialize, so the call is a no-op — drop it. Bare
                // `Article.all` (Const receiver) stays: that's AR's own
                // whole-table read.
                "all" if args.is_empty()
                    && block.is_none()
                    && matches!(&*recv.node, ExprNode::Send { .. }) =>
                {
                    Some((*recv.node).clone())
                }
                "with_pk" if args.len() == 1 => Some(ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: Symbol::from("find_by"),
                    args: vec![hash_kwargs(vec![(sym_lit("id"), args[0].clone())])],
                    block: None,
                    parenthesized: true,
                }),
                // `article.comments_dataset` → the association reader
                // (AR association readers return the relation form).
                m if m.ends_with("_dataset") && args.is_empty() && block.is_none() => {
                    Some(ExprNode::Send {
                        recv: Some(recv.clone()),
                        method: Symbol::from(m.trim_end_matches("_dataset")),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    })
                }
                // `Comment.dataset.delete` → `Comment.delete_all`.
                "delete" if args.is_empty() => match &*recv.node {
                    ExprNode::Send {
                        recv: Some(class_recv),
                        method: ds,
                        args: ds_args,
                        ..
                    } if ds.as_str() == "dataset"
                        && ds_args.is_empty()
                        && matches!(&*class_recv.node, ExprNode::Const { .. }) =>
                    {
                        Some(ExprNode::Send {
                            recv: Some(class_recv.clone()),
                            method: Symbol::from("delete_all"),
                            args: Vec::new(),
                            block: None,
                            parenthesized: false,
                        })
                    }
                    _ => None,
                },
                // `first.add_comment(h)` → `first.comments.create(h)`.
                m if m.starts_with("add_") && args.len() == 1 && block.is_none() => {
                    let plural = pluralize_snake(&camelize(m.trim_start_matches("add_")));
                    let assoc_read = Expr::new(
                        expr.span,
                        ExprNode::Send {
                            recv: Some(recv.clone()),
                            method: Symbol::from(plural),
                            args: Vec::new(),
                            block: None,
                            parenthesized: false,
                        },
                    );
                    Some(ExprNode::Send {
                        recv: Some(assoc_read),
                        method: Symbol::from("create"),
                        args: args.clone(),
                        block: None,
                        parenthesized: true,
                    })
                }
                "set_fields" if args.len() == 2 => rewrite_set_fields(recv, args, expr.span),
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(node) = replacement {
        expr.node = Box::new(node);
    }
}

/// `M.new.set_fields(params[:k], %w[a b])` → `M.new(params.expect(k: [:a, :b]))`;
/// `m.set_fields(params[:k], %w[a b])` → `m.assign_attributes(params.expect(...))`.
/// Requires the hash to be a `params[...]` read and the field list to
/// be literal strings/symbols — the statically-checkable allow-list
/// idiom. Other shapes stay verbatim (ledger via analyze).
fn rewrite_set_fields(recv: &Expr, args: &[Expr], span: Span) -> Option<ExprNode> {
    // args[0]: `params[:article]` — Send { recv: params(), "[]", [Sym] }.
    let params_key = match &*args[0].node {
        ExprNode::Send { recv: Some(p), method, args: idx, .. }
            if method.as_str() == "[]"
                && idx.len() == 1
                && matches!(
                    &*p.node,
                    ExprNode::Send { recv: None, method, args, .. }
                        if method.as_str() == "params" && args.is_empty()
                ) =>
        {
            sym_of(&idx[0])?
        }
        _ => return None,
    };
    // args[1]: `%w[title body]` (or `%i[...]`).
    let fields: Vec<String> = match &*args[1].node {
        ExprNode::Array { elements, .. } => elements
            .iter()
            .map(|e| match &*e.node {
                ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.to_string()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    let expect = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("params"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            )),
            method: Symbol::from("expect"),
            args: vec![hash_kwargs(vec![(
                sym_lit(&params_key),
                Expr::new(
                    span,
                    ExprNode::Array {
                        elements: fields.iter().map(|f| sym_lit(f)).collect(),
                        style: crate::expr::ArrayStyle::default(),
                    },
                ),
            )])],
            block: None,
            parenthesized: true,
        },
    );
    match &*recv.node {
        // `M.new.set_fields(...)` → `M.new(expect)`.
        ExprNode::Send { recv: Some(class_recv), method, args: new_args, .. }
            if method.as_str() == "new"
                && new_args.is_empty()
                && matches!(&*class_recv.node, ExprNode::Const { .. }) =>
        {
            Some(ExprNode::Send {
                recv: Some(class_recv.clone()),
                method: Symbol::from("new"),
                args: vec![expect],
                block: None,
                parenthesized: true,
            })
        }
        // `m.set_fields(...)` → `m.assign_attributes(expect)`.
        _ => Some(ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from("assign_attributes"),
            args: vec![expect],
            block: None,
            parenthesized: true,
        }),
    }
}

// Small Expr constructors ------------------------------------------------

pub(super) fn sym_lit(s: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } },
    )
}

pub(super) fn hash_kwargs(entries: Vec<(Expr, Expr)>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: true })
}

fn order_call(arg: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("order"),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
    )
}

fn sym_of(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.to_string()),
        _ => None,
    }
}
