//! Method catalog — the IDL-shaped single source of truth for what
//! the compiler knows about framework and runtime method surfaces.
//!
//! ## Why this exists
//!
//! Before the catalog, knowledge about ActiveRecord methods was
//! scattered across five places: `SqliteAdapter.classify_ar_method`
//! (effect classification), `Analyzer::new` class_methods HashMap
//! (return types), `lower::controller::is_query_builder_method`
//! (chain classification), hand-coded emitter templates (emission
//! shapes), and per-target runtime stubs (actual implementations).
//! Adding a new AR method meant editing N places, and drift was
//! inevitable.
//!
//! The catalog is the authoritative declarative record. Each entry
//! captures the facets every consumer needs: identity (name +
//! receiver context), side-effect class, chain semantics (for
//! terminal-vs-builder distinction), and — growing over time —
//! return-type signature, capability gate, per-target runtime
//! symbol maps.
//!
//! ## What this is not
//!
//! - Not an external DSL today. Entries live as Rust code (static
//!   table). If/when externalization is needed (gem-author RBS,
//!   user annotations), a parser will populate the same
//!   `CatalogedMethod` struct.
//! - Not a type system. The analyzer still owns type inference; the
//!   catalog just declares what's available for dispatch.
//! - Not a capability profile. Adapters declare *which* catalog
//!   entries they support; the catalog itself is adapter-neutral.
//!
//! ## What's in the minimum viable version
//!
//! AR methods only. The catalog will grow to include view helpers
//! (`form_with`, `link_to`, `render`), controller helpers (`render`,
//! `redirect_to`, `head`), and route DSL over time — but today's
//! scope is the ~45 AR methods the current analyzer knows.
//!
//! Return-type signatures are *not yet* in the struct — they
//! require a `Relation<T>` type kind that doesn't exist yet. When
//! that lands, return types join as a facet; existing entries get
//! augmented without breaking consumers.

use std::collections::BTreeSet;

/// One cataloged method. Static-lifetime strings keep entries
/// zero-allocation at runtime — the catalog is const data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogedMethod {
    /// Method name as written in Ruby source. Bang variants
    /// (`save!`, `destroy!`) are distinct entries from their
    /// non-bang counterparts.
    pub name: &'static str,
    /// Where the method is called — on the class, on an instance,
    /// on a Relation, or on an association. Same method name
    /// (`find`, `create`) can mean different things in different
    /// receiver contexts; the catalog keys on `(name, receiver)`.
    pub receiver: ReceiverContext,
    /// Side-effect class. `DbRead` / `DbWrite` attach a
    /// corresponding `Effect::DbRead { table }` / `Effect::DbWrite
    /// { table }` when the analyzer visits the Send site. `Pure`
    /// attaches nothing (e.g., attribute readers, to_s).
    pub effect: EffectClass,
    /// For Relation-builder methods, whether this call is the
    /// terminal step (executes the query) or a chainable step
    /// (builds the query further). `NotApplicable` covers writes
    /// (which always execute) and non-relation methods.
    pub chain: ChainKind,
    /// Declared return-type shape, parametric on the receiver's
    /// Self type. The analyzer instantiates this against each
    /// model class when building `class_methods` / `instance_
    /// methods` registries — `ArrayOfSelf` for `Article` becomes
    /// `Ty::Array<Ty::Class(Article)>`, etc. `None` means the
    /// return type isn't declared in the catalog; the analyzer
    /// falls back to not populating a method-signature entry,
    /// leaving downstream type inference to produce Unknown.
    pub return_kind: Option<ReturnKind>,
}

/// Which receiver shape this method is defined on. Distinguishing
/// these is load-bearing: `find` on a class looks up by primary key
/// (`User.find(1)`), while `find` on an association looks up within
/// a scope (`user.posts.find(1)`). Same method name, different
/// semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReceiverContext {
    /// Called on the model class: `User.find(1)`, `User.all`.
    Class,
    /// Called on a model instance: `user.save`, `post.destroy`.
    Instance,
    // Relation and Association receiver contexts will join as the
    // analyzer gains Relation<T> and Association<T> type kinds.
    // Today's catalog stops at Class/Instance because the
    // analyzer's receiver-context detection stops there.
}

/// Side-effect class of a cataloged method. Maps onto the
/// `Effect::DbRead` / `Effect::DbWrite` / (nothing) triad the
/// analyzer's effect inference produces today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectClass {
    /// Method executes a SELECT-equivalent — `find`, `all`,
    /// `where`, `count`, `pluck`, …
    DbRead,
    /// Method executes an INSERT / UPDATE / DELETE — `save`,
    /// `destroy`, `update_all`, `create`, …
    DbWrite,
    /// No database effect. In-memory operations like `Model.new`
    /// (constructs an instance; doesn't hit the DB until `.save`),
    /// attribute accessors, format conversions.
    Pure,
}

/// Return-type shape for a cataloged method, parametric on the
/// receiver's Self type. Consumers (analyzer building
/// class_methods registries) instantiate these against the
/// concrete model class — `ArrayOfSelf` for the `Article` model
/// becomes `Ty::Array<Ty::Class(Article)>`.
///
/// Covers the five shapes the current analyzer declares inline.
/// Grows naturally (HashOf, ClassRef for ActiveModel::Errors,
/// etc.) as more of the AR surface comes into the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnKind {
    /// Returns the receiver type itself (the model class).
    /// Example: `Model.new`, `Model.create`, `Model.find` (by
    /// primary key — non-nullable; find raises if not found).
    SelfType,
    /// Returns `Array<Self>` — a concrete materialized collection
    /// of records. Example: `Model.all`, `Model.where(...)`,
    /// `Model.limit(5)`. Note: catalog doesn't yet distinguish
    /// `Relation<T>` from `Array<T>` — terminal-vs-builder
    /// distinction is tracked via `ChainKind`, not the return
    /// type. When `Relation<T>` arrives, these split.
    ArrayOfSelf,
    /// Returns `Self | Nil`. Example: `Model.find_by(...)`,
    /// `Model.first`, `Model.last` — lookups that may return
    /// nothing without raising.
    SelfOrNil,
    /// Returns `Int`. Example: `Model.count`.
    Int,
    /// Returns `Bool`. Example: `Model.exists?`, `#save`,
    /// `#valid?`, `#persisted?`.
    Bool,
    /// Returns `Hash<Sym, Str>`. Example: `#attributes` on an
    /// ActiveRecord instance — the canonical schema-derived
    /// attribute dictionary. Specific rather than generic because
    /// this one shape is what the Rails dialect emits; if/when
    /// another Hash shape enters the catalog, generalize to a
    /// `HashOf(PrimKind, PrimKind)` variant.
    HashSymStr,
    /// Reference to a concrete class by dotted-name path
    /// (e.g. `"ActiveModel::Errors"`). Analyzer instantiates as
    /// `Ty::Class { id: ClassId(<path>), args: vec![] }`.
    /// Used by `#errors` to reference the ActiveModel::Errors
    /// class without needing it to be a user-defined model.
    ClassRef(&'static str),
}

/// Chain semantics for a Relation-builder method.
///
/// ActiveRecord's query builder is lazy: `Article.where(...)`
/// returns a `Relation` that hasn't executed yet; only terminal
/// operations (`.to_a`, `.first`, `.count`) trigger the actual
/// SELECT. This distinction matters for async emission — only the
/// terminal step needs `await` under an async adapter; chainable
/// steps don't hit the database.
///
/// Today's classification is coarse: everything is Terminal
/// because the emitter doesn't yet distinguish chain-builder from
/// terminal, and the SqliteAdapter (current sole adapter) is sync
/// so it doesn't matter. When async adapters and `Relation<T>`
/// typing land, Builder-marked methods stop producing DbRead
/// effects (the Relation carries them; only the Terminal step
/// emits them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainKind {
    /// Method executes the query — `all`, `find`, `first`,
    /// `to_a`, `count`, `pluck`, …
    Terminal,
    /// Method builds the query further without executing —
    /// `where`, `limit`, `order`, `includes`, `joins`, …
    Builder,
    /// Chain semantic doesn't apply — all writes, and reads that
    /// aren't part of a relation chain (e.g., aggregate class
    /// methods that always execute).
    NotApplicable,
}

/// The AR method catalog — every ActiveRecord method the compiler
/// recognizes today. Ordered roughly by: class-method reads,
/// class-method writes, instance-method writes.
///
/// Adding a new AR method: add one entry here. Consumers
/// (SqliteAdapter classifier, future effect inference, future
/// emitter templates) all pick it up via the single source.
pub const AR_CATALOG: &[CatalogedMethod] = &[
    // ---- Class-method factory ----
    // `Model.new(attrs)` — constructs an in-memory instance; no
    // database hit until `.save`. Tracked here (rather than
    // omitted) because the analyzer's `class_methods` registry
    // includes it, and consolidating both return-type + effect
    // declarations in one place is the catalog's purpose.
    CatalogedMethod {
        name: "new",
        receiver: ReceiverContext::Class,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    // ---- Class-method reads (query surface) ----
    // Terminal reads — execute a SELECT and return results.
    CatalogedMethod {
        name: "all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "find",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::SelfType),
    },
    CatalogedMethod {
        name: "find_by",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::SelfOrNil),
    },
    CatalogedMethod {
        name: "find_by!",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "first",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::SelfOrNil),
    },
    CatalogedMethod {
        name: "last",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::SelfOrNil),
    },
    CatalogedMethod {
        name: "take",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "count",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::Int),
    },
    CatalogedMethod {
        name: "exists?",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "pluck",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "pick",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "sum",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "average",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "maximum",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    CatalogedMethod {
        name: "minimum",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
        return_kind: None,
    },
    // Builder reads — chain further without executing. When the
    // analyzer grows Relation<T>, these stop producing effects
    // (the terminal .to_a / .first / .count does). Today they
    // classify as DbRead because the analyzer treats every
    // class-method read uniformly.
    CatalogedMethod {
        name: "where",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "limit",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "offset",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "order",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "group",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "having",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "joins",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "includes",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "preload",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    CatalogedMethod {
        name: "select",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: None,
    },
    CatalogedMethod {
        name: "distinct",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
        return_kind: Some(ReturnKind::ArrayOfSelf),
    },
    // ---- Class-method writes ----
    // Bulk / class-level mutations — always execute, no chain
    // semantic (you can't chain after `create_all`, you just run
    // it and get a result).
    CatalogedMethod {
        name: "create",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    CatalogedMethod {
        name: "create!",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "update_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "destroy_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "delete_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "insert",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "insert_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "upsert",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "upsert_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "touch_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    // ---- Instance-method writes ----
    // Mutations on a loaded record. Rails bangs-vs-non-bangs
    // convention: non-bang returns Bool (success/failure);
    // bang returns Self or raises on failure.
    CatalogedMethod {
        name: "save",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "save!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    CatalogedMethod {
        name: "update",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "update!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    CatalogedMethod {
        name: "destroy",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    CatalogedMethod {
        name: "destroy!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    CatalogedMethod {
        name: "delete",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "increment!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "decrement!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: None,
    },
    CatalogedMethod {
        name: "touch",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    // ---- Instance-method reads ----
    // `#reload` refreshes from the DB — writes-vs-reads-wise it's
    // a read, but carries the DbRead effect because it issues a
    // SELECT.
    CatalogedMethod {
        name: "reload",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbRead,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::SelfType),
    },
    // ---- Instance-method state predicates ----
    // Pure — query in-memory flags the record already carries.
    // `#persisted?` / `#new_record?` check loaded state;
    // `#valid?` / `#invalid?` run validations (arguably pure
    // against the catalog's effect classification since they
    // don't hit the DB, though they may trigger user code).
    CatalogedMethod {
        name: "valid?",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "invalid?",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "persisted?",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "new_record?",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "destroyed?",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    CatalogedMethod {
        name: "changed?",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::Bool),
    },
    // ---- Instance-method accessors (state / metadata) ----
    // `#attributes` returns Hash<Sym, Str>; `#errors` returns the
    // per-instance ActiveModel::Errors collection. Both pure
    // (no DB hit); both structural rather than expression-
    // dispatchable.
    CatalogedMethod {
        name: "attributes",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::HashSymStr),
    },
    CatalogedMethod {
        name: "errors",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::Pure,
        chain: ChainKind::NotApplicable,
        return_kind: Some(ReturnKind::ClassRef("ActiveModel::Errors")),
    },
];

/// Look up a method in the catalog by name + receiver context.
/// Returns the single matching entry or None. Used by adapters
/// that need the full record (effect + chain + future facets).
pub fn lookup(name: &str, receiver: ReceiverContext) -> Option<&'static CatalogedMethod> {
    AR_CATALOG
        .iter()
        .find(|m| m.name == name && m.receiver == receiver)
}

/// Look up a method by name only, returning all matching entries
/// across receiver contexts. Used by consumers that don't track
/// receiver context yet (the current `SqliteAdapter.classify_ar_
/// method`, which takes only a method name).
pub fn lookup_any(name: &str) -> impl Iterator<Item = &'static CatalogedMethod> {
    AR_CATALOG.iter().filter(move |m| m.name == name)
}

/// Every receiver context that has at least one cataloged method.
/// Used by tests and by adapter introspection.
pub fn receivers_for(name: &str) -> BTreeSet<ReceiverContext> {
    lookup_any(name).map(|m| m.receiver).collect()
}

/// Query-builder method names whose scaffold-runtime handling is
/// "collapse the chain to an empty collection of the target model
/// type." This is a **runtime-capability** question, not a Ruby/
/// Rails semantics question — it reflects what the current per-
/// target runtime stubs (Juntos, rusqlite wrappers, etc.) actually
/// implement.
///
/// The 13 methods listed here match the pre-catalog hand-rolled
/// list in `src/lower/controller.rs` — preserved verbatim during
/// the catalog migration to keep emit output byte-stable. They
/// intentionally overlap with but don't equal the catalog's
/// Class-receiver DbRead set: aggregate methods (`count`,
/// `exists?`, `sum`, `average`, etc.) are excluded because their
/// runtime handling is pass-through (the Juntos stub implements
/// `.count()` directly), not collapse-to-empty.
///
/// **TODO**: this belongs on `DatabaseAdapter` eventually — it's a
/// per-backend capability declaration, not a universal classifier.
/// Different adapters will support different subsets of the AR
/// surface at their runtime. Today's single-SQLite world lets us
/// keep the list catalog-local; the adapter trait method can
/// subsume it when a second runtime arrives.
pub fn is_query_builder_method(method: &str) -> bool {
    matches!(
        method,
        "all"
            | "includes"
            | "order"
            | "where"
            | "group"
            | "limit"
            | "offset"
            | "joins"
            | "distinct"
            | "select"
            | "pluck"
            | "first"
            | "last"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_entries_are_unique_per_name_and_receiver() {
        // (name, receiver) must be unique; same method name can
        // appear on multiple receivers (e.g., `create` on Class
        // and `create` on Association later).
        let mut seen: BTreeSet<(&str, ReceiverContext)> = BTreeSet::new();
        for m in AR_CATALOG {
            assert!(
                seen.insert((m.name, m.receiver)),
                "duplicate entry: ({}, {:?})",
                m.name,
                m.receiver,
            );
        }
    }

    #[test]
    fn catalog_covers_expected_read_methods() {
        // Regression anchor: every method the pre-catalog
        // SqliteAdapter classified as Read must still be in the
        // catalog as DbRead under at least one receiver context.
        for m in [
            "all", "find", "find_by", "find_by!", "first", "last",
            "where", "limit", "offset", "order", "group", "having",
            "joins", "includes", "preload", "select", "distinct",
            "count", "exists?", "pluck", "pick", "take",
            "sum", "average", "maximum", "minimum",
        ] {
            let found: Vec<_> = lookup_any(m)
                .filter(|e| e.effect == EffectClass::DbRead)
                .collect();
            assert!(
                !found.is_empty(),
                "expected at least one DbRead entry for `{m}`",
            );
        }
    }

    #[test]
    fn catalog_covers_expected_write_methods() {
        for m in [
            "save", "save!", "create", "create!", "update", "update!",
            "update_all", "destroy", "destroy!", "destroy_all",
            "delete", "delete_all", "increment!", "decrement!",
            "touch", "touch_all", "insert", "insert_all",
            "upsert", "upsert_all",
        ] {
            let found: Vec<_> = lookup_any(m)
                .filter(|e| e.effect == EffectClass::DbWrite)
                .collect();
            assert!(
                !found.is_empty(),
                "expected at least one DbWrite entry for `{m}`",
            );
        }
    }

    #[test]
    fn builder_reads_are_classified() {
        // Chain-builder methods — the round-3 distinction that
        // matters once Relation<T> typing lands. Today's classifier
        // ignores the chain facet, but the catalog already carries
        // it so the future work is data-in-place.
        for m in [
            "where", "limit", "offset", "order", "group", "having",
            "joins", "includes", "preload", "select", "distinct",
        ] {
            let entry = lookup(m, ReceiverContext::Class)
                .unwrap_or_else(|| panic!("no Class entry for `{m}`"));
            assert_eq!(
                entry.chain,
                ChainKind::Builder,
                "`{m}` should be Builder",
            );
        }
    }

    #[test]
    fn terminal_reads_are_classified() {
        for m in [
            "all", "find", "find_by", "find_by!", "first", "last",
            "take", "count", "exists?", "pluck", "pick",
            "sum", "average", "maximum", "minimum",
        ] {
            let entry = lookup(m, ReceiverContext::Class)
                .unwrap_or_else(|| panic!("no Class entry for `{m}`"));
            assert_eq!(
                entry.chain,
                ChainKind::Terminal,
                "`{m}` should be Terminal",
            );
        }
    }

    #[test]
    fn writes_are_not_chainable() {
        // Writes have chain=NotApplicable uniformly — you don't
        // chain after save.
        for entry in AR_CATALOG {
            if entry.effect == EffectClass::DbWrite {
                assert_eq!(
                    entry.chain,
                    ChainKind::NotApplicable,
                    "write `{}` should have chain=NotApplicable",
                    entry.name,
                );
            }
        }
    }
}
