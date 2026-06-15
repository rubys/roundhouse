# Vendored `ruby-rbs-sys` (wasm build support)

This is a temporary, in-repo copy of `ruby-rbs-sys` 0.3.0 carrying the
`wasm32-*` build support that is upstream-pending as **ruby/rbs#2992**
(branch `rust-wasm-bindings`). `wasm/Cargo.toml` patches `ruby-rbs-sys` to this
path so the browser-demo compiler (`roundhouse_wasm.wasm`) can build for
`wasm32-wasip1`.

## Why vendored (not a committed binary)

Building the wasm needs a `ruby-rbs-sys` that (a) compiles the RBS C parser with
the WASI SDK's clang and (b) runs bindgen against the **host** (`#[repr(C)]` is
layout-portable) — neither is in the published crate yet. Rather than commit the
3.8 MB built binary and refresh it by hand on every compiler/emit change, we
commit this **stable** crate (~750 KB, frozen until #2992 merges) and let CI
rebuild the wasm from source on each deploy (`build-wasm` job → `build-site`).
No per-commit binary churn; the demo always tracks `main`.

## Provenance (how to regenerate)

- Crate skeleton (`Cargo.toml`, `build.rs`, `wrapper.h`, `src/`): from
  `rubys/rbs@rust-wasm-bindings:rust/ruby-rbs-sys/`.
- `vendor/rbs/{include,src}`: the RBS C parser at the pinned tag **v4.0.2**
  (`rust/rbs_version`), exactly as the upstream `rake rust:rbs:sync` task
  vendors it: `git archive v4.0.2 -- include src`.
- `vendor/rbs/BSDL`: the RBS C parser's BSD-2-Clause license (it is BSD-2;
  roundhouse is MIT/Apache — compatible, attribution preserved here).

## Removing this (when #2992 lands)

When `ruby-rbs-sys` with the wasm support **publishes** to crates.io: delete
this directory and the `[patch.crates-io] ruby-rbs-sys` block in
`wasm/Cargo.toml`, then bump the dependency. The `build-wasm` CI job keeps
working unchanged (it still just needs the WASI SDK).
