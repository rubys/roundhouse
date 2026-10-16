//! `render json: <record>` for a table-backed model with no `as_json`: Rails' `serializable_hash`, written down from the columns.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::dialect::{MethodDef, Model, ModelBodyItem};
use crate::expr::ExprNode;
use crate::ident::ClassId;
use crate::schema::{ColumnType, Table};
use crate::ty::Ty;

use super::as_json_poro::encoding_for;
use super::as_json_shape::{JsonPair, PairValue, ShapeError};
use super::as_json_writer::{writer_method, PairEncoding};

// Not the columns alone: these add to `attribute_names` or change what a column's reader answers.
pub(super) const ATTRIBUTE_MACROS: &[&str] =
    &["attribute", "serialize", "store", "store_accessor", "encrypts", "composed_of"];

// Not read from the declaration: an app that renames the inheritance column or ignores columns keeps the runtime encoder.
pub(super) const COLUMN_SETTERS: &[&str] = &["inheritance_column=", "ignored_columns=", "ignored_columns"];

pub(super) fn column_writer(
    model: &Model,
    table: &Table,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Result<MethodDef, ShapeError> {
    column_writer_with(model, table, registry, &SerializeOpts::default())
}

/// `serializable_hash(only:, except:, methods:)` as a writer named for those options.
pub(super) fn column_writer_with(
    model: &Model,
    table: &Table,
    registry: &HashMap<ClassId, ClassInfo>,
    opts: &SerializeOpts,
) -> Result<MethodDef, ShapeError> {
    if model.body.iter().any(|item| match item {
        ModelBodyItem::Unknown { expr, .. } => matches!(
            &*expr.node,
            ExprNode::Send { recv: None, method, .. } if ATTRIBUTE_MACROS.contains(&method.as_str())
        ),
        _ => false,
    }) {
        return Err("the model declares attributes beyond its columns");
    }
    if model.body.iter().any(|item| match item {
        ModelBodyItem::Unknown { expr, .. } => matches!(
            &*expr.node,
            ExprNode::Send { method, .. } if COLUMN_SETTERS.contains(&method.as_str())
        ),
        _ => false,
    }) {
        return Err("the model changes which columns serialize");
    }
    let info = registry.get(&model.name).ok_or("the model has no analyzed class info")?;
    let mut pairs = Vec::new();
    // Not the table's order under `only:`: Rails' `serializable_hash` takes `Array(only) & attribute_names`, the `only:` list's order.
    let columns: Vec<_> = match &opts.only {
        Some(only) => {
            let mut seen = Vec::new();
            only.iter()
                .filter(|name| !seen.contains(*name) && { seen.push((*name).clone()); true })
                .filter_map(|name| table.columns.iter().find(|c| &c.name == name))
                .collect()
        }
        // Not serialized: Rails' `serializable_hash` excepts the inheritance column whenever the record has one.
        None => table.columns.iter()
            .filter(|c| c.name.as_str() != "type" && !opts.except.contains(&c.name))
            .collect(),
    };
    for column in columns {
        match &column.col_type {
            // `writer_body` routes these through the date seam and `encode_datetime`, which a later pass zones for a non-UTC app.
            ColumnType::Date | ColumnType::DateTime => {
                // Not the stored value when the model overrides the reader: Rails serializes through `send(name)`.
                if overrides_reader(model, &column.name) {
                    return Err("a temporal column's reader is overridden");
                }
            }
            // Not written: Rails anchors a time to 2000-01-01, quotes a decimal, nests json, and the schema keeps a reference under its logical name.
            ColumnType::Time
            | ColumnType::Decimal { .. }
            | ColumnType::Json
            | ColumnType::Jsonb
            | ColumnType::Binary
            | ColumnType::Reference { .. } => {
                return Err("a column's JSON form is not the one written here");
            }
            _ => {
                let ty = match info.instance_methods.get(&column.name) {
                    Some(Ty::Fn { ret, .. }) => (**ret).clone(),
                    Some(ty) => ty.clone(),
                    None => info
                        .attributes
                        .fields
                        .get(&column.name)
                        .cloned()
                        .ok_or("a column has no typed reader")?,
                };
                if !matches!(encoding_for(&ty)?, PairEncoding::Scalar(_)) {
                    return Err("a column's reader is not a JSON scalar");
                }
            }
        }
        pairs.push(JsonPair { key: column.name.clone(), value: PairValue::Reader(column.name.clone()), cond: None });
    }
    for name in &opts.methods {
        let ty = match info.instance_methods.get(name) {
            Some(Ty::Fn { ret, .. }) => (**ret).clone(),
            Some(ty) => ty.clone(),
            None => return Err("a `methods:` name has no typed reader"),
        };
        if !matches!(encoding_for(&ty)?, PairEncoding::Scalar(_)) {
            return Err("a `methods:` value is not a JSON scalar");
        }
        // Not a `Reader`: that reads a column, which Python spells as an attribute, where a `methods:` name is called.
        pairs.push(JsonPair { key: name.clone(), value: PairValue::Computed(super::as_json_writer::self_call(name.as_str())), cond: None });
    }
    let mut writer = writer_method(&model.name, &pairs, Some(table), &[])?;
    writer.name = crate::ident::Symbol::from(opts.method_name());
    Ok(writer)
}

/// The `only:`/`except:`/`methods:` a `to_json` or `render json:` passes, each a literal Symbol list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct SerializeOpts {
    pub only: Option<Vec<crate::ident::Symbol>>,
    pub except: Vec<crate::ident::Symbol>,
    pub methods: Vec<crate::ident::Symbol>,
}

impl SerializeOpts {
    pub(super) fn method_name(&self) -> String {
        let mut name = super::as_json_writer::WRITER_METHOD.to_string();
        // Not joined by `_` alone: `only: [:a_b, :c]` and `only: [:a, :b_c]` would share a name; each name carries its length.
        let join = |l: &[crate::ident::Symbol]| l.iter().map(|s| format!("{}{}", s.as_str().len(), s.as_str())).collect::<Vec<_>>().join("_");
        if let Some(only) = &self.only {
            name += &format!("__only_{}", join(only));
        }
        if !self.except.is_empty() {
            name += &format!("__except_{}", join(&self.except));
        }
        if !self.methods.is_empty() {
            name += &format!("__methods_{}", join(&self.methods));
        }
        name
    }

    /// Read the options among `entries`. `Err` for one this does not write (`include:`, `root:`, a list that is not literal); `ignore` names the keys that are not serialization options (a render's own `status:`).
    pub(super) fn read(entries: &[(crate::expr::Expr, crate::expr::Expr)], ignore: &[&str]) -> Result<Option<Self>, ()> {
        let mut opts = SerializeOpts::default();
        let mut any = false;
        for (k, v) in entries {
            let ExprNode::Lit { value: crate::expr::Literal::Sym { value: key } } = &*k.node else { return Err(()) };
            if ignore.contains(&key.as_str()) {
                continue;
            }
            let list = match &*v.node {
                ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => vec![value.clone()],
                ExprNode::Array { elements, .. } => elements
                    .iter()
                    .map(|e| match &*e.node {
                        ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => Ok(value.clone()),
                        _ => Err(()),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(()),
            };
            match key.as_str() {
                "only" => opts.only = Some(list),
                "except" => opts.except = list,
                "methods" => opts.methods = list,
                _ => return Err(()),
            }
            any = true;
        }
        Ok(any.then_some(opts))
    }
}

fn overrides_reader(model: &Model, name: &crate::ident::Symbol) -> bool {
    model.body.iter().any(|item| match item {
        ModelBodyItem::Method { method, .. } => {
            method.receiver == crate::dialect::MethodReceiver::Instance && &method.name == name
        }
        ModelBodyItem::Alias { new, .. } => new == name,
        ModelBodyItem::Unknown { expr, .. } => matches!(
            &*expr.node,
            ExprNode::Send { recv: None, method, args, .. }
                if method.as_str() == "alias_attribute"
                    && args.first().is_some_and(|a| matches!(&*a.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } } if value == name))
        ),
        _ => false,
    })
}
