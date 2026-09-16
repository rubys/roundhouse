# Targets

Thirteen names are accepted by `--target`. Ten are server targets that
emit a complete HTTP + Action Cable application; the rest are
variations on those. This page says what each needs and how far each
is tested — which is the honest measure of how much to trust it.

## The server targets

| Target | Emits | Prerequisites to build and run |
|---|---|---|
| `rust` | Cargo crate (axum, rusqlite) | Rust 1.85+, SQLite library |
| `go` | Go module | Go 1.24+ |
| `typescript` | Node package | Node.js 18+ |
| `crystal` | shard | Crystal 1.10+, SQLite library |
| `elixir` | Mix project | Elixir 1.15+ |
| `kotlin` | Gradle build (JVM) | JDK 17+, Gradle 8+ |
| `swift` | Swift package | Swift 6+; on Linux `libsqlite3-dev` |
| `python` | Python project (`uv`) | Python 3.11+, `uv` |
| `csharp` | .NET solution | .NET SDK 10+ |
| `ruby` | Ruby tree — the framework runtime in Ruby, no Rails | Ruby 3.4+, bundler, SQLite; Node for the asset build |

All ten serve on `:3000`, speak Action Cable at `/cable`, use SQLite at
`storage/development.sqlite3`, and are seeded by
`sqlite3 storage/development.sqlite3 < db/seed.sql`. Each ships the
app's model and controller tests and a Playwright `e2e/` suite; the
`sqlite3` CLI and Node.js 18+ are needed for the latter.

## The variations

| Target | What it is |
|---|---|
| `jruby` | The `ruby` emit with prebuilt assets and JRuby run/test commands. JRuby 10+ (JDK 21+). |
| `spinel` | The `ruby` shape packaged as a `spin` project for ahead-of-time compilation to a native binary. Needs the Spinel compiler; [`spinel.md`](spinel.md). |
| `typescript-worker` | The `typescript` emit bundled to run in a browser `SharedWorker`, with SQLite compiled to WebAssembly, for the in-browser demos. Node.js 18+ to bundle; no server. |

## How far each is tested

Every claim below is a CI job on every push to `main`, all of them
against the blog fixture (`fixtures/real-blog`: articles, comments,
nested routes, validations, Turbo Streams over Action Cable, Tailwind)
unless another app is named. A target's row in
[`RELEASES.md`](../../RELEASES.md) records where these stood at the
snapshot.

| Lane | What it proves | Targets |
|---|---|---|
| **compare** | The emitted server renders the same DOM as live Rails, node for node, on every page of the fixture; JSON endpoints value for value. [`verifying.md`](verifying.md). | rust, go, typescript, crystal, elixir, kotlin, swift, python, csharp, ruby, jruby, spinel |
| **smoke** | The emitted README's own commands — build, setup, test, e2e in a real browser — run verbatim from the published archive. | rust, go, typescript, crystal, elixir, kotlin, swift, python, csharp, ruby, jruby, spinel |
| **framework tests** | The framework runtime's own test suite (`runtime/ruby/test/`) passes when transpiled to the target — the runtime is *itself* transpiled, so this is the target's Active Record, Action Controller and Action View under test. | crystal, kotlin, swift, typescript, ruby, spinel; rust on a subset |
| **toolchain** | Unit tests emitted for a smaller fixture (`tiny-blog`) compile and pass — the absent-feature shape the full blog can't exercise. | crystal, elixir, python, typescript, csharp, spinel (go, rust, kotlin, swift are covered by smoke instead and keep their harness for the dev loop) |
| **Campfire** | Basecamp's Campfire — attachments, rich text, web push, bots, search, boosts, multiple rooms and users — compared against live Rails page by page, its own test suite run against the transpile, and its Action Cable broadcasts checked frame for frame. | ruby, spinel |

The last row is the important asymmetry. The nine compiled and JVM
targets are proven on the blog; the Ruby shape — on CRuby, and
compiled by Spinel — is proven on Campfire, a real product with a real
surface, and is the lane the framework runtime is developed against
first. When a Rails feature lands in roundhouse it lands there, and
reaches the other targets as their emitters and runtimes catch up.
[`rails-coverage.md`](rails-coverage.md) is the feature-level view of
that gap.

## The conformance bar

A target is on the list above because it passes `compare` on the
fixture on every push. A page that renders one node differently — an
attribute missing, a whitespace text node — is red. That is
deliberately a higher bar than "the tests pass": it means the emitted
app is the same app, as a browser or a scraper would see it, not a
reimplementation that happens to agree on the tested paths.

The cost of the bar is that a target can drop off it. A target whose
compare lane goes red on `main` and stays red is not on the list for
the next snapshot, and its row in `RELEASES.md` says so.
