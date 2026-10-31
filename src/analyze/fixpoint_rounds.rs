//! How each loop of the whole-program fixpoint in
//! [`super::Analyzer::analyze`] ended.

/// How each loop ended, in the order they run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixpointRounds {
    /// Harvest, unify and retype over production code.
    pub production: LoopEnd,
    /// Views once, then test rounds.
    pub views_and_tests: LoopEnd,
    /// Production again, when view or test call sites moved a production
    /// signature; otherwise one unchecked pass runs in its place.
    pub absorb: LoopEnd,
    /// Params re-joined from the restamped views and production retyped
    /// until its returns settle, round after round until a re-join moves
    /// no param.
    pub restamp: Restamp,
}

/// How the restamp ended, and what it spent. Its rounds share one budget
/// of production passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Restamp {
    /// `Settled(round)` when that round's re-join moved no param and the
    /// passes before it had settled; `RanToCap` when the budget ran out.
    pub end: LoopEnd,
    /// Rounds run, each one re-join.
    pub rounds: usize,
    /// Production passes spent.
    pub passes: usize,
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
