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
//! first dir that has the partial. Everything else stays as written.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::naming::snake_case;

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
            let dir = snake_case(c.name.0.as_str().strip_suffix("Controller")?);
            let parent = c.parent.as_ref()?;
            let pdir = snake_case(parent.0.as_str().strip_suffix("Controller")?);
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
    let ExprNode::Hash { entries, kwargs: true } = &mut *first.node else { return };
    for (k, v) in entries {
        if !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "partial")
        {
            continue;
        }
        let ExprNode::Lit { value: Literal::Str { value } } = &mut *v.node else { continue };
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
