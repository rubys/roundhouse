//! Database adapter trait — the seam between effect inference and
//! per-backend capability.
//!
//! Today's only use: classify an ActiveRecord method name as a
//! side-effecting database read, write, or neither. The analyzer
//! consults the adapter when attaching `DbRead(table)` /
//! `DbWrite(table)` effects to a Send whose receiver has a bound
//! table.
//!
//! The adapter is *backend-specific*, not language-specific —
//! `SqliteAdapter` answers "what does AR's `.pluck` do under
//! SQLite?" regardless of which target language emits the generated
//! project. Future adapters (`PostgresAdapter`, `IndexedDbAdapter`,
//! `D1Adapter`, `NeonAdapter`) plug in by implementing the trait
//! differently — e.g., an IndexedDB adapter can return `Unknown`
//! for AR methods it doesn't yet support on key-value storage,
//! which surfaces as diagnostics downstream instead of silently
//! dropping the effect.
//!
//! Phase-2 growth of this surface (without breaking callers):
//! - `async_suspending_effects()` — which effects suspend under
//!   this backend (empty for sync drivers, non-empty for network /
//!   IndexedDB / OPFS drivers). Drives `await` insertion.
//! - `supports_method()` — binary accept/reject for diagnostics
//!   that need richer reporting than "Unknown means no effect."
//! - `DbOpaque` handling — for raw-SQL / `connection.execute` sites
//!   that bypass the signature table.
//!
//! Today's minimum: one method on one trait, one impl, no consumers
//! outside the analyzer.

/// Classification of an ActiveRecord method by side-effect class.
/// `Unknown` means "not an AR method that this adapter recognizes
/// as effectful" — the analyzer attaches no DB effect. Later
/// adapters may want finer-grained variants (`ReadMany`,
/// `WriteBulk`, `Opaque`) but the three-way split is enough for
/// the current effect inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArMethodKind {
    /// Method executes a SELECT (or equivalent) against the
    /// receiver's bound table — `find`, `all`, `where`, `count`, …
    Read,
    /// Method executes an INSERT / UPDATE / DELETE (or equivalent)
    /// against the receiver's bound table — `save`, `destroy`,
    /// `update_all`, …
    Write,
    /// Method is not classified by this adapter. Could be a non-AR
    /// method (schema attribute reader, user-defined helper) or an
    /// AR method the adapter doesn't yet recognize. Effect inference
    /// attaches nothing; other typing paths may still produce a
    /// return type.
    Unknown,
}

/// A database adapter declares which AR methods are DB reads, DB
/// writes, or neither, and — per the adapter's backend — which
/// effects suspend (drive `await` insertion in async-capable
/// emitters). `Send + Sync` so `Analyzer` can hold a boxed adapter
/// and be shared freely across threads.
///
/// Capability checks and diagnostic producers come as future trait
/// methods when their respective consumers land.
pub trait DatabaseAdapter: Send + Sync {
    /// Classify `method` (an AR method name — no receiver context,
    /// since this is called only after the analyzer has confirmed
    /// the receiver is a class with a bound table).
    fn classify_ar_method(&self, method: &str) -> ArMethodKind;

    /// Does this effect suspend under this adapter's backend?
    ///
    /// Async-capable emitters (Rust/axum, TypeScript/Juntos,
    /// Python/FastAPI) consult this per Send site: when any effect
    /// on the expression is suspending, an `await` / `.await`
    /// prefix gets emitted. Sync-only backends (the default
    /// `SqliteAdapter`, wrapping `better-sqlite3` / `rusqlite` /
    /// stdlib `sqlite3`) return false uniformly — nothing
    /// suspends, so emitted code is unconditionally synchronous.
    ///
    /// The default impl returns false for every effect — adapters
    /// that support async backends override, classifying specific
    /// effect variants (typically `DbRead` / `DbWrite` / `Net`) as
    /// suspending. Adapters that want to classify by the effect's
    /// payload (e.g., only suspending on a particular table) pass
    /// the full `Effect` reference; today's `SqliteAsyncAdapter`
    /// ignores payload and suspends on any DB effect.
    fn is_suspending_effect(&self, effect: &crate::effect::Effect) -> bool {
        let _ = effect;
        false
    }
}

/// The default adapter: SQLite semantics. Accepts the full AR query
/// builder surface the current analyzer knows about — same method
/// list that previously lived as two free functions
/// (`is_db_read_method` / `is_db_write_method`) in `analyze.rs`.
///
/// Every target language's sqlite driver (rusqlite, better-sqlite3,
/// exqlite, crystal-db, modernc.org/sqlite, stdlib sqlite3) behaves
/// the same way at the AR-method level, so one adapter covers them
/// all. When a later target introduces a driver that refuses some
/// AR surface (e.g., an IndexedDB adapter that can't do arbitrary
/// `joins`), it plugs in as a separate adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct SqliteAdapter;

impl DatabaseAdapter for SqliteAdapter {
    fn classify_ar_method(&self, method: &str) -> ArMethodKind {
        // Consult the shared catalog (`crate::catalog::AR_CATALOG`)
        // — the authoritative record of AR method classification.
        // The catalog is receiver-aware (Class vs Instance); this
        // adapter trait isn't yet (it takes only a method name), so
        // we search across any receiver context and return the
        // first matching effect class. When the trait grows to
        // carry receiver context, this lookup becomes a direct
        // `catalog::lookup(method, receiver)` call.
        //
        // SqliteAdapter accepts every entry in the catalog — sqlite
        // supports the full AR query builder. Future adapters
        // (IndexedDB, D1) that refuse some methods will return
        // `Unknown` for those even when the catalog has an entry,
        // surfacing as diagnostics downstream.
        for entry in crate::catalog::lookup_any(method) {
            match entry.effect {
                crate::catalog::EffectClass::DbRead => return ArMethodKind::Read,
                crate::catalog::EffectClass::DbWrite => return ArMethodKind::Write,
                crate::catalog::EffectClass::Pure => return ArMethodKind::Unknown,
            }
        }
        ArMethodKind::Unknown
    }
}

/// SQLite semantics with async suspension — everything the default
/// `SqliteAdapter` classifies, but with `DbRead` and `DbWrite`
/// flagged as suspending so async-capable emitters insert `await`
/// at every AR call site.
///
/// Role: the minimum-divergence second adapter. Shares the full
/// AR surface with `SqliteAdapter` (same catalog lookups, same
/// `classify_ar_method` behavior) — differs only on the
/// suspending-effects axis. Exercises the metamodel's
/// polymorphism and the eventual effects-consumption machinery
/// without introducing a novel SQL dialect, capability profile,
/// or runtime integration.
///
/// Under the hood there's no "async SQLite" today — `better-
/// sqlite3` / `rusqlite` / stdlib `sqlite3` are all sync. Emitted
/// `await` against them is a no-op (`await` of a non-Promise
/// returns the value unchanged in JS / TS; Rust's `.await` on a
/// ready future is immediate). That's the point: validate the
/// async-emission plumbing against a backend we know works,
/// before introducing a real async backend (IndexedDB, D1, pg-
/// on-Node) where the awaits become load-bearing.
///
/// Future real async adapters (e.g., `IndexedDbAdapter`,
/// `D1Adapter`, `PostgresTokioAdapter`) will have the same
/// `is_suspending_effect` profile plus additional divergences:
/// different capability profiles, possibly refusing some AR
/// methods, different runtime symbol maps.
#[derive(Debug, Default, Clone, Copy)]
pub struct SqliteAsyncAdapter;

impl DatabaseAdapter for SqliteAsyncAdapter {
    fn classify_ar_method(&self, method: &str) -> ArMethodKind {
        // Same AR surface as the sync SqliteAdapter. The divergence
        // lives entirely in `is_suspending_effect` below — we
        // delegate to the catalog-backed classification to keep
        // the two adapters in lockstep on the AR method surface.
        SqliteAdapter.classify_ar_method(method)
    }

    fn is_suspending_effect(&self, effect: &crate::effect::Effect) -> bool {
        matches!(
            effect,
            crate::effect::Effect::DbRead { .. } | crate::effect::Effect::DbWrite { .. },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_classifies_reads() {
        let a = SqliteAdapter;
        for m in [
            "all", "find", "find_by", "find_by!", "first", "last",
            "where", "limit", "offset", "order", "group", "having",
            "joins", "includes", "preload", "select", "distinct",
            "count", "exists?", "pluck", "pick", "take",
            "sum", "average", "maximum", "minimum",
        ] {
            assert_eq!(a.classify_ar_method(m), ArMethodKind::Read, "{m}");
        }
    }

    #[test]
    fn sqlite_classifies_writes() {
        let a = SqliteAdapter;
        for m in [
            "save", "save!", "create", "create!", "update", "update!",
            "update_all", "destroy", "destroy!", "destroy_all",
            "delete", "delete_all", "increment!", "decrement!",
            "touch", "touch_all", "insert", "insert_all",
            "upsert", "upsert_all",
        ] {
            assert_eq!(a.classify_ar_method(m), ArMethodKind::Write, "{m}");
        }
    }

    #[test]
    fn sqlite_returns_unknown_for_non_ar_methods() {
        let a = SqliteAdapter;
        for m in ["title", "to_s", "length", "empty?", "unrelated_helper"] {
            assert_eq!(a.classify_ar_method(m), ArMethodKind::Unknown, "{m}");
        }
    }

    /// A minimal alternate adapter that refuses every AR method.
    /// Proves the trait is plug-in-able — effect inference under
    /// this adapter should attach no DB effects anywhere.
    #[test]
    fn custom_adapter_can_refuse_everything() {
        struct NoDbAdapter;
        impl DatabaseAdapter for NoDbAdapter {
            fn classify_ar_method(&self, _method: &str) -> ArMethodKind {
                ArMethodKind::Unknown
            }
        }
        let a = NoDbAdapter;
        for m in ["all", "find", "save", "destroy", "count"] {
            assert_eq!(a.classify_ar_method(m), ArMethodKind::Unknown);
        }
    }

    // Suspending-effects — the axis along which SqliteAsyncAdapter
    // diverges from the default sync SqliteAdapter.

    fn table(name: &str) -> crate::ident::TableRef {
        crate::ident::TableRef(crate::ident::Symbol::from(name))
    }

    #[test]
    fn sqlite_sync_suspends_nothing() {
        use crate::effect::Effect;
        let a = SqliteAdapter;
        let cases = [
            Effect::DbRead { table: table("articles") },
            Effect::DbWrite { table: table("articles") },
            Effect::Io,
            Effect::Time,
            Effect::Random,
            Effect::Log,
            Effect::Net { host: None },
        ];
        for e in &cases {
            assert!(
                !a.is_suspending_effect(e),
                "SqliteAdapter should not suspend on {e:?}",
            );
        }
    }

    #[test]
    fn sqlite_async_suspends_db_effects() {
        use crate::effect::Effect;
        let a = SqliteAsyncAdapter;
        for e in [
            Effect::DbRead { table: table("articles") },
            Effect::DbWrite { table: table("articles") },
            Effect::DbRead { table: table("comments") },
            Effect::DbWrite { table: table("comments") },
        ] {
            assert!(
                a.is_suspending_effect(&e),
                "SqliteAsyncAdapter should suspend on {e:?}",
            );
        }
    }

    #[test]
    fn sqlite_async_does_not_suspend_non_db_effects() {
        use crate::effect::Effect;
        let a = SqliteAsyncAdapter;
        // IO / Time / Random / Log / Net don't classify as
        // DB-suspending under SqliteAsyncAdapter — async is
        // specific to the DB backend, not universal. A future
        // AsyncNetAdapter would add Net to its suspending set;
        // that's a separate profile, not this one.
        for e in [
            Effect::Io,
            Effect::Time,
            Effect::Random,
            Effect::Log,
            Effect::Net { host: None },
            Effect::Net { host: Some("example.com".into()) },
        ] {
            assert!(
                !a.is_suspending_effect(&e),
                "SqliteAsyncAdapter should not suspend on {e:?}",
            );
        }
    }

    #[test]
    fn sync_and_async_share_ar_classification() {
        // The two adapters differ only in suspending-effects;
        // their AR method classification is identical because
        // both consult the shared catalog.
        let sync = SqliteAdapter;
        let async_ = SqliteAsyncAdapter;
        for m in [
            "all", "find", "where", "limit", "save", "destroy",
            "count", "pluck", "unknown_method", "title",
        ] {
            assert_eq!(
                sync.classify_ar_method(m),
                async_.classify_ar_method(m),
                "AR classification should match for `{m}`",
            );
        }
    }
}
