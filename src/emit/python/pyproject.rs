//! Project config — emits `pyproject.toml`.

use std::path::PathBuf;

use super::super::EmittedFile;

/// Emit `pyproject.toml` at the project root. Declares `aiohttp`
/// as the runtime dep so `uv run python3 -m app` (the invocation
/// pattern railcar adopted) resolves and installs it on first run.
/// Test extras pull in pytest tooling; roundhouse's generated
/// tests use stdlib `unittest`, so the test extras are forward-
/// looking (align with railcar) and not required by the current
/// python_toolchain suite.
pub(super) fn emit_py_pyproject() -> EmittedFile {
    let content = "\
[project]
name = \"app\"
version = \"0.1.0\"
requires-python = \">=3.11\"
dependencies = [\"aiohttp\"]

[project.optional-dependencies]
test = [\"pytest\", \"pytest-aiohttp\", \"pytest-asyncio\"]

[tool.pytest.ini_options]
asyncio_mode = \"auto\"
";
    EmittedFile {
        path: PathBuf::from("pyproject.toml"),
        content: content.to_string(),
    }
}
