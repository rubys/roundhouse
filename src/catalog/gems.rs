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
                args: vec![].into(),
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
            ("locale_available?", GemTy::Bool),
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
    // Thin surface for `Model.reflect_on_association(:name)` — enough
    // for ActionText / attachment helpers that compare `.klass` or
    // ask presence. Full reflection stays unmodeled (Untyped on the
    // deeper readers).
    GemClass {
        name: "ActiveRecord::Reflection::AssociationReflection",
        class_methods: &[],
        instance_methods: &[
            ("klass", GemTy::Untyped),
            ("name", GemTy::Sym),
            ("macro", GemTy::Sym),
            // Nil when the association did not declare `counter_cache:`
            // (campfire's Message→Room). Untyped until option ingest lands.
            ("counter_cache_column", GemTy::Untyped),
        ],
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
            // The step in seconds, 30 unless `interval:` named another;
            // lobsters widens `verify`'s drift by it.
            ("interval", GemTy::Int),
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
    // htmlentities — entity encode/decode; both directions return the
    // converted String (lobsters' `HtmlEncoder` wraps one instance).
    GemClass {
        name: "HTMLEntities",
        class_methods: &[],
        instance_methods: &[("encode", GemTy::Str), ("decode", GemTy::Str)],
    },
    // commonmarker 2.x — lobsters' Markdowner parses to a node tree,
    // edits it, and renders it back. The gem on the ruby family,
    // spinel-commonmarker (same surface over cmark-gfm) on spinel.
    // Node links answer a Node or nil, and the editing calls answer
    // what the gem's C side does; those stay gradual.
    GemClass {
        name: "Commonmarker",
        class_methods: &[
            ("parse", GemTy::Instance("Commonmarker::Node")),
            ("to_html", GemTy::Str),
        ],
        instance_methods: &[],
    },
    GemClass {
        name: "Commonmarker::Node",
        class_methods: &[],
        instance_methods: &[
            ("type", GemTy::Sym),
            ("string_content", GemTy::Str),
            ("string_content=", GemTy::Str),
            ("to_html", GemTy::Str),
            ("to_commonmark", GemTy::Str),
            ("first_child", GemTy::Untyped),
            ("insert_before", GemTy::Untyped),
            ("insert_after", GemTy::Untyped),
            ("append_child", GemTy::Untyped),
            ("delete", GemTy::Untyped),
            ("walk", GemTy::Untyped),
            ("each", GemTy::Untyped),
        ],
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
    // bcrypt — what `has_secure_password` is built on, and what an app
    // reaches for directly when it hashes something that is not a
    // password (the Rails tutorial's remember/activation/reset tokens:
    // `BCrypt::Password.create(string, cost:)` and
    // `BCrypt::Password.new(digest).is_password?(token)`).
    GemClass {
        name: "BCrypt::Password",
        class_methods: &[
            ("create", GemTy::Instance("BCrypt::Password")),
            ("valid_hash?", GemTy::Bool),
        ],
        instance_methods: &[
            ("is_password?", GemTy::Bool),
            ("==", GemTy::Bool),
            ("to_s", GemTy::Str),
            ("to_str", GemTy::Str),
            ("cost", GemTy::Int),
            ("salt", GemTy::Str),
            ("checksum", GemTy::Str),
            ("version", GemTy::Str),
        ],
    },
    GemClass {
        name: "BCrypt::Engine",
        class_methods: &[
            ("cost", GemTy::Int),
            ("cost=", GemTy::Int),
            ("generate_salt", GemTy::Str),
            ("hash_secret", GemTy::Str),
            ("calibrate", GemTy::Int),
            ("valid_salt?", GemTy::Bool),
            ("valid_secret?", GemTy::Bool),
        ],
        instance_methods: &[],
    },
    // `ActiveModel::SecurePassword.min_cost` — the test-environment
    // switch the tutorial reads to pick a bcrypt cost.
    GemClass {
        name: "ActiveModel::SecurePassword",
        class_methods: &[("min_cost", GemTy::Bool), ("min_cost=", GemTy::Bool)],
        instance_methods: &[],
    },
    GemClass {
        name: "Nokogiri",
        class_methods: &[
            ("HTML", GemTy::Untyped),
            ("HTML4", GemTy::Untyped),
            ("HTML5", GemTy::Untyped),
            ("XML", GemTy::Untyped),
        ],
        instance_methods: &[],
    },
    // Nokogiri's HTML5 parser (gumbo, vendored by the gem and carried by
    // spinel-nokogiri): `Nokogiri::HTML5.fragment(html)` and the module's
    // Document/DocumentFragment classes. The parsed tree stays unmodeled,
    // as `Nokogiri::HTML`'s does.
    GemClass {
        name: "Nokogiri::HTML5",
        class_methods: &[("parse", GemTy::Untyped), ("fragment", GemTy::Untyped)],
        instance_methods: &[],
    },
    GemClass {
        name: "Nokogiri::HTML5::Document",
        class_methods: &[("parse", GemTy::Untyped)],
        instance_methods: &[],
    },
    GemClass {
        name: "Nokogiri::HTML5::DocumentFragment",
        class_methods: &[("parse", GemTy::Untyped)],
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
    // pay — Stripe/Braintree/Paddle billing (Jumpstart Pro). First slice:
    // exception classes for `rescue Pay::Error` / `Pay::ActionRequired`,
    // and the model class constants for `is_a?(Pay::Charge)` /
    // `is_a?(Pay::Subscription)`. Not ActiveRecord: `.all` / scopes /
    // `payment_processor` stay **errors** (attribution skips
    // unknown-gem downgrade for any `GEM_CATALOG` receiver — see
    // `catalogued_class_receiver`). The gem stays **Unknown** in the
    // census so *other* Pay::* names (Currency, PaddleClassic, …)
    // remain gap-attributed notes rather than invented errors.
    // Runtime: `runtime/ruby/pay.rb` façade (see `apply_pay_demand`).
    GemClass {
        name: "Pay",
        class_methods: &[],
        instance_methods: &[],
    },
    GemClass {
        name: "Pay::Error",
        class_methods: &[],
        instance_methods: &[("message", GemTy::Str), ("to_s", GemTy::Str)],
    },
    GemClass {
        name: "Pay::ActionRequired",
        class_methods: &[],
        instance_methods: &[("message", GemTy::Str), ("to_s", GemTy::Str)],
    },
    GemClass {
        name: "Pay::Charge",
        class_methods: &[],
        instance_methods: &[],
    },
    GemClass {
        name: "Pay::Subscription",
        class_methods: &[],
        instance_methods: &[],
    },
    GemClass {
        name: "Pay::Customer",
        class_methods: &[],
        instance_methods: &[],
    },
    // Faraday — the HTTP client. Ruby and JRuby run the real gem (the
    // emitted Gemfile declares it on demand, `gems::RUNTIME_GEMS`);
    // every other target refuses each reference (`lower::ruby_family_gems`).
    // Responses and bodies are the gem's own objects, so they stay
    // `Untyped` past the shapes below.
    GemClass {
        name: "Faraday",
        class_methods: &[
            ("new", GemTy::Instance("Faraday::Connection")),
            ("default_adapter", GemTy::Sym),
            ("default_adapter=", GemTy::Sym),
            ("get", GemTy::Instance("Faraday::Response")),
            ("post", GemTy::Instance("Faraday::Response")),
            ("put", GemTy::Instance("Faraday::Response")),
            ("patch", GemTy::Instance("Faraday::Response")),
            ("delete", GemTy::Instance("Faraday::Response")),
            ("head", GemTy::Instance("Faraday::Response")),
        ],
        instance_methods: &[],
    },
    GemClass {
        name: "Faraday::Connection",
        class_methods: &[],
        instance_methods: &[
            ("get", GemTy::Instance("Faraday::Response")),
            ("post", GemTy::Instance("Faraday::Response")),
            ("put", GemTy::Instance("Faraday::Response")),
            ("patch", GemTy::Instance("Faraday::Response")),
            ("delete", GemTy::Instance("Faraday::Response")),
            ("head", GemTy::Instance("Faraday::Response")),
            ("run_request", GemTy::Instance("Faraday::Response")),
            ("request", GemTy::Untyped),
            ("response", GemTy::Untyped),
            ("use", GemTy::Untyped),
            ("adapter", GemTy::Untyped),
            ("headers", GemTy::Untyped),
            ("params", GemTy::Untyped),
            ("options", GemTy::Instance("Faraday::RequestOptions")),
            ("url_prefix", GemTy::Untyped),
            ("set_basic_auth", GemTy::Untyped),
            ("builder", GemTy::Untyped),
        ],
    },
    GemClass {
        name: "Faraday::RequestOptions",
        class_methods: &[],
        instance_methods: &[
            ("timeout", GemTy::Untyped),
            ("timeout=", GemTy::Untyped),
            ("open_timeout", GemTy::Untyped),
            ("open_timeout=", GemTy::Untyped),
            ("read_timeout", GemTy::Untyped),
            ("read_timeout=", GemTy::Untyped),
            ("write_timeout", GemTy::Untyped),
            ("write_timeout=", GemTy::Untyped),
            ("params_encoder", GemTy::Untyped),
            ("params_encoder=", GemTy::Untyped),
        ],
    },
    GemClass {
        name: "Faraday::Response",
        class_methods: &[],
        instance_methods: &[
            ("status", GemTy::Int),
            ("body", GemTy::Untyped),
            ("headers", GemTy::Untyped),
            ("success?", GemTy::Bool),
            ("finished?", GemTy::Bool),
            ("reason_phrase", GemTy::Untyped),
            ("env", GemTy::Untyped),
            ("to_hash", GemTy::Untyped),
        ],
    },
    // Values an app hands the connection: an encoder, a middleware class.
    GemClass { name: "Faraday::FlatParamsEncoder", class_methods: &[], instance_methods: &[] },
    GemClass { name: "Faraday::NestedParamsEncoder", class_methods: &[], instance_methods: &[] },
    GemClass { name: "Faraday::Response::RaiseError", class_methods: &[], instance_methods: &[] },
    // faraday-multipart: the parts a multipart body carries, and the
    // Faraday v1 names it still aliases (`Faraday::UploadIO`, `FilePart`).
    GemClass { name: "Faraday::Multipart", class_methods: &[], instance_methods: &[] },
    GemClass { name: "Faraday::Multipart::FilePart", class_methods: &[], instance_methods: FARADAY_FILE_PART },
    GemClass { name: "Faraday::Multipart::ParamPart", class_methods: &[], instance_methods: &[] },
    GemClass { name: "Faraday::Multipart::CompositeReadIO", class_methods: &[], instance_methods: &[] },
    GemClass { name: "Faraday::FilePart", class_methods: &[], instance_methods: FARADAY_FILE_PART },
    GemClass { name: "Faraday::ParamPart", class_methods: &[], instance_methods: &[] },
    GemClass { name: "Faraday::UploadIO", class_methods: &[], instance_methods: FARADAY_FILE_PART },
    GemClass { name: "Faraday::CompositeReadIO", class_methods: &[], instance_methods: &[] },
    // The error hierarchy an app rescues. `response` is the Hash the gem
    // builds (`status`, `body`, `headers`), or nil for a connection error.
    GemClass { name: "Faraday::Error", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ClientError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ServerError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::BadRequestError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::UnauthorizedError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ForbiddenError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ResourceNotFound", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ConflictError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::UnprocessableEntityError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::TooManyRequestsError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ConnectionFailed", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::TimeoutError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::SSLError", class_methods: &[], instance_methods: FARADAY_ERROR },
    GemClass { name: "Faraday::ParsingError", class_methods: &[], instance_methods: FARADAY_ERROR },
    // Retryable — a block retried on an exception. As Faraday: the real
    // gem on ruby and jruby, refused elsewhere (`lower::ruby_family_gems`).
    GemClass {
        name: "Retryable",
        class_methods: &[("retryable", GemTy::Untyped)],
        instance_methods: &[],
    },
    // JWT — token encode/decode (the jwt gem). As Faraday: the real gem
    // on ruby and jruby, refused elsewhere (`lower::ruby_family_gems`).
    // `decode` answers `[payload, header]`, both the gem's Hashes.
    GemClass {
        name: "JWT",
        class_methods: &[("encode", GemTy::Str), ("decode", GemTy::Untyped)],
        instance_methods: &[],
    },
    GemClass {
        name: "JWT::Claims",
        class_methods: &[("verify_payload!", GemTy::Untyped), ("valid_payload?", GemTy::Bool)],
        instance_methods: &[],
    },
    GemClass { name: "JWT::JWK", class_methods: &[], instance_methods: &[] },
    GemClass {
        name: "JWT::JWK::Set",
        class_methods: &[],
        instance_methods: &[("keys", GemTy::Untyped), ("export", GemTy::Untyped), ("select!", GemTy::Untyped), ("filter!", GemTy::Untyped)],
    },
    GemClass { name: "JWT::EncodeError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::DecodeError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::VerificationError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::ExpiredSignature", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::IncorrectAlgorithm", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::ImmatureSignature", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidIssuerError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidIatError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidAudError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidSubError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidCritError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidJtiError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::InvalidPayload", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::MissingRequiredClaim", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::Base64DecodeError", class_methods: &[], instance_methods: JWT_ERROR },
    GemClass { name: "JWT::JWKError", class_methods: &[], instance_methods: JWT_ERROR },
    // RestClient — the HTTP client (rest-client). As Faraday: the real gem
    // on ruby and jruby, refused elsewhere (`lower::ruby_family_gems`).
    GemClass {
        name: "RestClient",
        class_methods: &[
            ("get", GemTy::Instance("RestClient::Response")),
            ("post", GemTy::Instance("RestClient::Response")),
            ("put", GemTy::Instance("RestClient::Response")),
            ("patch", GemTy::Instance("RestClient::Response")),
            ("delete", GemTy::Instance("RestClient::Response")),
            ("head", GemTy::Instance("RestClient::Response")),
            ("options", GemTy::Instance("RestClient::Response")),
        ],
        instance_methods: &[],
    },
    GemClass {
        name: "RestClient::Request",
        // Gradual: `raw_response: true` answers a `RestClient::RawResponse`.
        class_methods: &[("execute", GemTy::Untyped)],
        instance_methods: &[("execute", GemTy::Untyped)],
    },
    GemClass {
        name: "RestClient::Response",
        class_methods: &[],
        instance_methods: &[
            ("body", GemTy::Str),
            ("code", GemTy::Int),
            ("headers", GemTy::Untyped),
            ("raw_headers", GemTy::Untyped),
            ("cookies", GemTy::Untyped),
            ("to_s", GemTy::Str),
            ("to_str", GemTy::Str),
            ("request", GemTy::Untyped),
        ],
    },
    GemClass { name: "RestClient::Exceptions", class_methods: &[], instance_methods: &[] },
    GemClass { name: "RestClient::Exception", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::ExceptionWithResponse", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::RequestFailed", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::RequestTimeout", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::ServerBrokeConnection", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::SSLCertificateNotVerified", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::Exceptions::Timeout", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::Exceptions::OpenTimeout", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::Exceptions::ReadTimeout", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::BadRequest", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::Unauthorized", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::Forbidden", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::NotFound", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::ResourceNotFound", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::MethodNotAllowed", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::Conflict", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::UnprocessableEntity", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::TooManyRequests", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::InternalServerError", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::BadGateway", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::ServiceUnavailable", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    GemClass { name: "RestClient::GatewayTimeout", class_methods: &[], instance_methods: REST_CLIENT_ERROR },
    // ElasticAPM — the elastic-apm agent. As Faraday: the real gem on
    // ruby and jruby, refused elsewhere (`lower::ruby_family_gems`). An
    // app that never starts the agent gets the pass-through it has in
    // Ruby: no transaction, `with_span` answers its block.
    GemClass {
        name: "ElasticAPM",
        class_methods: &[
            ("running?", GemTy::Bool),
            ("current_transaction", GemTy::Untyped),
            ("current_span", GemTy::Untyped),
            ("start_transaction", GemTy::Untyped),
            ("end_transaction", GemTy::Untyped),
            ("with_transaction", GemTy::Untyped),
            ("start_span", GemTy::Untyped),
            ("end_span", GemTy::Untyped),
            ("with_span", GemTy::Untyped),
            ("set_label", GemTy::Untyped),
            ("set_custom_context", GemTy::Untyped),
            ("set_user", GemTy::Untyped),
            ("report", GemTy::Untyped),
            ("report_message", GemTy::Untyped),
        ],
        instance_methods: &[],
    },
    // `include ElasticAPM::SpanHelpers` gives the includer `span_method`
    // and `span_class_method`; the Ruby emit replays those calls.
    GemClass { name: "ElasticAPM::SpanHelpers", class_methods: &[], instance_methods: &[] },
];

const FARADAY_FILE_PART: &[(&str, GemTy)] = &[
    ("content_type", GemTy::Untyped),
    ("original_filename", GemTy::Untyped),
    ("local_path", GemTy::Untyped),
    ("io", GemTy::Untyped),
];

const FARADAY_ERROR: &[(&str, GemTy)] = &[
    ("message", GemTy::Str),
    ("to_s", GemTy::Str),
    ("response", GemTy::Untyped),
    ("response_status", GemTy::Untyped),
    ("response_body", GemTy::Untyped),
    ("response_headers", GemTy::Untyped),
    ("wrapped_exception", GemTy::Untyped),
    ("backtrace", GemTy::Untyped),
];

const JWT_ERROR: &[(&str, GemTy)] = &[("message", GemTy::Str), ("to_s", GemTy::Str)];

const REST_CLIENT_ERROR: &[(&str, GemTy)] = &[
    ("message", GemTy::Str),
    ("to_s", GemTy::Str),
    ("http_code", GemTy::Untyped),
    ("http_body", GemTy::Untyped),
    ("http_headers", GemTy::Untyped),
    ("response", GemTy::Untyped),
];
