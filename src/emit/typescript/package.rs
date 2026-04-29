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
    let mut includes = String::from("\"app/models/**/*.ts\", \"src/juntos.ts\"");
    if !app.models.is_empty() {
        includes.push_str(", \"src/schema_sql.ts\"");
    }
    if !app.controllers.is_empty() {
        includes.push_str(
            ", \"app/controllers/**/*.ts\", \"app/views/**/*.ts\", \"src/http.ts\", \"src/test_support.ts\", \"src/view_helpers.ts\", \"src/route_helpers.ts\", \"src/routes.ts\", \"src/server.ts\", \"main.ts\"",
        );
    }
    if !app.test_modules.is_empty() || !app.fixtures.is_empty() {
        includes.push_str(", \"spec/**/*.ts\"");
    }
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
