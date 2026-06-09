//! `LibraryClass` → Swift file.
//!
//! Phase 1 skeleton — empty. Phase 2 ports the kind-agnostic class/module
//! walker here from `src/emit/kotlin/library.rs` (the template), with the
//! Swift deltas: Ruby modules → caseless `enum` namespaces (no `object`
//! keyword), `open`/`override` → plain inheritance + `override`, and the
//! `throws`-propagation pass marking throwing methods.
