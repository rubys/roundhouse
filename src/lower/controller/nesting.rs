//! Resource-name and nested-parent resolution — `ArticlesController`
//! → `"article"`, and walking `routes.draw` for `resources :a do
//! resources :b end` shapes so emitters can generate `/a/:a_id/b/...`
//! paths and parent redirects.

use crate::App;
use crate::dialect::RouteSpec;
use crate::naming;

/// `ArticlesController` → `"article"`. `ApplicationController` →
/// `"application"`. Used to look up the `<resource>_params` helper
/// and to build default redirect paths.
pub fn resource_from_controller_name(class_name: &str) -> String {
    let trimmed = class_name.strip_suffix("Controller").unwrap_or(class_name);
    naming::singularize(&naming::snake_case(trimmed))
}

/// One nested-parent entry, carrying both forms for use in route
/// helpers and typed destinations. `singular` is the Ruby-style
/// singular ("article"); `plural` is the route segment
/// ("articles").
#[derive(Clone, Debug)]
pub struct NestedParent {
    pub singular: String,
    pub plural: String,
}

/// Walk the route table looking for a `resources :plural do resources
/// :child ... end` shape where `child` matches this controller's
/// resource. Returns the parent's (singular, plural) pair so the
/// emitter can emit `parent_id` path params and parent-redirects.
///
/// Recurses into nested blocks so deeper-than-two-level nesting
/// still resolves correctly.
pub fn find_nested_parent(app: &App, controller_class_name: &str) -> Option<NestedParent> {
    let resource = resource_from_controller_name(controller_class_name);
    let child_plural = naming::pluralize_snake(&naming::camelize(&resource));
    find_nested_parent_in(&app.routes.entries, &child_plural)
}

fn find_nested_parent_in(
    entries: &[RouteSpec],
    child_plural: &str,
) -> Option<NestedParent> {
    for entry in entries {
        if let RouteSpec::Resources { name, nested, .. } = entry {
            for child in nested {
                if let RouteSpec::Resources { name: child_name, .. } = child {
                    if child_name.as_str() == child_plural {
                        let parent_singular =
                            naming::singularize_camelize(name.as_str()).to_lowercase();
                        return Some(NestedParent {
                            singular: parent_singular,
                            plural: name.as_str().to_string(),
                        });
                    }
                }
            }
            if let Some(p) = find_nested_parent_in(nested, child_plural) {
                return Some(p);
            }
        }
    }
    None
}
