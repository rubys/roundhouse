//! `devise_for :users[, controllers: { … }]` → a static Devise route table.
//!
//! Expands the **default four** Devise mappings — sessions, registrations,
//! passwords, confirmations — matching stock Devise helpers/paths. This is
//! not driven by the model's `devise :module, …` list; `skip:` / `only:`
//! fail loud instead of silently narrowing. Controllers default to
//! `devise/<mapping>` (`Devise::SessionsController`, …); `controllers:`
//! overrides replace per mapping, and keys outside the four modeled
//! mappings fail loud (no silent OmniAuth accept-then-ignore). Does not
//! model Warden, OmniAuth callback routes, or Devise controller bodies.
//! Unsupported options (`skip:`, `only:`, `path:`, `module:`, …) fail loud.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::dialect::{HttpMethod, ResourceScope, RouteSpec};
use crate::{ClassId, Symbol};

use super::routes::controller_class_name;
use super::util::{string_value, symbol_value};
use super::{IngestError, IngestResult};

/// One static Devise route. Paths are `/{plural}{path_suffix}`; helper
/// names substitute `{singular}` in `as_name`.
struct DeviseRoute {
    method: HttpMethod,
    /// Suffix after `/{plural}`; empty means the bare resource path.
    path_suffix: &'static str,
    mapping: &'static str,
    action: &'static str,
    as_name: &'static str,
}

/// Mappings this expansion emits routes for. Any other `controllers:`
/// key (e.g. `omniauth_callbacks`) is Unsupported — accepted into a map
/// and then ignored would hide a real gap.
const MODELED_MAPPINGS: &[&str] = &["sessions", "registrations", "passwords", "confirmations"];

const DEVISE_ROUTES: &[DeviseRoute] = &[
    // Sessions (database_authenticatable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/sign_in",
        mapping: "sessions",
        action: "new",
        as_name: "new_{singular}_session",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_suffix: "/sign_in",
        mapping: "sessions",
        action: "create",
        as_name: "{singular}_session",
    },
    DeviseRoute {
        method: HttpMethod::Delete,
        path_suffix: "/sign_out",
        mapping: "sessions",
        action: "destroy",
        as_name: "destroy_{singular}_session",
    },
    // Registrations (registerable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/sign_up",
        mapping: "registrations",
        action: "new",
        as_name: "new_{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/cancel",
        mapping: "registrations",
        action: "cancel",
        as_name: "cancel_{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_suffix: "",
        mapping: "registrations",
        action: "create",
        as_name: "{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/edit",
        mapping: "registrations",
        action: "edit",
        as_name: "edit_{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Patch,
        path_suffix: "",
        mapping: "registrations",
        action: "update",
        as_name: "{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Put,
        path_suffix: "",
        mapping: "registrations",
        action: "update",
        as_name: "{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Delete,
        path_suffix: "",
        mapping: "registrations",
        action: "destroy",
        as_name: "{singular}_registration",
    },
    // Passwords (recoverable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/password/new",
        mapping: "passwords",
        action: "new",
        as_name: "new_{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/password/edit",
        mapping: "passwords",
        action: "edit",
        as_name: "edit_{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_suffix: "/password",
        mapping: "passwords",
        action: "create",
        as_name: "{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Patch,
        path_suffix: "/password",
        mapping: "passwords",
        action: "update",
        as_name: "{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Put,
        path_suffix: "/password",
        mapping: "passwords",
        action: "update",
        as_name: "{singular}_password",
    },
    // Confirmations (confirmable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/confirmation/new",
        mapping: "confirmations",
        action: "new",
        as_name: "new_{singular}_confirmation",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_suffix: "/confirmation",
        mapping: "confirmations",
        action: "show",
        as_name: "{singular}_confirmation",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_suffix: "/confirmation",
        mapping: "confirmations",
        action: "create",
        as_name: "{singular}_confirmation",
    },
];

/// Expand `devise_for :users[, controllers: { sessions: "users/sessions", … }]`.
///
/// Returns a `Scope` holding every Explicit entry, or `Unsupported` when
/// the call carries options this expansion does not yet read (`skip:`,
/// `only:`, `path:`, `path_names:`, `as:`, `class_name:`, `module:`, …).
/// OmniAuth callback routes are intentionally omitted — providers are dynamic.
pub(super) fn ingest_devise_for(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<Option<RouteSpec>> {
    let Some(args_node) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "devise_for without a resource".into(),
        });
    };
    let mut resource: Option<String> = None;
    let mut controllers: HashMap<String, String> = HashMap::new();
    for arg in args_node.arguments().iter() {
        if let Some(name) = symbol_value(&arg) {
            if resource.is_none() {
                resource = Some(name);
                continue;
            }
        }
        if let Some(s) = string_value(&arg) {
            if resource.is_none() {
                resource = Some(s);
                continue;
            }
        }
        let Some(kh) = arg.as_keyword_hash_node() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "unsupported devise_for argument".into(),
            });
        };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "unsupported devise_for option key".into(),
                });
            };
            match key.as_str() {
                "controllers" => {
                    let value = assoc.value();
                    let elements: Vec<_> = if let Some(h) = value.as_hash_node() {
                        h.elements().iter().collect()
                    } else if let Some(kw) = value.as_keyword_hash_node() {
                        kw.elements().iter().collect()
                    } else {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: "devise_for controllers: must be a hash".into(),
                        });
                    };
                    for cel in elements {
                        let Some(ca) = cel.as_assoc_node() else {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: "devise_for controllers: unsupported entry".into(),
                            });
                        };
                        let Some(ck) =
                            symbol_value(&ca.key()).or_else(|| string_value(&ca.key()))
                        else {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: "devise_for controllers: key must be a symbol or string"
                                    .into(),
                            });
                        };
                        let Some(cv) = string_value(&ca.value()) else {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: format!(
                                    "devise_for controllers: {ck} must be a string path"
                                ),
                            });
                        };
                        if !MODELED_MAPPINGS.contains(&ck.as_str()) {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: format!(
                                    "unsupported devise_for controllers: `{ck}` \
                                     (modeled: sessions, registrations, passwords, confirmations)"
                                ),
                            });
                        }
                        controllers.insert(ck, cv);
                    }
                }
                other => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "unsupported devise_for option `{other}` (only `controllers:` is modeled)"
                        ),
                    });
                }
            }
        }
    }
    let Some(plural) = resource else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "devise_for without a resource".into(),
        });
    };
    let singular = crate::naming::singularize(&plural);
    let path_prefix = format!("/{plural}");
    // Devise::Mapping#default_controllers uses `#{module}/#{mapping}`
    // with module defaulting to "devise".
    let ctrl = |mapping: &str| -> String {
        controllers
            .get(mapping)
            .cloned()
            .unwrap_or_else(|| format!("devise/{mapping}"))
    };
    let mut resolved: HashMap<&str, String> = HashMap::new();
    for route in DEVISE_ROUTES {
        resolved
            .entry(route.mapping)
            .or_insert_with(|| ctrl(route.mapping));
    }
    let entries = DEVISE_ROUTES
        .iter()
        .map(|route| {
            let controller = resolved.get(route.mapping).expect("mapping pre-resolved");
            let as_name = route.as_name.replace("{singular}", &singular);
            RouteSpec::Explicit {
                method: route.method.clone(),
                path: format!("{path_prefix}{}", route.path_suffix),
                controller: ClassId(Symbol::from(controller_class_name(controller))),
                action: Symbol::from(route.action),
                as_name: Some(Symbol::from(as_name.as_str())),
                constraints: IndexMap::new(),
                scope: ResourceScope::Nested,
            }
        })
        .collect();
    // OmniAuth callbacks are provider-dynamic; we do not invent
    // `/auth/:provider` routes even when `controllers:` names one.
    Ok(Some(RouteSpec::Scope {
        path: None,
        module: None,
        as_prefix: None,
        defaults: IndexMap::new(),
        nest: false,
        entries,
    }))
}
