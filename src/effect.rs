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
