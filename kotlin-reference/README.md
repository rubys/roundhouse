# Kotlin target — Phase R reference

Hand-written reference output for the Kotlin target (see
`docs/kotlin-migration-plan.md`). This is the **forcing function**: a
minimal, compileable Kotlin app that serves `GET /articles` from the
real-blog sqlite DB, in the shape the `src/emit/kotlin` emitter must
reproduce. It is not generated — it is the spec the generator is driven
toward.

Scope: the `/articles` index slice only. Every file is transcribed from
the lowered IR (`./target/debug/dump_ir fixtures/real-blog --format ruby`)
or modeled on `runtime/crystal/` (the closest strict-typed analog).

Layout:
- `runtime/` — hand-written per-target primitives (`Db.kt` = xerial JDBC
  SQLite, `Server.kt` = Javalin, `ParamValue.kt`).
- `framework/` — samples of the framework runtime that is normally
  *transpiled* from `runtime/ruby/` (`Inflector.kt`, `ViewHelpers.kt`,
  `RouteHelpers.kt`).
- `app/` — the generated-code shapes (`Article.kt`, `Comment.kt`,
  `ArticlesController.kt`, `ArticlesView.kt`).

Run:

```sh
# stage the seeded DB (quiescent copy through Rails' WAL)
mkdir -p storage
sqlite3 ../fixtures/real-blog/storage/development.sqlite3 ".backup storage/development.sqlite3"

PORT=9000 ./gradlew run
curl -s localhost:9000/articles | head
```
