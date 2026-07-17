//! Gem / ecosystem catalog — declarative signatures for the
//! third-party gem surface a Rails app actually calls.
//!
//! ## Why a separate catalog
//!
//! Most compilers target a *language*. roundhouse nominally targets a
//! *framework* (Rails), but realistically it targets that framework's
//! *gem ecosystem* — and not by enumeration, but by **discovery**.
//! When an app calls `Arel.sql(...)` or `ROTP::TOTP#secret` and the
//! analyzer can't resolve the dispatch, the gem's signature lands
//! here. The set grows as real apps surface real calls.
//!
//! ## What an entry carries
//!
//! A concrete return type where one is knowable (`#secret -> Str`,
//! `ROTP::Base32.random -> Str`) and the gradual escape (`Untyped`)
//! for opaque gem objects we don't model structurally (`Arel.sql`'s
//! AST node, a parsed `Nokogiri` document). Either way the dispatch
//! *resolves* — never a hard `send_dispatch_failed`. `Untyped` is the
//! floor, not the ceiling: an entry is free to declare a precise type
//! the moment one is worth modeling.
//!
//! ## Shape
//!
//! Const data, same spirit as [`super::AR_CATALOG`]; registered into
//! the class registry in `Analyzer::new` via `register_stdlib_class`
//! (so a user class of the same name always wins). Centralized here
//! for now. Culling the list and admitting external, gem-author- or
//! user-supplied catalogs (an "enumerated federation") is deferred —
//! the data shape is the same either way, so externalization is a
//! loader, not a rewrite.

use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Const-friendly return-type descriptor (no `Box`/`Vec`), expanded
/// to a [`Ty`] at registry-build time — mirrors [`super::ReturnKind`]
/// for the AR catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GemTy {
    Str,
    Int,
    Bool,
    Float,
    Sym,
    /// Gradual escape: the gem returns an object we don't model
    /// structurally. Dispatch resolves and the choice propagates.
    Untyped,
    /// An instance of the named (dotted-path) class, so a *factory*
    /// method can return another cataloged gem type and chains
    /// resolve. (`.new` is already universal, so this is only for
    /// non-`new` constructors.)
    Instance(&'static str),
}

impl GemTy {
    pub fn to_ty(self) -> Ty {
        match self {
            GemTy::Str => Ty::Str,
            GemTy::Int => Ty::Int,
            GemTy::Bool => Ty::Bool,
            GemTy::Float => Ty::Float,
            GemTy::Sym => Ty::Sym,
            GemTy::Untyped => Ty::Untyped,
            GemTy::Instance(path) => Ty::Class {
                id: ClassId(Symbol::from(path)),
                args: vec![],
            },
        }
    }
}

/// One gem class/module's cataloged surface.
pub struct GemClass {
    /// Fully-qualified constant path as written in source
    /// (`"ROTP::TOTP"`, `"Mail::Address"`).
    pub name: &'static str,
    /// Methods called on the class/module itself (`Arel.sql`).
    pub class_methods: &'static [(&'static str, GemTy)],
    /// Methods called on an instance (`totp.secret`). The instance
    /// arrives through the universal `.new`, which already yields
    /// `Class { id: <name> }`, so these resolve without a factory
    /// entry.
    pub instance_methods: &'static [(&'static str, GemTy)],
}

/// The catalog. One entry per gem class/module; add a gem by adding a
/// row.
pub const GEM_CATALOG: &[GemClass] = &[
    // Faker — synthetic data. Production code shouldn't call it, but
    // lobsters' /cabinet dev-tools page renders sample content
    // inline; every generator returns a String.
    GemClass {
        name: "Faker::Internet",
        class_methods: &[
            ("url", GemTy::Str),
            ("user_name", GemTy::Str),
            ("email", GemTy::Str),
        ],
        instance_methods: &[],
    },
    GemClass {
        name: "Faker::Lorem",
        class_methods: &[
            ("sentence", GemTy::Str),
            ("paragraph", GemTy::Str),
            ("word", GemTy::Str),
        ],
        instance_methods: &[],
    },
    // Telebugs — error-reporting service (lobsters). Config/enrichment
    // calls made for side effect from controller filters; returns are
    // discarded at every corpus call site. The reporting itself is
    // ops-tooling outside the transpiled app's semantics (gem-fate:
    // facade), so resolving the calls as gradual is the whole job.
    GemClass {
        name: "Telebugs",
        class_methods: &[
            ("user", GemTy::Untyped),
            ("context", GemTy::Untyped),
            ("capture", GemTy::Untyped),
            ("report", GemTy::Untyped),
        ],
        instance_methods: &[],
    },
    // I18n — Rails' translation module, called as a bare singleton
    // (`I18n.t('admin.…')`) everywhere outside views (in views the
    // `t` helper delegates here). `t`/`l` return the translated /
    // localized string; the locale accessors return the locale
    // Symbol. Mastodon alone has 110+ `I18n.t` controller call sites.
    GemClass {
        name: "I18n",
        class_methods: &[
            ("t", GemTy::Str),
            ("t!", GemTy::Str),
            ("translate", GemTy::Str),
            ("l", GemTy::Str),
            ("localize", GemTy::Str),
            ("locale", GemTy::Sym),
            ("default_locale", GemTy::Sym),
            // `with_locale` returns its block's value — opaque here.
            ("with_locale", GemTy::Untyped),
            ("available_locales", GemTy::Untyped),
            ("exists?", GemTy::Bool),
        ],
        instance_methods: &[],
    },
    // ActiveRecord::Promise — the handle returned by the async query
    // surface (`relation.async_count`, `async_sum`, …; see
    // `array_method`'s relation branch). `value` blocks until the
    // background query resolves; its type depends on the originating
    // call, so gradual.
    GemClass {
        name: "ActiveRecord::Promise",
        class_methods: &[],
        instance_methods: &[("value", GemTy::Untyped), ("pending?", GemTy::Bool)],
    },
    // Arel — ActiveRecord's low-level SQL AST builder. `sql` wraps a
    // raw fragment, `star` is the `*` projection node; both produce
    // opaque AST consumed by where/order/select, so gradual.
    GemClass {
        name: "Arel",
        class_methods: &[("sql", GemTy::Untyped), ("star", GemTy::Untyped)],
        instance_methods: &[],
    },
    // Addressable — RFC-conformant URI parsing (Mastodon's URL
    // handling). `parse`/`heuristic_parse` produce a URI instance;
    // the read surface is string-shaped.
    GemClass {
        name: "Addressable::URI",
        class_methods: &[
            ("parse", GemTy::Instance("Addressable::URI")),
            ("heuristic_parse", GemTy::Instance("Addressable::URI")),
        ],
        instance_methods: &[
            ("to_s", GemTy::Str),
            ("host", GemTy::Str),
            ("path", GemTy::Str),
            ("scheme", GemTy::Str),
            // `normalize` returns a normalized copy — chains keep the type.
            ("normalize", GemTy::Instance("Addressable::URI")),
            // `query`/`fragment` are nil on absence — gradual floor.
            ("query", GemTy::Untyped),
            ("normalized_host", GemTy::Untyped),
        ],
    },
    // ROTP — TOTP/HOTP one-time passwords (the 2FA surface).
    GemClass {
        name: "ROTP::TOTP",
        class_methods: &[],
        instance_methods: &[
            ("secret", GemTy::Str),
            ("provisioning_uri", GemTy::Str),
            ("now", GemTy::Str),
            ("at", GemTy::Str),
            // `verify` returns the matching timestamp Integer or nil —
            // gradual rather than committing to `Int?`.
            ("verify", GemTy::Untyped),
        ],
    },
    GemClass {
        name: "ROTP::Base32",
        class_methods: &[("random", GemTy::Str), ("random_base32", GemTy::Str)],
        instance_methods: &[],
    },
    // RQRCode — QR-code rendering; the `as_*` methods serialize to a
    // String in the requested format.
    GemClass {
        name: "RQRCode::QRCode",
        class_methods: &[],
        instance_methods: &[
            ("as_svg", GemTy::Str),
            ("as_png", GemTy::Str),
            ("as_ansi", GemTy::Str),
        ],
    },
    // Nokogiri — HTML/XML parsing. The `HTML`/`XML` module methods
    // (`Nokogiri::HTML(str)`) return a Document we don't model.
    GemClass {
        name: "Nokogiri",
        class_methods: &[("HTML", GemTy::Untyped), ("XML", GemTy::Untyped)],
        instance_methods: &[],
    },
    // pdf-reader — PDF metadata/text extraction.
    GemClass {
        name: "PDF::Reader",
        class_methods: &[],
        instance_methods: &[
            ("info", GemTy::Untyped),
            ("pages", GemTy::Untyped),
            ("page_count", GemTy::Int),
        ],
    },
    // pushover — push notifications. `SUBSCRIPTION_CODE` is a config
    // accessor (truthiness-checked), `subscription_url` builds a URL.
    GemClass {
        name: "Pushover",
        class_methods: &[
            ("SUBSCRIPTION_CODE", GemTy::Untyped),
            ("subscription_url", GemTy::Str),
            ("notification", GemTy::Untyped),
        ],
        instance_methods: &[],
    },
    // mail — RFC822 address parsing; the parts are Strings.
    GemClass {
        name: "Mail::Address",
        class_methods: &[],
        instance_methods: &[
            ("address", GemTy::Str),
            ("domain", GemTy::Str),
            ("local", GemTy::Str),
        ],
    },
    // rack-mini-profiler — dev profiling middleware; the gate call is
    // side-effecting.
    GemClass {
        name: "Rack::MiniProfiler",
        class_methods: &[("authorize_request", GemTy::Untyped)],
        instance_methods: &[],
    },
];
