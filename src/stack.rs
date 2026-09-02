//! The stack the compiler runs on.
//!
//! Every phase — ingest, analyze, lower, emit — is a recursive walker over
//! the program's expression tree, so the stack it needs is proportional to
//! the deepest expression it meets, not to the program's size. A 45-term
//! `a == "x" || a == "y" || …` chain in `runtime/ruby/action_view/
//! view_helpers.rb` (bf2e160f) is a 45-deep left spine, and the ingest
//! walk's DEBUG-profile frame is large enough that 45 of them overflowed
//! the 8 MB the OS gives a main thread. Release frames are a fraction of
//! the size, so `cargo run --release` (bin/rh, CI's site job) sailed
//! through while `cargo run` (scripts/bench's `emit_preview` step) aborted
//! — nine bench lanes vanished from the page on 2026-09-02 with the reason
//! discarded to /dev/null.
//!
//! [`run`] gives the pipeline a thread with a budget the walkers cannot
//! plausibly reach, so a CLI's stack is a property of the compiler, not
//! of whichever profile or launcher happened to start it. The alternative
//! the cargo config once pointed at, `stacker::maybe_grow` in each
//! load-bearing walker, fixes one walker per call site and adds a
//! dependency; a thread budget fixes every walker in one place. Test
//! threads get theirs from `RUST_MIN_STACK` in `.cargo/config.toml`.
//!
//! Host-only: wasm has neither threads nor a resizable stack, and the
//! browser build's stack is set at link time.

/// Stack budget for the thread that runs the compiler pipeline. Virtual
/// address space, committed a page at a time as recursion actually
/// touches it, so the cost of headroom is nil. 256 MB is eight times the
/// test-thread budget that has never been reached.
pub const COMPILER_STACK_BYTES: usize = 256 << 20;

/// Run `f` on a thread with [`COMPILER_STACK_BYTES`] of stack and return
/// its result. A panic inside `f` is re-raised on the caller so the
/// process still fails the way it did before this existed (message from
/// the panic hook, exit status 101).
pub fn run<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("roundhouse".into())
        .stack_size(COMPILER_STACK_BYTES)
        .spawn(f)
        .expect("spawn the compiler thread")
        .join()
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}
