//! Models — Rust struct + impl + persistence + validations + broadcasters.

use std::fmt::Write;
use std::path::PathBuf;

use crate::App;
use crate::dialect::{MethodDef, Model};
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::lower::{BroadcastAction, LoweredBroadcast, LoweredBroadcasts};
use crate::ty::Ty;

use super::super::EmittedFile;
use super::controller::{emit_expr, EmitCtx};
use super::rust_ty;

// ── Broadcaster emission ───────────────────────────────────────

fn rust_render_broadcast_expr(expr: &Expr, self_param: Option<&Symbol>) -> String {
    let p = self_param.map(|s| s.as_str());
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Str { value },
        } => format!("{value:?}"),
        ExprNode::Lit {
            value: Literal::Int { value },
        } => format!("{value}"),
        ExprNode::Var { name, .. } => {
            if let Some(pname) = p {
                let stripped = pname.strip_prefix('_').unwrap_or(pname);
                if name.as_str() == pname || name.as_str() == stripped {
                    return "self".to_string();
                }
            }
            name.as_str().to_string()
        }
        ExprNode::Send {
            recv: Some(r),
            method,
            ..
        } => {
            let recv_s = rust_render_broadcast_expr(r, self_param);
            format!("{recv_s}.{}", method.as_str())
        }
        ExprNode::StringInterp { parts } => {
            let mut fmt = String::new();
            let mut exprs: Vec<String> = Vec::new();
            for part in parts {
                match part {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '{' || c == '}' {
                                fmt.push(c);
                                fmt.push(c);
                            } else {
                                fmt.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        fmt.push_str("{}");
                        exprs.push(rust_render_broadcast_expr(expr, self_param));
                    }
                }
            }
            format!("&format!({:?}, {})", fmt, exprs.join(", "))
        }
        _ => "/* unsupported broadcast expr */".to_string(),
    }
}

fn emit_rust_broadcaster_impl(
    out: &mut String,
    class: &str,
    table: &str,
    decls: &LoweredBroadcasts,
) {
    writeln!(out).unwrap();
    writeln!(out, "impl crate::cable::Broadcaster for {class} {{").unwrap();

    writeln!(out, "    fn after_save(&self) {{").unwrap();
    for b in &decls.save {
        emit_one_broadcast_call(out, class, table, b);
    }
    writeln!(out, "    }}").unwrap();

    writeln!(out, "    fn after_delete(&self) {{").unwrap();
    for b in &decls.destroy {
        emit_one_broadcast_call(out, class, table, b);
    }
    writeln!(out, "    }}").unwrap();

    writeln!(out, "}}").unwrap();
}

fn emit_one_broadcast_call(out: &mut String, class: &str, table: &str, b: &LoweredBroadcast) {
    let channel = rust_render_broadcast_expr(&b.channel, b.self_param.as_ref());
    let target = b
        .target
        .as_ref()
        .map(|t| rust_render_broadcast_expr(t, b.self_param.as_ref()))
        .unwrap_or_else(|| "\"\"".to_string());
    if let Some(assoc) = &b.on_association {
        let var = assoc.name.as_str();
        let target_class = assoc.target_class.as_str();
        let target_table = assoc.target_table.as_str();
        let fk = assoc.foreign_key.as_str();
        writeln!(
            out,
            "        if let Some({var}) = {target_class}::find(self.{fk}) {{"
        )
        .unwrap();
        if b.action == BroadcastAction::Remove {
            writeln!(
                out,
                "            crate::cable::broadcast_remove_to({target_table:?}, {var}.id, {channel}, {target});",
            )
            .unwrap();
        } else {
            let func = action_to_fn(b.action);
            writeln!(
                out,
                "            crate::cable::{func}({target_table:?}, {var}.id, {target_class:?}, {channel}, {target});",
            )
            .unwrap();
        }
        writeln!(out, "        }}").unwrap();
        return;
    }
    if b.action == BroadcastAction::Remove {
        writeln!(
            out,
            "        crate::cable::broadcast_remove_to({table:?}, self.id, {channel}, {target});",
        )
        .unwrap();
    } else {
        let func = action_to_fn(b.action);
        writeln!(
            out,
            "        crate::cable::{func}({table:?}, self.id, {class:?}, {channel}, {target});",
        )
        .unwrap();
    }
}

fn action_to_fn(action: BroadcastAction) -> &'static str {
    match action {
        BroadcastAction::Prepend => "broadcast_prepend_to",
        BroadcastAction::Append => "broadcast_append_to",
        BroadcastAction::Replace => "broadcast_replace_to",
        BroadcastAction::Remove => "broadcast_remove_to",
    }
}

pub(super) fn emit_models(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();

    let any_validations = app
        .models
        .iter()
        .any(|m| !crate::lower::lower_validations(m).is_empty());
    if any_validations {
        writeln!(s).unwrap();
        writeln!(s, "use crate::runtime;").unwrap();
    }

    for model in &app.models {
        writeln!(s).unwrap();
        emit_struct(&mut s, model);
        let lowered = crate::lower::lower_validations(model);
        if !model.attributes.fields.is_empty() {
            writeln!(s).unwrap();
            emit_model_impl(&mut s, model, &lowered, app);
        }
    }
    EmittedFile { path: PathBuf::from("src/models.rs"), content: s }
}

fn emit_struct(out: &mut String, model: &Model) {
    writeln!(out, "#[derive(Debug, Clone, Default, PartialEq)]").unwrap();
    writeln!(out, "pub struct {} {{", model.name.0).unwrap();
    for (name, ty) in &model.attributes.fields {
        writeln!(out, "    pub {}: {},", name, rust_ty(ty)).unwrap();
    }
    writeln!(
        out,
        "    pub errors: Vec<crate::runtime::ValidationError>,",
    )
    .unwrap();
    writeln!(out, "}}").unwrap();
}

fn emit_model_impl(
    out: &mut String,
    model: &Model,
    validations: &[crate::lower::LoweredValidation],
    app: &App,
) {
    writeln!(out, "impl {} {{", model.name.0).unwrap();
    let self_methods: Vec<Symbol> = model
        .attributes
        .fields
        .keys()
        .cloned()
        .chain(model.methods().map(|m| m.name.clone()))
        .collect();

    let mut first = true;
    for method in model.methods() {
        if !first {
            writeln!(out).unwrap();
        }
        first = false;
        emit_model_method(out, method, &self_methods);
    }
    if !validations.is_empty() {
        if !first {
            writeln!(out).unwrap();
        }
        emit_validate_method(out, validations);
    }
    if !first || !validations.is_empty() {
        writeln!(out).unwrap();
    }
    let broadcasts = crate::lower::lower_broadcasts(model);
    emit_persistence_methods(
        out,
        model,
        !validations.is_empty(),
        app,
        !broadcasts.is_empty(),
    );
    writeln!(out, "}}").unwrap();

    if !broadcasts.is_empty() {
        let lp = crate::lower::lower_persistence(model, app);
        emit_rust_broadcaster_impl(
            out,
            lp.class.0.as_str(),
            lp.table.as_str(),
            &broadcasts,
        );
    }
}

fn emit_persistence_methods(
    out: &mut String,
    model: &Model,
    has_validate: bool,
    app: &App,
    has_broadcasts: bool,
) {
    let lp = crate::lower::lower_persistence(model, app);
    let class = lp.class.0.as_str();

    let non_id_params: Vec<String> = lp
        .non_id_columns
        .iter()
        .map(|s| format!("self.{}", s.as_str()))
        .collect();

    // ----- save -----
    writeln!(out, "    pub fn save(&mut self) -> bool {{").unwrap();
    if has_validate {
        writeln!(out, "        let errors = self.validate();").unwrap();
        writeln!(out, "        if !errors.is_empty() {{ self.errors = errors; return false; }}").unwrap();
        writeln!(out, "        self.errors.clear();").unwrap();
    }
    for check in &lp.belongs_to_checks {
        let fk = check.foreign_key.as_str();
        let target = check.target_class.0.as_str();
        writeln!(
            out,
            "        if self.{fk} == 0 || {target}::find(self.{fk}).is_none() {{",
        )
        .unwrap();
        writeln!(out, "            return false;").unwrap();
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(out, "            if self.id == 0 {{").unwrap();
    writeln!(
        out,
        "                conn.execute(\n                    {:?},\n                    rusqlite::params![{}],\n                ).expect(\"INSERT {}\");",
        lp.insert_sql,
        non_id_params.join(", "),
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "                self.id = conn.last_insert_rowid();").unwrap();
    writeln!(out, "            }} else {{").unwrap();
    writeln!(
        out,
        "                conn.execute(\n                    {:?},\n                    rusqlite::params![{}, self.id],\n                ).expect(\"UPDATE {}\");",
        lp.update_sql,
        non_id_params.join(", "),
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }});").unwrap();
    if has_broadcasts {
        writeln!(
            out,
            "        crate::cable::Broadcaster::after_save(self);"
        )
        .unwrap();
    }
    writeln!(out, "        true").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- destroy -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn destroy(&self) {{").unwrap();
    for dc in &lp.dependent_children {
        let child_class = dc.child_class.0.as_str();
        writeln!(
            out,
            "        let dependents: Vec<{child_class}> = crate::db::with_conn(|conn| {{"
        )
        .unwrap();
        writeln!(
            out,
            "            let mut stmt = conn.prepare({:?}).expect(\"prepare child select\");",
            dc.select_by_parent_sql,
        )
        .unwrap();
        writeln!(
            out,
            "            let rows = stmt.query_map(rusqlite::params![self.id], |r| Ok({child_class} {{"
        )
        .unwrap();
        for (i, col) in dc.child_columns.iter().enumerate() {
            writeln!(out, "                {}: r.get({i})?,", col.as_str()).unwrap();
        }
        writeln!(out, "                ..Default::default()").unwrap();
        writeln!(out, "            }})).expect(\"query child rows\");").unwrap();
        writeln!(out, "            rows.filter_map(|r| r.ok()).collect()").unwrap();
        writeln!(out, "        }});").unwrap();
        writeln!(out, "        for child in &dependents {{").unwrap();
        writeln!(out, "            child.destroy();").unwrap();
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.execute({:?}, rusqlite::params![self.id])\n                .expect(\"DELETE {}\");",
        lp.delete_sql,
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "        }});").unwrap();
    if has_broadcasts {
        writeln!(
            out,
            "        crate::cable::Broadcaster::after_delete(self);"
        )
        .unwrap();
    }
    writeln!(out, "    }}").unwrap();

    // ----- count -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn count() -> i64 {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.query_row({:?}, [], |r| r.get(0))\n                .expect(\"count {}\")",
        lp.count_sql,
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- find -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn find(id: i64) -> Option<{class}> {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.query_row(\n                {:?},\n                rusqlite::params![id],",
        lp.select_by_id_sql,
    )
    .unwrap();
    writeln!(out, "                |r| Ok({class} {{").unwrap();
    for (i, field) in lp.columns.iter().enumerate() {
        writeln!(out, "                    {}: r.get({i})?,", field.as_str()).unwrap();
    }
    writeln!(out, "                    ..Default::default()").unwrap();
    writeln!(out, "                }}),\n            ).ok()").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- all -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn all() -> Vec<{class}> {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            let mut stmt = conn.prepare({:?}).expect(\"prepare all\");",
        lp.select_all_sql,
    )
    .unwrap();
    writeln!(out, "            let rows = stmt").unwrap();
    writeln!(out, "                .query_map([], |r| Ok({class} {{").unwrap();
    for (i, field) in lp.columns.iter().enumerate() {
        writeln!(out, "                    {}: r.get({i})?,", field.as_str()).unwrap();
    }
    writeln!(out, "                    ..Default::default()").unwrap();
    writeln!(out, "                }}))").unwrap();
    writeln!(out, "                .expect(\"query all\");").unwrap();
    writeln!(out, "            rows.filter_map(|r| r.ok()).collect()").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- last -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn last() -> Option<{class}> {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.query_row(\n                {:?},\n                [],",
        lp.select_last_sql,
    )
    .unwrap();
    writeln!(out, "                |r| Ok({class} {{").unwrap();
    for (i, field) in lp.columns.iter().enumerate() {
        writeln!(out, "                    {}: r.get({i})?,", field.as_str()).unwrap();
    }
    writeln!(out, "                    ..Default::default()").unwrap();
    writeln!(out, "                }}),\n            ).ok()").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- reload -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn reload(&mut self) {{").unwrap();
    writeln!(out, "        if let Some(fresh) = Self::find(self.id) {{").unwrap();
    writeln!(out, "            *self = fresh;").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn emit_validate_method(out: &mut String, validations: &[crate::lower::LoweredValidation]) {
    writeln!(
        out,
        "    pub fn validate(&self) -> Vec<runtime::ValidationError> {{"
    )
    .unwrap();
    writeln!(out, "        let mut errors = Vec::new();").unwrap();
    for lv in validations {
        for check in &lv.checks {
            emit_check_inline(out, lv.attribute.as_str(), check);
        }
    }
    writeln!(out, "        errors").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn emit_check_inline(out: &mut String, attr: &str, check: &crate::lower::Check) {
    use crate::lower::{Check, InclusionValue};
    let msg = check.default_message();
    let push = |cond: &str| -> String {
        format!(
            "        if {cond} {{\n            errors.push(runtime::ValidationError::new({attr:?}, {msg:?}));\n        }}",
        )
    };
    let block = match check {
        Check::Presence => push(&format!("self.{attr}.is_empty()")),
        Check::Absence => push(&format!("!self.{attr}.is_empty()")),
        Check::MinLength { n } => push(&format!("self.{attr}.len() < {n}")),
        Check::MaxLength { n } => push(&format!("self.{attr}.len() > {n}")),
        Check::GreaterThan { threshold } => {
            push(&format!("self.{attr} <= {threshold}"))
        }
        Check::LessThan { threshold } => push(&format!("self.{attr} >= {threshold}")),
        Check::OnlyInteger => {
            format!("        // OnlyInteger check on {attr:?} — enforced by Rust type system")
        }
        Check::Inclusion { values } => {
            let parts: Vec<String> = values.iter().map(inclusion_value_to_rust).collect();
            push(&format!(
                "![{}].contains(&self.{attr})",
                parts.join(", ")
            ))
        }
        Check::Format { pattern } => {
            format!(
                "        // TODO: Format check on {attr:?} requires runtime regex ({pattern:?})",
            )
        }
        Check::Uniqueness { .. } => {
            format!(
                "        // TODO: Uniqueness check on {attr:?} requires DB access at runtime",
            )
        }
        Check::Custom { method } => {
            let _ = InclusionValue::Str { value: String::new() };
            format!("        self.{method}(&mut errors);")
        }
    };
    writeln!(out, "{block}").unwrap();
}

fn inclusion_value_to_rust(v: &crate::lower::InclusionValue) -> String {
    use crate::lower::InclusionValue;
    match v {
        InclusionValue::Str { value } => format!("{value:?}.to_string()"),
        InclusionValue::Int { value } => format!("{value}i64"),
        InclusionValue::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { format!("{s}f64") } else { format!("{s}.0f64") }
        }
        InclusionValue::Bool { value } => value.to_string(),
    }
}

fn emit_model_method(out: &mut String, m: &MethodDef, self_methods: &[Symbol]) {
    let ret_ty = m.body.ty.clone().unwrap_or(Ty::Nil);
    let receiver = match m.receiver {
        crate::dialect::MethodReceiver::Instance => "&self",
        crate::dialect::MethodReceiver::Class => "",
    };
    writeln!(
        out,
        "    pub fn {}({}) -> {} {{",
        m.name,
        receiver,
        rust_ty(&ret_ty),
    )
    .unwrap();
    let ctx = EmitCtx {
        self_methods,
        ..EmitCtx::default()
    };
    for line in emit_expr(&m.body, ctx).lines() {
        writeln!(out, "        {}", line).unwrap();
    }
    writeln!(out, "    }}").unwrap();
}
