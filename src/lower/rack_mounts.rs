//! Ordered prefix dispatch for Rack endpoints. A mount is not a controller
//! route: its remainder includes dots, and its response is already a Rack
//! triple. Keep the ordinary routes before each mount so a prefix cannot
//! steal an earlier controller route.

use super::routes::{FlatRoute, flatten_route_entries};
use crate::dialect::RouteSpec;
use crate::{App, ClassId};

pub(crate) struct RackMount {
    pub target: ClassId,
    pub prefix: String,
    pub preceding: Vec<FlatRoute>,
    pub span: crate::span::Span,
}

pub(crate) fn mounts(app: &App) -> Vec<RackMount> {
    fn contains_mount(entries: &[RouteSpec]) -> bool {
        entries.iter().any(|entry| match entry {
            RouteSpec::RackMount { .. } => true,
            RouteSpec::Scope { entries, .. } => contains_mount(entries),
            _ => false,
        })
    }
    if !contains_mount(&app.routes.entries) {
        return Vec::new();
    }
    fn walk(entries: &[RouteSpec], preceding: &mut Vec<FlatRoute>, out: &mut Vec<RackMount>) {
        for entry in entries {
            match entry {
                RouteSpec::RackMount { target, path, span } => out.push(RackMount {
                    target: target.clone(),
                    prefix: path.clone(),
                    preceding: preceding.clone(),
                    span: *span,
                }),
                // A split draw can return a transparent scope. Admission
                // refuses mounts inside path/constraint/namespace wrappers.
                RouteSpec::Scope {
                    path: None,
                    module: None,
                    as_prefix: None,
                    entries,
                    ..
                } if contains_mount(entries) => {
                    walk(entries, preceding, out);
                }
                _ => preceding.extend(
                    flatten_route_entries(std::slice::from_ref(entry))
                        .into_iter()
                        .filter(|route| route.refused.is_none()),
                ),
            }
        }
    }
    let mut out = Vec::new();
    walk(&app.routes.entries, &mut Vec::new(), &mut out);
    out
}
