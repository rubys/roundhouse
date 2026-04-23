//! Rails controller ingestion — parses one `app/controllers/*.rb`
//! into a `Controller`, splitting class-body items into actions,
//! filters, a `private` marker, and unknown fall-throughs.

use ruby_prism::{Node, parse};

use crate::dialect::{Action, Comment, Controller, ControllerBodyItem, RenderTarget};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode};
use crate::span::Span;
use crate::ty::Row;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, collect_comments, constant_id_str, constant_path_of, drain_comments_before,
    find_first_class, flatten_statements, source_has_blank_line, symbol_list_style,
    symbol_list_value, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_controller(source: &[u8], file: &str) -> IngestResult<Option<Controller>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "controller class name must be a simple constant or path".into(),
    })?;

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    let mut comments = collect_comments(&result);
    drain_comments_before(&mut comments, class.location().start_offset());
    let mut body_items: Vec<ControllerBodyItem> = Vec::new();
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
            let mut item = ingest_controller_body_item(&stmt, file, leading)?;
            item.set_leading_blank_line(leading_blank);
            body_items.push(item);
            prev_end = Some(stmt.location().end_offset());
        }
    }

    Ok(Some(Controller {
        name: ClassId(Symbol::from(name_path.join("::"))),
        parent,
        body: body_items,
    }))
}

/// Classify one class-body statement into its `ControllerBodyItem` variant.
fn ingest_controller_body_item(
    stmt: &Node<'_>,
    file: &str,
    leading_comments: Vec<Comment>,
) -> IngestResult<ControllerBodyItem> {
    if let Some(def) = stmt.as_def_node() {
        let action_name = constant_id_str(&def.name()).to_string();
        let body_expr = match def.body() {
            Some(b) => ingest_expr(&b, file)?,
            None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
        };
        return Ok(ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from(action_name),
                params: Row::closed(),
                body: body_expr,
                renders: RenderTarget::Inferred,
                effects: EffectSet::pure(),
            },
            leading_comments,
            leading_blank_line: false,
        });
    }
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_some() {
            return Ok(ControllerBodyItem::Unknown {
                expr: ingest_expr(stmt, file)?,
                leading_comments,
                leading_blank_line: false,
            });
        }
        let method = constant_id_str(&call.name()).to_string();
        if let Some(filter) = parse_filter(&call, &method) {
            return Ok(ControllerBodyItem::Filter {
                filter,
                leading_blank_line: false,
                leading_comments,
            });
        }
        if method == "private" && call.arguments().is_none() && call.block().is_none() {
            return Ok(ControllerBodyItem::PrivateMarker {
                leading_blank_line: false,
                leading_comments,
            });
        }
        return Ok(ControllerBodyItem::Unknown {
            expr: ingest_expr(stmt, file)?,
            leading_comments,
            leading_blank_line: false,
        });
    }
    Ok(ControllerBodyItem::Unknown {
        expr: ingest_expr(stmt, file)?,
        leading_comments,
        leading_blank_line: false,
    })
}

fn parse_filter(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
) -> Option<crate::dialect::Filter> {
    use crate::dialect::{Filter, FilterKind};

    let kind = match method {
        "before_action" => FilterKind::Before,
        "around_action" => FilterKind::Around,
        "after_action" => FilterKind::After,
        "skip_before_action" => FilterKind::Skip,
        _ => return None,
    };

    let args = call.arguments()?;
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;
    let target = Symbol::from(symbol_value(&first)?.as_str());

    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    let mut only_style = crate::expr::ArrayStyle::default();
    let mut except_style = crate::expr::ArrayStyle::default();

    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "only" => {
                    only = symbol_list_value(&value);
                    only_style = symbol_list_style(&value);
                }
                "except" => {
                    except = symbol_list_value(&value);
                    except_style = symbol_list_style(&value);
                }
                _ => {}
            }
        }
    }

    Some(Filter { kind, target, only, except, only_style, except_style })
}
