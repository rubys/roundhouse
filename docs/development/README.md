# Contributor development

This handbook is for changing Roundhouse. For using it on a Rails app, read
the [user guide](../guide/README.md). [AGENTS.md](../../AGENTS.md) holds the
invariants; [RELEASES.md](../../RELEASES.md) records dated release claims.

## Setup

Install Rust through rustup; the selected toolchain is
[`rust-toolchain.toml`](../../rust-toolchain.toml), not Cargo's minimum
`rust-version`. Keep the stack budgets in [`.cargo/config.toml`](../../.cargo/config.toml).
Toolchain upgrades need native and browser/WASM verification.

Use the MRI line in [`.ruby-version`](../../.ruby-version). CI selects its
latest patch through `env.MRI_RUBY` in the workflow; `bin/rh doctor` reads
the local minimum from that file. Git and Ruby run `bin/rh verify --plan`; execution
also needs Cargo. Python 3 is optional for its hosted-coverage preview.
The default suite loads Ruby gems as well as Rust code. Prepare them and
the generated fixtures:

```sh
gem install rails rails-html-sanitizer sqlite3 bcrypt rack minitest rake --no-document
gem install activerecord -v '~> 8.1.0' --no-document
(cd runtime/spinel/scaffold && bundle install)   # framework_tests_ruby runs through this bundle
bin/rh fixture
(cd fixtures && ../scripts/create-store store)
bin/rh doctor
cargo build --locked
```

Selected ignored integrations need their own SDKs and dependencies; see the
harness you intend to run. `doctor` reports installed tools, not complete
test readiness. Fixture generation uses Rails and can change with its release.

For the tracked Campfire-to-Rust milestone, including the compiler inventory,
parallel work boundaries, and separate compile/runtime acceptance gates, see
the [Campfire Rust plan](campfire-rust-plan.md).

## Local loop

1. Read the [compiler ownership map](compiler-changes.md) for the affected path.
2. Pin the intended behavior with a regression test. Pick inputs that distinguish
   the fix from a plausible wrong implementation, not just a no-crash case.
3. Iterate with [focused tests](testing.md), inspecting [IR/output](debugging.md)
   when needed. Removing an error diagnostic requires emitted execution coverage.
4. Run the default `cargo test` suite before committing; use
   `cargo test --all-targets` at milestones. A focused pass is not full CI.

For a PR, include the repro, regression test, and what you actually verified.
Contributors use a fork and PR against upstream main; committers follow the
repository's main-branch convention. Stage only your own changes and include
the standard `Co-Authored-By` trailer. Missing local SDK coverage must be
reported, not described as passing; request [broader CI](../ci/README.md) when needed.

## Repository workflows

`bin/rh --help` lists commands; each subcommand has `--help`.

| Workflow | Command |
|---|---|
| Inspect prerequisites / fetch an archive | `bin/rh doctor` / `bin/rh fetch <target>` |
| Emit the blog fixture | `bin/rh transpile <target>` |
| Run the emitted Ruby app | `bin/rh dev ruby` (also `test ruby`, `run ruby`) |
| Compare against Rails | `bin/rh compare <target>` |
| Benchmark / build the complete site | `bin/rh bench` / `bin/rh site` |

`bin/rh clean <target>` removes that emitted build; `bin/rh clean fixture`
removes real-blog, not Store. Neither is a Cargo-cache cleanup command.
