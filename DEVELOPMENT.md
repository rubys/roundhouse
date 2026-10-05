# Developing Roundhouse

Start with the [development handbook](docs/development/README.md) for setup
and the local loop. Read [AGENTS.md](AGENTS.md) for invariants before changing
compiler behavior.

| Task | Reference |
|---|---|
| Choose tests or use `bin/rh verify` | [Testing](docs/development/testing.md) |
| Inspect AST, lowered IR, or emitted code | [Debugging](docs/development/debugging.md) |
| Change IR, lowering, runtime, or emitters | [Compiler changes](docs/development/compiler-changes.md) |
| Understand/request GitHub checks | [CI for contributors](docs/ci/README.md) |
| Understand the pipeline | [Architecture](docs/pipeline/) and [inputs](docs/data/) |

## Fixtures

`fixtures/real-blog` and `fixtures/store` are generated, not checked in.
Tests using [`src/fixtures.rs`](src/fixtures.rs) report the generating command
when either is absent. From the repository root:

```sh
bin/rh fixture
(cd fixtures && ../scripts/create-store store)
```

Prerequisites and checked-in fixtures: [testing](docs/development/testing.md#fixtures).
Using Roundhouse rather than developing it: [user guide](docs/guide/README.md).
