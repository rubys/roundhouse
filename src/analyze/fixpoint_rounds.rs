//! How each loop of the whole-program fixpoint in
//! [`super::Analyzer::analyze`] ended.

/// One [`LoopEnd`] per loop, in the order they run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixpointRounds {
    /// Harvest, unify and retype over production code.
    pub production: LoopEnd,
    /// Views once, then test rounds.
    pub views_and_tests: LoopEnd,
    /// Production again, when view or test call sites moved a production
    /// signature; otherwise one unchecked pass runs in its place.
    pub absorb: LoopEnd,
}

/// How one loop ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopEnd {
    /// Its signature check passed on this round, counting from 0.
    Settled(usize),
    /// It ran every round its cap allows without its check passing.
    RanToCap,
    /// It never started.
    #[default]
    NotRun,
}
