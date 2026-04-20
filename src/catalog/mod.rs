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
    /// No database effect. Pure attribute accessors, format
    /// conversions, etc. (Not in today's catalog — listed for
    /// future growth.)
    #[allow(dead_code)]
    Pure,
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
    // ---- Class-method reads (query surface) ----
    // Terminal reads — execute a SELECT and return results.
    CatalogedMethod {
        name: "all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "find",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "find_by",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "find_by!",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "first",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "last",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "take",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "count",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "exists?",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "pluck",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "pick",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "sum",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "average",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "maximum",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
    },
    CatalogedMethod {
        name: "minimum",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Terminal,
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
    },
    CatalogedMethod {
        name: "limit",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "offset",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "order",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "group",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "having",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "joins",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "includes",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "preload",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "select",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
    },
    CatalogedMethod {
        name: "distinct",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbRead,
        chain: ChainKind::Builder,
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
    },
    CatalogedMethod {
        name: "create!",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "update_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "destroy_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "delete_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "insert",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "insert_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "upsert",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "upsert_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "touch_all",
        receiver: ReceiverContext::Class,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    // ---- Instance-method writes ----
    // Mutations on a loaded record.
    CatalogedMethod {
        name: "save",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "save!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "update",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "update!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "destroy",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "destroy!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "delete",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "increment!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "decrement!",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
    },
    CatalogedMethod {
        name: "touch",
        receiver: ReceiverContext::Instance,
        effect: EffectClass::DbWrite,
        chain: ChainKind::NotApplicable,
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
