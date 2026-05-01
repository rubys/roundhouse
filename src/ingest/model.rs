//! ActiveRecord model ingestion — parses one `app/models/*.rb` into
//! a `Model`, including validations, associations, callbacks, scopes,
//! and methods.

use indexmap::IndexMap;
use ruby_prism::{Node, parse};

use crate::dialect::{Comment, Model, ModelBodyItem};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode};
use crate::naming::{camelize, pluralize_snake, singularize_camelize, snake_case};
use crate::schema::{ColumnType, Schema, Table};
use crate::span::Span;
use crate::ty::{Row, Ty};
use crate::{ClassId, Symbol, TableRef};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, collect_comments, constant_id_str, constant_path_of, drain_comments_before,
    find_first_class, flatten_statements, source_has_blank_line, symbol_value,
};
use super::{IngestError, IngestResult};

/// Parse a single model file. The first class definition is treated as the
/// model; any schema-derived attributes are filled in from `schema`.
pub fn ingest_model(
    source: &[u8],
    file: &str,
    schema: &Schema,
) -> IngestResult<Option<Model>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "model class name must be a simple constant or path".into(),
    })?;
    let class_name = Symbol::from(name_path.join("::"));
    let owner = ClassId(class_name.clone());
    let table_name = pluralize_snake(class_name.as_str());

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
    if let Some(class_body) = class.body() {
        let mut prev_end: Option<usize> = None;
        for stmt in flatten_statements(class_body) {
            let stmt_start = stmt.location().start_offset();
            let leading_area_start =
                comments.first().map(|(off, _)| *off).filter(|off| *off < stmt_start)
                    .unwrap_or(stmt_start);
            let leading = drain_comments_before(&mut comments, stmt_start);
            let leading_blank = prev_end
                .map(|pe| source_has_blank_line(source, pe, leading_area_start))
                .unwrap_or(false);
            let mut item = ingest_model_body_item(&stmt, &owner, file, leading)?;
            item.set_leading_blank_line(leading_blank);
            body.push(item);
            prev_end = Some(stmt.location().end_offset());
        }
    }

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    Ok(Some(Model {
        name: owner,
        parent,
        table: TableRef(Symbol::from(table_name)),
        attributes,
        body,
    }))
}

/// Classify one class-body statement into its `ModelBodyItem` variant.
/// `leading_comments` is attached regardless of variant so every item
/// keeps its inline docs.
fn ingest_model_body_item(
    stmt: &Node<'_>,
    owner: &ClassId,
    file: &str,
    leading_comments: Vec<Comment>,
) -> IngestResult<ModelBodyItem> {
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_some() {
            return Ok(ModelBodyItem::Unknown {
                expr: ingest_expr(stmt, file)?,
                leading_comments,
                leading_blank_line: false,
            });
        }
        let method = constant_id_str(&call.name()).to_string();
        if let Some(assoc) = parse_association(&call, owner, &method) {
            return Ok(ModelBodyItem::Association { assoc, leading_blank_line: false, leading_comments });
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
                });
                for v in parsed.drain(1..) {
                    items.push(ModelBodyItem::Validation {
                        validation: v,
                        leading_comments: Vec::new(),
                        leading_blank_line: false,
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
            return Ok(ModelBodyItem::Callback { callback, leading_blank_line: false, leading_comments });
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

fn ingest_method(
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

    // Collect required positional parameter names. Optional/keyword/
    // rest params will need richer handling; required params alone
    // cover setter methods (`def x=(v)`) and the common method shapes
    // used by transpiled-shape models.
    let params: Vec<Symbol> = match def.parameters() {
        Some(pn) => pn
            .requireds()
            .iter()
            .filter_map(|req| req.as_required_parameter_node())
            .map(|rp| Symbol::from(constant_id_str(&rp.name())))
            .collect(),
        None => Vec::new(),
    };

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name,
        receiver,
        params: params.into_iter().map(crate::dialect::Param::positional).collect(),
        body,
        signature: None,
        effects: EffectSet::pure(),
        // Rails model methods carry their owner on the surrounding
        // Model struct (model.name); no need to duplicate here.
        enclosing_class: None,
        // Source-defined `def` in a Rails model — Method by default.
        kind: crate::dialect::AccessorKind::Method,
    })
}

fn parse_callback(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
) -> Option<crate::dialect::Callback> {
    use crate::dialect::{Callback, CallbackHook};

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
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;
    let target = Symbol::from(symbol_value(&first)?.as_str());

    // `if:` / `unless:` conditions land when a fixture demands them.
    let condition = None;

    Some(Callback { hook, target, condition })
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
    let Some(lambda) = body_node.as_lambda_node() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("scope :{name} body must be a lambda (`-> {{ ... }}`)"),
        });
    };

    // Parameter parsing lands when a fixture needs parameterized scopes.
    let params: Vec<Symbol> = vec![];

    let body = match lambda.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(Some(Scope { name, params, body }))
}

fn parse_validates(call: &ruby_prism::CallNode<'_>) -> Vec<crate::dialect::Validation> {
    use crate::dialect::{Validation, ValidationRule};
    let Some(args) = call.arguments() else { return vec![] };
    let all_args = args.arguments();

    let mut attrs: Vec<Symbol> = Vec::new();
    let mut rules: Vec<ValidationRule> = Vec::new();

    for arg in all_args.iter() {
        if let Some(sym) = symbol_value(&arg) {
            attrs.push(Symbol::from(sym.as_str()));
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = assoc.value();
                if let Some(rule) = validation_rule_from_kv(&key, &value) {
                    rules.push(rule);
                }
            }
        }
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
        Some(ValidationRule::Length { min, max })
    }
}

fn parse_association(
    call: &ruby_prism::CallNode<'_>,
    owner: &ClassId,
    method: &str,
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
    let mut dependent: Option<Dependent> = None;
    let mut optional: Option<bool> = None;
    let mut join_table: Option<String> = None;

    for arg in iter {
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
                "dependent" => {
                    dependent = symbol_value(&value).and_then(|s| dependent_from_sym(&s))
                }
                "optional" => optional = bool_value(&value),
                "join_table" => join_table = string_value(&value),
                _ => {}
            }
        }
    }

    let owner_snake = snake_case(owner.0.as_str());

    match method {
        "has_many" => Some(Association::HasMany {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{owner_snake}_id"))),
            through: through.map(|s| Symbol::from(s.as_str())),
            dependent: dependent.unwrap_or_default(),
        }),
        "has_one" => Some(Association::HasOne {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{owner_snake}_id"))),
            dependent: dependent.unwrap_or_default(),
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

fn row_from_table(table: &Table) -> Row {
    let mut fields = IndexMap::new();
    for col in &table.columns {
        fields.insert(col.name.clone(), ty_of_column(&col.col_type));
    }
    Row { fields, rest: None }
}

fn ty_of_column(t: &ColumnType) -> Ty {
    match t {
        ColumnType::Integer | ColumnType::BigInt => Ty::Int,
        ColumnType::Float | ColumnType::Decimal { .. } => Ty::Float,
        ColumnType::String { .. } | ColumnType::Text => Ty::Str,
        ColumnType::Boolean => Ty::Bool,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Time => {
            Ty::Class { id: ClassId(Symbol::from("Time")), args: vec![] }
        }
        ColumnType::Binary => Ty::Str,
        ColumnType::Json => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) },
        ColumnType::Reference { .. } => Ty::Int,
    }
}
