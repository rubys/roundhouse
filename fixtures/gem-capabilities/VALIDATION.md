# Executed snapshot and open-gap ledger

2026-09-30, Ruby 4.0.7p0 / Bundler 2.6.9 / Cargo 1.98.1 / GCC 12.2.0.
Roundhouse was fetched directly from upstream main and rebuilt from a fresh
`thomasklemm/roundhouse` checkout, with `rubys/roundhouse` as upstream.
Chosen compiler pin: `f3d192810b53c61a73a52017e23d67df80265f72`;
compiler baseline tree: `7d70768281220df2154b41caa41283dce8f7bd91`.
The only source differences are the new corpus and its CLI; no compiler fix.
Spinel: `7147b681fd885ac1038f78474b010f2ab2d58402`.

Actual final build binaries:

- Roundhouse SHA256 `afb3ad6bbd3e5591e5069c9cbc196ecdd7e3ab29ec360062b63624f04e1323ef`
- Spinel SHA256 `86afab295eb0b0e15643ea3d4ed4d4ce23d40f3e595c95709e8f712c67f822d5`

Both source/build/binary receipts and post-run provenance verified. Full argv,
exits, stdout/stderr and hashes were retained in the execution result directory;
the table is a summary, not a replacement for them. Spinel was force-rebuilt with
`make -B -j4` (exit0, 24,753 stdout bytes), including ignored objects/runtime/package
artifacts; the earlier incremental receipt is no longer accepted. Its rebuilt
binary hash happened to match the prior binary, rather than being assumed fresh.

| Case | CRuby original | Roundhouse Ruby no-preload boot + contract | Original source Spinel compile | Roundhouse Spinel compile → binary |
|---|---|---|---|---|
| historical Alba 4.0.0 hash contract | pass | runtime-fail: Alba absent | rejected: optional `active_support/inflector` require | 0 → 1: `ArticleResource#to_h` missing |
| historical dry-validation 1.11.1 | pass | runtime-fail: Dry absent | rejected: `pp` require | 0 → 1: `SignupContract#call` missing |
| historical bcrypt 3.1.22 | pass | pass | rejected: `java` require in original source; native C extension not compiled | rejected: `bcrypt` require; binary not-run |
| new declarations-alba (4.0.0) | pass | runtime-fail: Alba absent | rejected: optional `active_support/inflector` require | 0 → 1: `EntryResource#to_h` missing |
| new declarations-dry (1.11.1) | pass | runtime-fail: Dry absent | rejected: `pp` require | 0 → 1: `AdultContract#call` missing |

All five library strict/continue checks and all five ordinary emissions exit0.
Analyzed controller strict/continue checks also exit0; their P/E/W/N/G counters:

| Case | Consumer P/E/W/N/G | Ruby lowering residues |
|---|---|---|
| historical Alba | 0/0/0/1/0 | 4 |
| historical dry | 0/0/9/0/0 | 0 |
| historical bcrypt | 0/0/0/0/0 | 0 |
| declarations-alba | 0/0/0/2/0 | 5 |
| declarations-dry | 0/0/18/0/0 | 2 |

All emitted Ruby syntax checks, output installs and output bundle checks passed.
Every original lock and emitted source/manifest remained unchanged. Output lock
SHA256: Alba/dry inputs
`b7107d328fe621df71244b89ab6afe09b7cde3150260c9d9903468565acd9a34`;
bcrypt `781c446af71e7d155fc5bbcd88b51a7f36d6a6d1bf6efaf4072acb0e36dfa92c`.
Installing a declared output bundle does not make missing dependencies present.
No manual preloads, original-class injection, native packages or JSON substitutes
were used to turn a failed lane into a pass. The other eleven historical cases
remain preserved and `not-run` in this new execution, not silently omitted.
Original transitive source-provider proof remains `blocked`: Spinel's bundled
libraries/packages precede explicit `-I` roots. Original entry files are selected
directly, but a future compile/binary success must not imply that all original
dependencies (especially CRuby extensions) were compiled. A runner regression
pins that blocked result for both pure-Ruby and native-extension projections.

## Exact executed commands

The orb installed Ruby4/Rust via mise; the prerequisite bindgen build failure
(missing libclang) was resolved by installing `libclang-dev`. No source patch.

```sh
mise exec ruby@4.0.7 rust@1.98.1 -- ruby fixtures/gem-capabilities/build.rb \
  roundhouse /home/user/workspace/roundhouse-capabilities \
  /home/user/workspace/capability-build-roundhouse-final
mise exec ruby@4.0.7 rust@1.98.1 -- ruby fixtures/gem-capabilities/build.rb \
  spinel /home/user/workspace/references/compiler/spinel \
  /home/user/workspace/capability-build-spinel-forced
mise exec ruby@4.0.7 -- ruby scripts/check-gem-capabilities.rb \
  --out /home/user/workspace/capability-core-reviewed \
  --roundhouse /home/user/workspace/capability-build-roundhouse-final/receipt.json \
  --spinel /home/user/workspace/capability-build-spinel-forced/receipt.json
# Exit1: failed lanes are the recorded outcome, not a green compatibility gate.
mise exec ruby@4.0.7 -- ruby scripts/check-gem-capabilities.rb --reference-only \
  --cases declarations-alba,declarations-dry \
  --out /home/user/workspace/capability-new-reference-final
# Exit0: both real gem reference contracts pass.
mise exec ruby@4.0.7 -- ruby scripts/check-gem-capabilities.rb --cases alba \
  --out /home/user/workspace/capability-missing-provenance-final
# Exit1: original reference pass; all three compiler lanes provenance-missing.
mise exec ruby@4.0.7 -- ruby fixtures/gem-capabilities/test_runner.rb
# 11 runs, 202 assertions, 0 failures, 0 errors, 0 skips.
```

Existing scoped Rust tests `gem_ancestry_attribution` (8) and
`scaffold_gemfile_declares_bundled_gems` (1) passed under the same exclusive Cargo
target. `cargo test --lib`: **826 passed, 40 failed**, caused by the absent generated
`fixtures/real-blog` tree in the fresh checkout. No fixture-generation/app/DB lane
was started to conceal that limitation. Whole-project Spin build/Rails engine,
full app integration, other targets and the remaining historical contracts were
not run. The successful Spinel compiles above are module artifact successes,
explicitly not runtime passes.
