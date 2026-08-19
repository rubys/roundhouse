//! Effect classification: the `Effect` variants and the `EffectSet`
//! the analyzer attaches to every expression and to `Ty::Fn`
//! signatures — per-table `DbRead`/`DbWrite` (from the adapter's
//! Active Record method classification) plus deliberately coarse
//! Io/Time/Random/Net/Log/Raises where per-site precision isn't
//! earned. Populated by `analyze/effects`; downstream, purity is a
//! license — lowerer hook passes re-evaluate or drop a receiver only
//! when its subtree is effect-free — and the per-table Db grain is
//! what lets IDE/MCP consumers report which tables an action reads
//! and writes. The empty set means pure and is the serde default, so
//! pure nodes serialize with no effects field at all.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::ident::{ClassId, EffectVar, TableRef};

/// A single side-effect class. Precise where useful (which table is read/written),
/// coarse where precision isn't earned (Io, Time, Random).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    Io,
    DbRead { table: TableRef },
    DbWrite { table: TableRef },
    Time,
    Random,
    Raises { class: ClassId },
    Net { host: Option<String> },
    Log,
    Var { var: EffectVar },
}

/// The set of effects a computation may perform. Empty set == pure.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectSet {
    pub effects: BTreeSet<Effect>,
}

impl EffectSet {
    pub fn pure() -> Self {
        Self::default()
    }

    pub fn is_pure(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn singleton(e: Effect) -> Self {
        let mut s = BTreeSet::new();
        s.insert(e);
        Self { effects: s }
    }

    pub fn insert(&mut self, e: Effect) {
        self.effects.insert(e);
    }

    pub fn union(mut self, other: Self) -> Self {
        self.effects.extend(other.effects);
        self
    }
}
