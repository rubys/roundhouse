//! Deployment profiles — the named (target + DB + HTTP shim) triples
//! the compiler validates as a unit.
//!
//! A profile selects every backend the emitted code talks to, in one
//! place. The async-coloring pass reads the profile to know which
//! adapter methods seed `is_async`; the emitter reads it to know
//! which HTTP entry-point shape to produce; the typer reads it to
//! know which DB capability surface is on.
//!
//! Phase 0 scope: define the data + cross-axis validation. Wire-up
//! to analyze/emit is Phase 1+.
//!
//! Initial profiles:
//! - `node-sync`: Node + better-sqlite3 + node:http. The
//!   pre-Phase-0 default; `is_async` propagates nothing.
//! - `node-async`: Node + libsql/pg + node:http. Async seeds light
//!   up; emit gains `await` at colored call sites.
//!
//! Future profiles arrive as added rows; the `validate` rules grow
//! to reject impossible pairings.

use crate::adapter::{DatabaseAdapter, SqliteAdapter, SqliteAsyncAdapter};

/// Compilation target — the language/runtime the emitter produces.
/// Mirrors the per-target emitters under `src/emit/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    TypeScript,
    Rust,
    Crystal,
    Elixir,
    Python,
    Go,
    Ruby,
    Spinel,
}

/// Database backend. Pairs with a `DatabaseAdapter` impl at
/// profile-construction time. Sync vs async is a property of the
/// driver, not the SQL dialect — `SqliteSync` (better-sqlite3,
/// rusqlite) and `SqliteAsync` (libsql) talk the same SQL but
/// suspend differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Database {
    /// Sync sqlite drivers: better-sqlite3, rusqlite, stdlib
    /// sqlite3. No suspension.
    SqliteSync,
    /// Async sqlite-shaped drivers: libsql, sql.js (web worker).
    /// All AR adapter methods suspend.
    SqliteAsync,
    /// Postgres via async driver (pg, asyncpg, sqlx-postgres).
    Postgres,
    /// Cloudflare D1 — async sqlite-over-RPC, only reachable from
    /// Workers.
    D1,
    /// Browser IndexedDB — async key-value with a thin AR shim.
    IndexedDb,
}

impl Database {
    /// Whether AR adapter methods on this database suspend (drive
    /// `await` insertion in async-capable emitters).
    pub fn is_async(self) -> bool {
        match self {
            Database::SqliteSync => false,
            Database::SqliteAsync
            | Database::Postgres
            | Database::D1
            | Database::IndexedDb => true,
        }
    }
}

/// HTTP entry-point shape — how the emitted process receives
/// requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpShim {
    /// Node `http.createServer` / `node:http`. The default for
    /// node targets; `runtime/typescript/server.ts` shape.
    NodeHttp,
    /// Cloudflare Workers fetch handler: `export default { fetch }`.
    CloudflareWorkers,
    /// Browser client-side router — no incoming HTTP, navigation
    /// drives action dispatch.
    BrowserRouter,
}

/// A validated (target, database, http shim) triple. Construct via
/// `DeploymentProfile::new(...)`, which runs `validate()`. Named
/// constructors (`node_sync`, `node_async`) skip validation because
/// their inputs are statically known to be valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeploymentProfile {
    pub name: &'static str,
    pub target: Target,
    pub database: Database,
    pub http_shim: HttpShim,
}

/// Why a profile was rejected. One variant per cross-axis
/// constraint so error messages can name the offending axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// The database is only reachable from a specific HTTP shim
    /// (e.g. D1 requires CloudflareWorkers).
    DatabaseRequiresShim {
        database: Database,
        required: HttpShim,
        got: HttpShim,
    },
    /// The HTTP shim is only valid on a specific target (e.g.
    /// CloudflareWorkers requires TypeScript today).
    ShimRequiresTarget {
        http_shim: HttpShim,
        required: Target,
        got: Target,
    },
    /// The database driver isn't reachable from this target/shim
    /// combination (e.g. better-sqlite3 in Workers).
    DatabaseUnreachable {
        database: Database,
        target: Target,
        http_shim: HttpShim,
    },
}

impl DeploymentProfile {
    /// Build and validate a profile. Returns `Err` for impossible
    /// pairings (D1 outside Workers, IndexedDB on Node, sync sqlite
    /// in Workers).
    pub fn new(
        name: &'static str,
        target: Target,
        database: Database,
        http_shim: HttpShim,
    ) -> Result<Self, ProfileError> {
        let p = Self { name, target, database, http_shim };
        p.validate()?;
        Ok(p)
    }

    /// The default Node profile: better-sqlite3, node:http. Identical
    /// emit to pre-Phase-0 behavior — no methods seed async.
    pub fn node_sync() -> Self {
        Self {
            name: "node-sync",
            target: Target::TypeScript,
            database: Database::SqliteSync,
            http_shim: HttpShim::NodeHttp,
        }
    }

    /// Node profile with an async sqlite driver (libsql). All AR
    /// adapter methods seed `is_async`; emit gains `await` at
    /// colored call sites.
    pub fn node_async() -> Self {
        Self {
            name: "node-async",
            target: Target::TypeScript,
            database: Database::SqliteAsync,
            http_shim: HttpShim::NodeHttp,
        }
    }

    /// Re-validate. `new()` calls this; named constructors skip it
    /// (their inputs are known-valid). Exposed so callers that
    /// build a profile by struct literal can verify it.
    pub fn validate(&self) -> Result<(), ProfileError> {
        // Constraint 1: shim → required target.
        let shim_target = match self.http_shim {
            HttpShim::NodeHttp => None,
            HttpShim::CloudflareWorkers => Some(Target::TypeScript),
            HttpShim::BrowserRouter => Some(Target::TypeScript),
        };
        if let Some(required) = shim_target {
            if self.target != required {
                return Err(ProfileError::ShimRequiresTarget {
                    http_shim: self.http_shim,
                    required,
                    got: self.target,
                });
            }
        }

        // Constraint 2: database → required shim.
        let db_shim = match self.database {
            Database::D1 => Some(HttpShim::CloudflareWorkers),
            Database::IndexedDb => Some(HttpShim::BrowserRouter),
            _ => None,
        };
        if let Some(required) = db_shim {
            if self.http_shim != required {
                return Err(ProfileError::DatabaseRequiresShim {
                    database: self.database,
                    required,
                    got: self.http_shim,
                });
            }
        }

        // Constraint 3: sync sqlite is unreachable from Workers /
        // browser. (Today's sync drivers are better-sqlite3 and
        // rusqlite; both need filesystem + native modules.)
        if self.database == Database::SqliteSync
            && matches!(
                self.http_shim,
                HttpShim::CloudflareWorkers | HttpShim::BrowserRouter
            )
        {
            return Err(ProfileError::DatabaseUnreachable {
                database: self.database,
                target: self.target,
                http_shim: self.http_shim,
            });
        }

        Ok(())
    }

    /// Construct the `DatabaseAdapter` impl matching this profile's
    /// database. The async-coloring pass uses this to seed its
    /// propagation; the analyzer uses it for effect classification.
    pub fn adapter(&self) -> Box<dyn DatabaseAdapter> {
        match self.database {
            Database::SqliteSync => Box::new(SqliteAdapter),
            Database::SqliteAsync => Box::new(SqliteAsyncAdapter),
            // Postgres / D1 / IndexedDB get their own adapters as
            // they land. Until then, fall back to async-sqlite —
            // it has the right suspension shape for any async DB.
            Database::Postgres | Database::D1 | Database::IndexedDb => {
                Box::new(SqliteAsyncAdapter)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_sync_is_valid() {
        let p = DeploymentProfile::node_sync();
        assert!(p.validate().is_ok());
        assert_eq!(p.name, "node-sync");
        assert!(!p.database.is_async());
    }

    #[test]
    fn node_async_is_valid() {
        let p = DeploymentProfile::node_async();
        assert!(p.validate().is_ok());
        assert_eq!(p.name, "node-async");
        assert!(p.database.is_async());
    }

    #[test]
    fn cloudflare_workers_with_d1_is_valid() {
        let p = DeploymentProfile::new(
            "cloudflare-d1",
            Target::TypeScript,
            Database::D1,
            HttpShim::CloudflareWorkers,
        )
        .expect("workers + D1 should validate");
        assert!(p.database.is_async());
    }

    #[test]
    fn browser_with_indexeddb_is_valid() {
        let p = DeploymentProfile::new(
            "browser-indexeddb",
            Target::TypeScript,
            Database::IndexedDb,
            HttpShim::BrowserRouter,
        )
        .expect("browser + IndexedDB should validate");
        assert!(p.database.is_async());
    }

    #[test]
    fn d1_outside_workers_is_rejected() {
        let err = DeploymentProfile::new(
            "bogus-d1-on-node",
            Target::TypeScript,
            Database::D1,
            HttpShim::NodeHttp,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProfileError::DatabaseRequiresShim {
                database: Database::D1,
                required: HttpShim::CloudflareWorkers,
                got: HttpShim::NodeHttp,
            }
        ));
    }

    #[test]
    fn indexeddb_on_node_is_rejected() {
        let err = DeploymentProfile::new(
            "bogus-indexeddb-on-node",
            Target::TypeScript,
            Database::IndexedDb,
            HttpShim::NodeHttp,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProfileError::DatabaseRequiresShim {
                database: Database::IndexedDb,
                required: HttpShim::BrowserRouter,
                ..
            }
        ));
    }

    #[test]
    fn sync_sqlite_in_workers_is_rejected() {
        let err = DeploymentProfile::new(
            "bogus-sync-in-workers",
            Target::TypeScript,
            Database::SqliteSync,
            HttpShim::CloudflareWorkers,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProfileError::DatabaseUnreachable {
                database: Database::SqliteSync,
                ..
            }
        ));
    }

    #[test]
    fn sync_sqlite_in_browser_is_rejected() {
        let err = DeploymentProfile::new(
            "bogus-sync-in-browser",
            Target::TypeScript,
            Database::SqliteSync,
            HttpShim::BrowserRouter,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProfileError::DatabaseUnreachable { .. }
        ));
    }

    #[test]
    fn workers_shim_on_non_typescript_is_rejected() {
        let err = DeploymentProfile::new(
            "bogus-workers-rust",
            Target::Rust,
            Database::D1,
            HttpShim::CloudflareWorkers,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProfileError::ShimRequiresTarget {
                http_shim: HttpShim::CloudflareWorkers,
                required: Target::TypeScript,
                got: Target::Rust,
            }
        ));
    }

    #[test]
    fn node_sync_adapter_is_sync_sqlite() {
        let p = DeploymentProfile::node_sync();
        let a = p.adapter();
        // Sync sqlite never suspends.
        let e = crate::effect::Effect::DbRead {
            table: crate::ident::TableRef(crate::ident::Symbol::from("articles")),
        };
        assert!(!a.is_suspending_effect(&e));
    }

    #[test]
    fn node_async_adapter_suspends_db() {
        let p = DeploymentProfile::node_async();
        let a = p.adapter();
        let e = crate::effect::Effect::DbRead {
            table: crate::ident::TableRef(crate::ident::Symbol::from("articles")),
        };
        assert!(a.is_suspending_effect(&e));
    }

    #[test]
    fn async_adapters_declare_seed_methods() {
        // The seed-method manifest is the bridge from "this DB is
        // async" to "these specific methods on the AR adapter
        // class get the seed `is_async` flag." Sync adapters
        // declare nothing; async adapters list the boundary.
        let sync = DeploymentProfile::node_sync().adapter();
        assert!(sync.async_seed_methods().is_empty());

        let async_ = DeploymentProfile::node_async().adapter();
        let seeds = async_.async_seed_methods();
        // The runtime/ruby/active_record/base.rb adapter calls:
        // all, find, where, count, exists?, insert, update, delete.
        // SqliteAsyncAdapter must list every one — these are the
        // methods on the AR adapter object that suspend.
        for m in ["all", "find", "where", "count", "exists?", "insert", "update", "delete"] {
            assert!(
                seeds.contains(&m),
                "async adapter missing seed method `{m}`",
            );
        }
    }
}
