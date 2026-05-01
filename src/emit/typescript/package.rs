//! `package.json` + `tsconfig.json` emission for the TypeScript target.

use std::path::PathBuf;

use super::super::EmittedFile;
use crate::App;

/// Minimal package.json. `"type": "module"` matches the ESM import/
/// export style the emitter produces. `@types/node` is required so
/// tsc can resolve `node:test` / `node:assert/strict` imports in the
/// emitted spec files. The tsconfig `paths` alias resolves `"juntos"`
/// to our local stub.
pub(super) fn emit_package_json() -> EmittedFile {
    let content = "\
{
  \"name\": \"app\",
  \"version\": \"0.1.0\",
  \"private\": true,
  \"type\": \"module\",
  \"scripts\": {
    \"start\": \"tsx main.ts\"
  },
  \"dependencies\": {
    \"better-sqlite3\": \"^11.5.0\",
    \"ws\": \"^8.18.0\"
  },
  \"devDependencies\": {
    \"@types/node\": \"^20\",
    \"@types/better-sqlite3\": \"^7.6.0\",
    \"@types/ws\": \"^8.5.0\",
    \"typescript\": \"5.7.3\",
    \"tsx\": \"4.19.2\"
  }
}
";
    EmittedFile {
        path: PathBuf::from("package.json"),
        content: content.to_string(),
    }
}

/// tsconfig.json — strict TS with the two bits that matter for the
/// generated shape: `paths` maps `"juntos"` to the local stub, and
/// `allowJs`/`esModuleInterop` let imports in both styles resolve.
/// As of Phase 4c controllers + http runtime land in the include list
/// since they compile against the `Roundhouse.Http` stubs; views and
/// routes still wait for their own runtime.
pub(super) fn emit_tsconfig_json(app: &App) -> EmittedFile {
    // Catch-all glob: every emitted .ts file is included. The
    // generated app + framework runtime live under fixed roots
    // (`app/`, `src/`, `test/`); a top-level `**/*.ts` would also
    // sweep node_modules — instead enumerate the roots explicitly.
    let mut includes =
        String::from("\"app/**/*.ts\", \"src/**/*.ts\"");
    if !app.test_modules.is_empty() || !app.fixtures.is_empty() {
        includes.push_str(", \"test/**/*.ts\"");
    }
    let _ = app; // app currently unused after the include simplification.
    let content = format!(
        "{{
  \"compilerOptions\": {{
    \"target\": \"ES2022\",
    \"module\": \"ESNext\",
    \"moduleResolution\": \"bundler\",
    \"strict\": false,
    \"esModuleInterop\": true,
    \"skipLibCheck\": true,
    \"noEmit\": true,
    \"baseUrl\": \".\",
    \"paths\": {{
      \"juntos\": [\"./src/juntos.ts\"]
    }}
  }},
  \"include\": [{includes}]
}}
"
    );
    EmittedFile {
        path: PathBuf::from("tsconfig.json"),
        content,
    }
}
