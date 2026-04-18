<p align="center">
  <img src="assets/logo.svg" alt="Roundhouse logo — a turntable at the center, six colored tracks radiating outward" width="200">
</p>

# Roundhouse

*Rails as a specification; deployment is a build flag.*

Roundhouse reads Ruby source — specifically, Rails applications — and
produces standalone projects in other target languages. The deployment
target (Rust binary, TypeScript bundle, Elixir OTP app, …) becomes a
compiler flag rather than a runtime choice.

A roundhouse is the circular hub in a rail yard where engines rotate and
route onto different tracks. That's the pipeline shape: one Ruby source
at the center, analyzed and dispatched to one of N target tracks.

## Pipeline

```
          ingest       analyze        lower         emit
Ruby ────▶ AST ─────▶ typed IR ────▶ IR ─────▶ target project
                         │
                         ▼
                    diagnostics
```

Ingest normalizes Ruby + ERB into a small typed IR. Analyze annotates
every expression with a type and effect set, flowing types along the
edges Rails conventions already draw (schema → models, associations,
before_action, render → view, partials). Lower expands Rails-dialect
nodes into target-neutral IR. Emit walks the IR per target, consulting
each expression's type where the target needs it.

Diagnostics surface anything the analyzer couldn't type — the subset
of programs we can transpile is defined by "zero diagnostics."

## Current state

Early. The analyzer fully types a basic Rails 8 MVC fixture
(`fixtures/real-blog`) without annotations — schema-derived attributes,
associations, controller actions, `before_action` flow, views,
partials, and collection rendering all resolve to concrete types.
A test enforces zero diagnostics on every commit.

Six target emitters are scaffolded; none currently produces runnable
output. Foundations (typed IR, dialect channels, diagnostic
infrastructure) landed first by design — the hardest invariants to
retrofit go in before there's downstream surface depending on the
old shape.

## Running the tests

```
cargo test
```

## Prior art

- [railcar](https://github.com/rubys/railcar) — the Crystal-based predecessor; taught us which bets were worth keeping and where the shape needed to change.
- [ruby2js](https://www.ruby2js.com) — transpiles Ruby to JavaScript; originator of the filter/escape-hatch pattern for per-app transformations.
- [Juntos](https://www.ruby2js.com/docs/juntos/) — ruby2js extension that transpiles entire Rails apps; validated the multi-target ambition against Basecamp's Writebook.

## Contributing

Issues and discussion are welcome. Architecture is still forming —
a quick conversation before a PR is usually the most helpful path.

## License

Dual-licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
