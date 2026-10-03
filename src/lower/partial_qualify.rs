//! Qualify BARE partial names through the controller ancestry — Rails'
//! template lookup prefixes. `render partial: 'subnav'` in
//! mod_notes/index resolves against ["mod_notes", "mod", …] because
//! ModNotesController < ModController; our resolution machinery
//! (partial keys, ivar closures, locals contracts, the dispatch emit)
//! is all own-dir-keyed, so the bare name pointed at a
//! Views::ModNotes.subnav that doesn't exist. Rewriting the literal to
//! its qualified spelling ("mod/subnav") ONCE, before any of that
//! machinery runs, fixes every consumer at the same time.
//!
//! Conservative: only bare literals whose own-dir partial does NOT
//! exist, resolved strictly up the controller parent chain to the
//! first dir that has the partial. A controller under
//! ApplicationController reaches `application/` through that chain.
//! Everything else stays as written.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::naming::underscore;

pub fn apply_partial_qualification(app: &mut App) {
    use std::collections::{HashMap, HashSet};

    // Every (dir, stem) a partial exists at.
    let existing: HashSet<(String, String)> = app
        .views
        .iter()
        .filter_map(|v| {
            let (dir, base) = v.name.as_str().rsplit_once('/')?;
            let stem = base.strip_prefix('_')?;
            let stem = stem.split('.').next().unwrap_or(stem);
            Some((dir.to_string(), stem.to_string()))
        })
        .collect();

    // View-dir → parent view-dir, via the controller class chain
    // (mod_notes → ModNotesController < ModController → mod).
    let parent_dir: HashMap<String, String> = app
        .controllers
        .iter()
        .filter_map(|c| {
            let dir = underscore(c.name.0.as_str().strip_suffix("Controller")?);
            let parent = c.parent.as_ref()?;
            let pdir = underscore(parent.0.as_str().strip_suffix("Controller")?);
            Some((dir, pdir))
        })
        .collect();

    for view in &mut app.views {
        let Some((dir, _)) = view.name.as_str().rsplit_once('/') else { continue };
        let dir = dir.to_string();
        qualify(&mut view.body, &dir, &existing, &parent_dir);
    }
}

fn qualify(
    expr: &mut Expr,
    own_dir: &str,
    existing: &std::collections::HashSet<(String, String)>,
    parent_dir: &std::collections::HashMap<String, String>,
) {
    expr.node
        .for_each_child_mut(&mut |c| qualify(c, own_dir, existing, parent_dir));
    let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
    if method.as_str() != "render" && method.as_str() != "render_to_string" {
        return;
    }
    let Some(first) = args.first_mut() else { return };
    // `render "name", k: v` names the partial in the first argument;
    // `render partial: "name"` names it under the `partial:` key.
    let names: Vec<&mut String> = match &mut *first.node {
        ExprNode::Lit { value: Literal::Str { value } } => vec![value],
        ExprNode::Hash { entries, kwargs: true } => entries
            .iter_mut()
            .filter(|(k, _)| {
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                    if value.as_str() == "partial")
            })
            .filter_map(|(_, v)| match &mut *v.node {
                ExprNode::Lit { value: Literal::Str { value } } => Some(value),
                _ => None,
            })
            .collect(),
        _ => return,
    };
    for value in names {
        if value.contains('/') {
            continue;
        }
        if existing.contains(&(own_dir.to_string(), value.clone())) {
            continue;
        }
        // Walk up the controller chain to the first dir carrying the
        // partial. A cycle or a miss leaves the name as written.
        let mut d = own_dir.to_string();
        let mut hops = 0;
        while let Some(p) = parent_dir.get(&d) {
            hops += 1;
            if hops > 8 {
                break;
            }
            if existing.contains(&(p.clone(), value.clone())) {
                *value = format!("{p}/{value}");
                break;
            }
            d = p.clone();
        }
    }
}
