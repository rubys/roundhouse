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
/// writes, or neither. `Send + Sync` so `Analyzer` can hold a boxed
/// adapter and be shared freely across threads.
///
/// Deliberately minimal for the current refactor: only the
/// effect-classification surface the analyzer already needs.
/// Capability checks, async-suspension profiles, and diagnostic
/// producers come as future trait methods when their respective
/// consumers land.
pub trait DatabaseAdapter: Send + Sync {
    /// Classify `method` (an AR method name — no receiver context,
    /// since this is called only after the analyzer has confirmed
    /// the receiver is a class with a bound table).
    fn classify_ar_method(&self, method: &str) -> ArMethodKind;
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
}
