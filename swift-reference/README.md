# Swift target — Phase R reference

Hand-written reference output for the Swift target (see
`docs/swift-migration-plan.md`). This is the **forcing function**: a
minimal, compileable Swift app that serves `GET /articles` from the
real-blog sqlite DB, in the shape the `src/emit/swift` emitter must
reproduce. It is not generated — it is the spec the generator is driven
toward.

Scope: the `/articles` index slice only. Every file is transcribed from
the lowered IR (`./target/debug/dump_ir fixtures/real-blog --format ruby`)
via `kotlin-reference/` (the completed near-exact template — see the plan).

Layout:
- `Sources/CSQLite/` — systemLibrary target wrapping the system SQLite3
  C API (`import SQLite3` is Apple-only; this modulemap is the
  cross-platform spelling; Linux needs `libsqlite3-dev`).
- `Sources/App/runtime/` — hand-written per-target primitives (`Db.swift`
  = SQLite3 C API, thread-confined via `ThreadSpecificVariable`;
  `Server.swift` = Hummingbird 2 with the `NIOThreadPool.runIfActive`
  whole-request bridge; `ParamValue.swift`).
- `Sources/App/framework/` — samples of the framework runtime that is
  normally *transpiled* from `runtime/ruby/` (`Inflector.swift`,
  `ViewHelpers.swift`, `RouteHelpers.swift`). Ruby modules → caseless
  `enum` namespaces of static functions.
- `Sources/App/app/` — the generated-code shapes (`Article.swift`,
  `Comment.swift`, `ArticlesController.swift`, `ArticlesView.swift`).

Run (macOS):

```sh
# stage the seeded DB (quiescent copy through Rails' WAL)
mkdir -p storage
sqlite3 ../fixtures/real-blog/storage/development.sqlite3 ".backup storage/development.sqlite3"

PORT=9000 swift run
curl -s localhost:9000/articles | head
```

Build (Linux — the CI/bench platform; de-risks CSQLite + Foundation):

```sh
docker run --rm -v "$PWD":/work -w /work swift:6.1 \
  bash -c 'apt-get update -q && apt-get install -y -q libsqlite3-dev && swift build'
```
