# What's different about this attempt

If you arrived at this repo wondering "we've seen Rails-to-X transpilers before, why is this one different?" — the question is reasonable. Most attempts in this category are abandoned within a few years.

This document explains the structural differences in reference form. For the *why bother* argument (the constraints that push successful Rails apps off CRuby), see [WHY.md](WHY.md). For the long-form essay version with predecessor history, see [the blog post](https://intertwingly.net/blog/2026/05/04/Weve-Seen-This-Movie-Before.html).

## What's hard about this category

Most compile-Ruby projects haven't reached broad adoption, and the structural causes are mostly not technical:

1. **Two-communities problem** — the audience (Rubyists who need X-language deployment) is small.
2. **Ecosystem gap** — Rails plus gems is the value, not just Ruby; transpilers that don't bridge it deliver a fraction of the value.
3. **Pace mismatch** — small teams can't track Rails major versions plus dependency evolution forever.
4. **No clear differentiating narrative** — "why use this over the alternative" never has a sharp answer.

The projects that have sustained longest (mruby, JRuby, Crystal, Ruby2js+juntos) share at least one of: personal authority of a recognized name, commercial backing, or a specific defensible niche. Plus iteration time measured in decade-plus.

## Lineage

This is not a zero-to-N attempt. It's the third generation of a thirteen-year transpilation lineage by the same author.

- **Ruby2js** (2013–): Ruby-to-JavaScript transpiler. ~1,150 commits 2013–2024, plus 761 in 2025, plus 1,466 in January–April 2026, after which the work moved to roundhouse.
- **Juntos**: Rails-on-JS-runtimes layer built on Ruby2js. 8 demos shipping to browser (Dexie/IndexedDB), Node, Bun, Deno, Cloudflare Workers, Vercel Edge, Capacitor mobile, Electron+Tauri desktop. The demos prove "Rails-with-platform-native-capabilities" — audio recording, device APIs, mobile shells — not just "Rails-on-runtime." [Live demos](https://ruby2js.github.io/ruby2js/).
- **Railcar**: Crystal-based intermediate experiment. Surfaced that type inference is its own design space — Hindley-Milner is a starting point, every real system subsets or extends it, and Crystal's choices fit Crystal's needs not Rails-shape Ruby's. Crystal's `Semantic` IR is also internal-only, never intended as a public substrate. So roundhouse's type inference is built deliberately for the subset it serves.
- **Roundhouse**: this project. First commit 2026-04-17. Adds strong type inference up front. Ruby's polymorphism (`+` means addition / concatenation / runtime-raise depending on operand types) means correct emission to *any* target — dynamic or strict — requires knowing operand types at each site. Type inference is what lets the compiler pick the correct target syntax (`[...a, ...b]` vs `+` vs raise). It also enables strict-typed targets (Rust, Swift, Kotlin) as a consequence. See [Static Types for Dynamic Targets](https://intertwingly.net/blog/2026/04/23/Static-Types-for-Dynamic-Targets.html).

## The three bets

Roundhouse's intermediate representation is a **type-annotated spinel subset**. Spinel is matz's AOT Ruby compiler, defining the "no metaprogramming over runtime data" discipline. Type annotation is what roundhouse adds.

Three bets, each independently testable, with independent failure modes and backups:

### Bet 1: Developers want a Rails-DSL frontend over subset Ruby, plus reach beyond any single target

Spinel takes subset-clean Ruby to fast native binaries. Roundhouse and spinel are complementary: roundhouse adds the Rails-DSL frontend that lowers app code to subset-clean IR, and the multi-target reach where the IR feeds a dozen backends (spinel being the stay-in-Ruby native-binary one). Together they give developers what neither provides alone.

Thirteen years of Rails-as-preferred-Ruby-web-framework empirically supports the DSL-frontend half. The multi-target half is supported by ruby2js+juntos's existing demos shipping to seven JS-runtime deployment surfaces.

- **If wrong**: developers prefer to target the subset directly → spinel serves that audience.
- **Continuing value**: roundhouse continues to add value on the multi-target side (TS, Rust, Swift, Kotlin, Python, Elixir, Go, Crystal, C#) where spinel doesn't reach.
- **Reasoned probability estimate**: ~95%.

### Bet 2: Framework runtime in the subset + type inference covers the rest

Argued in detail in [Rails Was Already Typed](https://intertwingly.net/blog/2026/04/21/Rails-Was-Already-Typed.html). Schema + Rails conventions + standard inference materialize types without annotations.

The bet is load-bearing for *all* targets, not just strict-typed ones. Per [Static Types for Dynamic Targets](https://intertwingly.net/blog/2026/04/23/Static-Types-for-Dynamic-Targets.html): Ruby's polymorphism means the compiler can't pick the correct target syntax without operand types — `[1,2] + [3,4]` becomes `[...a, ...b]` in TS only if the compiler knows the operands are arrays. Type inference is what makes correct multi-target emission possible; the alternative is heuristic transpilation that's right most of the time and silently wrong the rest.

- **Empirical evidence**: no developer-written annotations anywhere. The analyzer types real applications from unmodified source; a pass over Mastodon (1,173 files, all 337 controllers) takes about 1.5 seconds. Each target is checked against live Rails by a conformance oracle (the same URL must produce the same response, DOM node for DOM node): the full target matrix runs in scheduled validation, and pull requests run a Ruby floor plus targeted additions. Spinel goes furthest: Basecamp's Campfire compiles to a native binary, with every page and every cable frame compared against live Rails in the full validation cycle.
- **Failure mode**: inference plateaus too high; framework runtime gaps don't close.
- **Backup**: RBS sidecars or inline pragmas as escape hatches for the residual.
- **Scoping**: 80% target, not perfection. Long-tail apps need refactoring of untypable hot spots.
- **Reasoned probability estimate**: ~85%.

### Bet 3: Transpilation is mechanical; ecosystems do the hard part

Roundhouse doesn't generate machine code, manage memory, schedule concurrency, or implement standard libraries. It produces source code in the target language; rustc / V8 / swiftc / kotlinc / spinel does the rest. The factoring is right because alternatives are an order of magnitude more work.

- **Mechanical-ness varies by target**:
  - TypeScript / Crystal / Kotlin / Swift: ~85–90%
  - Rust: ~65–70% (strict typing forces commits where dynamic Ruby was permissive)
  - Spinel: ~95% (identity-ish emit)
  - Paradigm shifts (Elixir pattern matching, Go's no-expression-if): ~70%
- **Failure mode**: target's semantic distance defeats mechanical translation.
- **Backup**: per-target manual emit work for the failed target; others continue.

### The combination is the differentiator

Each bet has been tried in isolation by predecessors. The combination has not.

| | Bet 1 (subset discipline) | Bet 2 (framework + inference) | Bet 3 (multi-target ecosystem) |
|---|---|---|---|
| Opal | Rejects (any Ruby) | No | Single-target, non-idiomatic output |
| Sorbet Compiler | Requires annotations | No | Single LLVM target; same deployment shape |
| RubyMotion | Partial | Limited | Single iOS target; hostile platform owner |
| Roundhouse | Accepts | Yes | A dozen targets, idiomatic output |

- **Opal** takes a different position on bet 1: accept all of Ruby (including metaprogramming) and pay for that with semantic-shim machinery in the output. The trade-off is that the output isn't idiomatic JavaScript, which bounds how naturally it integrates with the broader JS ecosystem. A real value proposition for the audience that wants exactly that trade-off; roundhouse makes a different one.
- **Sorbet Compiler** is the inverse: requires annotations, generates LLVM IR, produces native shared libraries that MRI loads via FFI. Speeds up CRuby on the same deployment shape; doesn't change deployment economics or reachability. Natural audience: shops that have already invested in Sorbet annotations.
- **RubyMotion** had commercial backing but was single-target dependent on Apple's platform direction. Once Swift emerged in 2014 as Apple's first-party language, the value proposition for Ruby-on-iOS narrowed. Roundhouse's multi-target architecture spreads platform-owner dependency across many platforms.

## Per-target technical viability

Per-target viability = P(bet 1) × P(bet 2) × P(bet 3 for that target). Multipliers are independent and all high; reasoned estimates only, not measurements.

| Target | Estimate |
|---|---|
| Spinel | ~77% |
| TypeScript / Kotlin / Swift | ~69% |
| Rust | ~52% |

These are *technical viability* numbers. **Project survival** = technical viability × adoption × execution × community, and the other factors are not estimated here.

## Acknowledged risks

- **Maintainer concentration.** The project started as one person's LLM-augmented effort. As of October 2026 it has 16 collaborators, Matz and Rafael França among them, and over 800 merged pull requests from 28 authors, Tobi Lütke among them. Review and design direction still run largely through the originator. Mitigated by documenting the architecture and the invariants contributors must keep ([AGENTS.md](AGENTS.md), [docs/](docs/)), and by ensuring useful artifacts exist even in failure.
- **Gem ecosystem coverage.** Metaprogramming-heavy gems are the long tail, covered as demand arrives rather than up front. Some are modeled in part (Devise's helpers and routes, Sidekiq workers). Pure-Ruby gems such as Faraday and JWT run as the real gem on the Ruby and JRuby targets, and the other targets report them unsupported. Others (Pundit, ActiveAdmin) remain out. Apps that need those gems can refactor (showcase, the author's own ballroom-dance-event app, uses custom auth instead of Devise — it works), wait, or use a different tool. The path is demand-driven contribution, not exhaustive pre-coverage.
- **Cultural memory inertia.** The "we've seen this movie" prior is reasonable. Updating it requires concrete evidence — demos that work, code that compiles, tests that pass.

## Bounded outcomes

The architecture is designed so partial failure leaves useful artifacts:

- **`runtime/ruby/`** — Rails reimplemented to be statically analyzable. Useful to spinel, to RBS, to anyone trying to type Rails — independent of whether roundhouse-as-multi-target-compiler succeeds.
- **`src/lower/`** — Rails-shape Ruby normalization. Research artifact even if no target ships in production.

Predecessors didn't have analogous fallbacks. The downside of this project isn't "wasted years."

## How to evaluate

- Try `bin/rh fixture && bin/rh dev ruby` for the working demo.
- Read [WHY.md](WHY.md) for the constraints argument.
- Read the [blog post](https://intertwingly.net/blog/2026/05/04/Weve-Seen-This-Movie-Before.html) for the long-form differentiation argument.
- Try the analyzer against your own Rails app (`roundhouse check --continue path/to/app`) — zero annotations required. If it types cleanly, you're inside the subset.
- File issues with concrete bug reports or feature requests.
- Contribute via LLM-augmented workflow when you have a specific need; [AGENTS.md](AGENTS.md) is the orientation.
