//! `devise_for :users[, path:, path_names:, format:, controllers: { … }]`
//! → a static Devise route table.
//!
//! Expands the **default four** Devise mappings — sessions, registrations,
//! passwords, confirmations — matching stock Devise helpers/paths. This is
//! not driven by the model's `devise :module, …` list; `skip:` / `only:`
//! fail loud instead of silently narrowing. Controllers default to
//! `devise/<mapping>` (`devise/sessions`, …) the way
//! `Devise::Mapping#default_controllers` does (`mod = options[:module] ||
//! @scope[:module] || "devise"`) — route lowering substitutes the
//! enclosing `scope module:`, if any, for `"devise"`; see
//! `resolve_default_controller` and its caller in `lower::routes`.
//! `controllers:` overrides replace per mapping and are absolute (Devise
//! resets the scope's module to `nil` before applying them —
//! `with_devise_exclusive_scope` — so no enclosing scope ever re-qualifies
//! them). Keys outside the four modeled mappings fail loud (no silent
//! OmniAuth accept-then-ignore). Does not model Warden, OmniAuth callback
//! routes, or Devise controller bodies. Unsupported options (`skip:`,
//! `only:`, `as:`, `class_name:`, `module:`, …) fail loud.

use crate::det::HashMap;

use indexmap::IndexMap;

use crate::dialect::{HttpMethod, ResourceScope, RouteSpec};
use crate::{ClassId, Symbol};

use super::routes::controller_class_name;
use super::util::{bool_value, string_value, symbol_or_string_value, symbol_value};
use super::{IngestError, IngestResult};

/// One static Devise route. Paths are `/{plural}{suffix}`; `suffix` is a
/// template over `path_names` keys (`"{sign_in}"`, …) rendered by
/// [`render_suffix`]. Helper names substitute `{singular}` in `as_name`.
struct DeviseRoute {
    method: HttpMethod,
    /// Suffix template after `/{plural}` (or `/{path}` under `path:`).
    path_template: &'static str,
    mapping: &'static str,
    action: &'static str,
    as_name: &'static str,
}

/// Mappings this expansion emits routes for. Any other `controllers:`
/// key (e.g. `omniauth_callbacks`) is Unsupported — accepted into a map
/// and then ignored would hide a real gap.
const MODELED_MAPPINGS: &[&str] = &["sessions", "registrations", "passwords", "confirmations"];

/// `Devise::Mapping#default_path_names` — every key falls back to its
/// own name except `registration`, which defaults empty (the
/// registration sub-resource is mounted at the mapping's own path, not
/// a nested segment).
fn default_path_names() -> HashMap<String, String> {
    [
        ("sign_in", "sign_in"),
        ("sign_out", "sign_out"),
        ("sign_up", "sign_up"),
        ("edit", "edit"),
        ("cancel", "cancel"),
        ("password", "password"),
        ("confirmation", "confirmation"),
        ("registration", ""),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Substitute `{key}` placeholders from `names`, then collapse the
/// doubled/trailing slashes a blank `registration` default leaves
/// behind (`"/{registration}/{sign_up}"` → `"//sign_up"` → `"/sign_up"`).
fn render_suffix(template: &str, names: &HashMap<String, String>) -> String {
    let mut s = template.to_string();
    for (k, v) in names {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    let mut out = String::new();
    let mut prev_slash = false;
    for c in s.chars() {
        if c == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        out.push(c);
    }
    if out == "/" {
        String::new()
    } else {
        out.trim_end_matches('/').to_string()
    }
}

/// Join the `path:` segment with a route's rendered suffix the way
/// `"/#{path_prefix}/#{@path}".squeeze("/")` does in Devise's own
/// `Mapping#fullpath` — a leading/trailing slash on `path:` itself
/// (`path: "/auth"`, `path: "/auth/"`) and an empty `path:`
/// (`path: ""`, the registration root, or Devise's own default
/// `path_names[:registration]`) must not leave a doubled `//` or an
/// empty path where Devise gives `/`.
fn join_devise_path(segment: &str, suffix: &str) -> String {
    let segment = segment.trim_matches('/');
    let joined = format!("{segment}{suffix}");
    if joined.is_empty() {
        "/".to_string()
    } else if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

const DEVISE_ROUTES: &[DeviseRoute] = &[
    // Sessions (database_authenticatable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{sign_in}",
        mapping: "sessions",
        action: "new",
        as_name: "new_{singular}_session",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_template: "/{sign_in}",
        mapping: "sessions",
        action: "create",
        as_name: "{singular}_session",
    },
    DeviseRoute {
        method: HttpMethod::Delete,
        path_template: "/{sign_out}",
        mapping: "sessions",
        action: "destroy",
        as_name: "destroy_{singular}_session",
    },
    // Registrations (registerable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{registration}/{sign_up}",
        mapping: "registrations",
        action: "new",
        as_name: "new_{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{registration}/{cancel}",
        mapping: "registrations",
        action: "cancel",
        as_name: "cancel_{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_template: "/{registration}",
        mapping: "registrations",
        action: "create",
        as_name: "{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{registration}/{edit}",
        mapping: "registrations",
        action: "edit",
        as_name: "edit_{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Patch,
        path_template: "/{registration}",
        mapping: "registrations",
        action: "update",
        as_name: "{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Put,
        path_template: "/{registration}",
        mapping: "registrations",
        action: "update",
        as_name: "{singular}_registration",
    },
    DeviseRoute {
        method: HttpMethod::Delete,
        path_template: "/{registration}",
        mapping: "registrations",
        action: "destroy",
        as_name: "{singular}_registration",
    },
    // Passwords (recoverable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{password}/new",
        mapping: "passwords",
        action: "new",
        as_name: "new_{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{password}/edit",
        mapping: "passwords",
        action: "edit",
        as_name: "edit_{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_template: "/{password}",
        mapping: "passwords",
        action: "create",
        as_name: "{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Patch,
        path_template: "/{password}",
        mapping: "passwords",
        action: "update",
        as_name: "{singular}_password",
    },
    DeviseRoute {
        method: HttpMethod::Put,
        path_template: "/{password}",
        mapping: "passwords",
        action: "update",
        as_name: "{singular}_password",
    },
    // Confirmations (confirmable)
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{confirmation}/new",
        mapping: "confirmations",
        action: "new",
        as_name: "new_{singular}_confirmation",
    },
    DeviseRoute {
        method: HttpMethod::Get,
        path_template: "/{confirmation}",
        mapping: "confirmations",
        action: "show",
        as_name: "{singular}_confirmation",
    },
    DeviseRoute {
        method: HttpMethod::Post,
        path_template: "/{confirmation}",
        mapping: "confirmations",
        action: "create",
        as_name: "{singular}_confirmation",
    },
];

/// Sentinel for a DEFAULT devise_for controller (no `controllers:`
/// override): lower-case with a leading NUL, which no camelized
/// `FooController` class name can look like, so route lowering can tell
/// it apart from an ordinary already-qualified controller and
/// substitute the enclosing scope's module for Devise's own `"devise"`
/// default instead of concatenating onto it. See `resolve_default_controller`.
const DEFAULT_MARKER_PREFIX: &str = "\u{0}devise-default:";

fn default_controller_marker(mapping: &str) -> String {
    format!("{DEFAULT_MARKER_PREFIX}{mapping}")
}

/// Resolve a [`default_controller_marker`] placeholder against the
/// enclosing scope's accumulated module prefix (`"Auth::"`, already
/// `Camel::`-joined with a trailing `::`, or empty for no enclosing
/// module). Mirrors `Devise::Mapping#default_controllers`: `mod =
/// options[:module] || @scope[:module] || "devise"` — we only ever see
/// the scope's module here, since `devise_for … module:` itself fails
/// loud at ingest. Returns `None` when `controller` is not one of our
/// placeholders, so the caller falls through to ordinary qualification.
pub(crate) fn resolve_default_controller(module_prefix: &str, controller: &str) -> Option<String> {
    let mapping = controller.strip_prefix(DEFAULT_MARKER_PREFIX)?;
    let module = if module_prefix.is_empty() {
        "Devise"
    } else {
        module_prefix.trim_end_matches("::")
    };
    Some(format!("{module}::{}Controller", crate::naming::camelize(mapping)))
}

/// Expand `devise_for :users[, path:, path_names:, format:, controllers:
/// { sessions: "users/sessions", … }]`.
///
/// Returns a `Scope` holding every Explicit entry, or `Unsupported` when
/// the call carries options this expansion does not yet read (`skip:`,
/// `only:`, `as:`, `class_name:`, `module:`, …).
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
    let mut controllers: HashMap<String, String> = HashMap::default();
    let mut path: Option<String> = None;
    let mut path_name_overrides: HashMap<String, String> = HashMap::default();
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
                // `Devise::Mapping#initialize`: `@path = (options[:path]
                // || name).to_s` — the path segment, independent of the
                // singular used for helper names.
                "path" => {
                    let Some(v) = string_value(&assoc.value())
                        .or_else(|| symbol_value(&assoc.value()))
                    else {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: "devise_for path: must be a string or symbol".into(),
                        });
                    };
                    path = Some(v);
                }
                // `Devise::Mapping#default_path_names`, merged over the
                // stock defaults (`sign_in`, `sign_out`, `sign_up`,
                // `edit`, `cancel`, `password`, `confirmation`,
                // `registration`). Keys outside the modeled mappings
                // (e.g. `unlock:`) are accepted but unused, matching
                // Devise's own unvalidated hash.
                "path_names" => {
                    let value = assoc.value();
                    let elements: Vec<_> = if let Some(h) = value.as_hash_node() {
                        h.elements().iter().collect()
                    } else if let Some(kw) = value.as_keyword_hash_node() {
                        kw.elements().iter().collect()
                    } else {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: "devise_for path_names: must be a hash".into(),
                        });
                    };
                    for nel in elements {
                        let Some(na) = nel.as_assoc_node() else {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: "devise_for path_names: unsupported entry".into(),
                            });
                        };
                        let Some(nk) = symbol_or_string_value(&na.key()) else {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: "devise_for path_names: key must be a symbol or string"
                                    .into(),
                            });
                        };
                        let Some(nv) = string_value(&na.value()) else {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: format!(
                                    "devise_for path_names: {nk} must be a string"
                                ),
                            });
                        };
                        path_name_overrides.insert(nk, nv);
                    }
                }
                // `format: false` drops the `(.:format)` optional
                // suffix Rails appends to ordinary resource routes —
                // which is also what this table already models for
                // every devise_for call, `format:` or not. `format:
                // true` would need the opposite (and isn't modeled),
                // so it fails loud rather than silently matching the
                // `false` behavior.
                "format" => {
                    let Some(v) = bool_value(&assoc.value()) else {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: "devise_for format: must be true or false".into(),
                        });
                    };
                    if v {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: "devise_for format: true is not modeled (routes already \
                                      omit the optional format suffix)"
                                .into(),
                        });
                    }
                }
                other => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "unsupported devise_for option `{other}` (modeled: `controllers:`, \
                             `path:`, `path_names:`, `format:`)"
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
    // `@singular = scoped_path.tr('/', '_').singularize` where
    // `scoped_path` defaults to `name.to_s` (we don't model `as:`, so
    // this is always the plain singularize) — derived from the
    // RESOURCE name, independent of a `path:` override.
    let singular = crate::naming::singularize(&plural);
    let base_path = path.unwrap_or_else(|| plural.clone());
    let mut names = default_path_names();
    names.extend(path_name_overrides);
    // `Devise::Mapping#default_controllers`: `mod = options[:module] ||
    // "devise"`; an explicit `controllers:` entry is absolute (Devise
    // resets the scope's module before applying it), marked here with a
    // leading `::` the way `qualify_controller` already treats any
    // other absolute target. The default (no override) is left as a
    // placeholder route lowering resolves against the enclosing scope's
    // module — see `resolve_default_controller`.
    let ctrl = |mapping: &str| -> String {
        match controllers.get(mapping) {
            Some(cv) => format!("::{}", controller_class_name(cv)),
            None => default_controller_marker(mapping),
        }
    };
    let mut resolved: HashMap<&str, String> = HashMap::default();
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
            let suffix = render_suffix(route.path_template, &names);
            RouteSpec::Explicit {
                method: route.method.clone(),
                path: join_devise_path(&base_path, &suffix),
                controller: ClassId(Symbol::from(controller.clone())),
                action: Symbol::from(route.action),
                as_name: Some(Symbol::from(as_name.as_str())),
                constraints: IndexMap::new(),
                scope: ResourceScope::Nested,
                action_symbol: false,
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
        suppress_helpers: false,
        entries,
    }))
}
