//! `package.json` + `tsconfig.json` emission for the TypeScript target.

use std::path::PathBuf;

use super::super::EmittedFile;
use crate::App;

/// Minimal package.json. `"type": "module"` matches the ESM import/
/// export style the emitter produces. The tsconfig `paths` alias
/// resolves `"juntos"` to our local stub.
///
/// Dependency set switches with the active deployment profile:
///   - SharedWorker (`worker`): vite + sqlite-wasm + Turbo, no node
///     deps. Scripts: dev/build/preview.
///   - Async node (`node-async`): libsql client + ws.
///   - Sync node (`node-sync`): better-sqlite3 + ws + node types.
pub(super) fn emit_package_json() -> EmittedFile {
    if crate::profile::active_http_shim() == crate::profile::HttpShim::SharedWorker {
        return emit_worker_package_json();
    }
    let async_profile = !crate::analyze::async_color::active_extern_async_names().is_empty();
    let (db_dep, db_types_dep) = if async_profile {
        ("    \"@libsql/client\": \"^0.14.0\",", "")
    } else {
        (
            "    \"better-sqlite3\": \"^12.11.2\",",
            "    \"@types/better-sqlite3\": \"^7.6.0\",\n",
        )
    };
    let content = format!(
        "{{\n  \"name\": \"app\",\n  \"version\": \"0.1.0\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"scripts\": {{\n    \"start\": \"tsx main.ts\",\n    \"test\": \"tsx --test test/*.test.ts\"\n  }},\n  \"dependencies\": {{\n{db_dep}\n    \"linkedom\": \"^0.18.0\",\n    \"ws\": \"^8.18.0\"\n  }},\n  \"devDependencies\": {{\n    \"@types/node\": \"^20\",\n{db_types_dep}    \"@types/ws\": \"^8.5.0\",\n    \"typescript\": \"5.7.3\",\n    \"tsx\": \"4.19.2\"\n  }}\n}}\n",
    );
    EmittedFile {
        path: PathBuf::from("package.json"),
        content,
    }
}

/// Worker-target package.json. Vite is the build toolchain — three
/// rollup entries (main / worker / db_worker) bundled into
/// fingerprinted assets. sqlite-wasm + Turbo are runtime deps; no
/// node packages because the whole app runs in the browser.
fn emit_worker_package_json() -> EmittedFile {
    let content = "{
  \"name\": \"app\",
  \"version\": \"0.1.0\",
  \"private\": true,
  \"type\": \"module\",
  \"scripts\": {
    \"dev\": \"vite\",
    \"build\": \"vite build\",
    \"preview\": \"vite preview\"
  },
  \"dependencies\": {
    \"@hotwired/turbo\": \"^8.0.0\",
    \"@sqlite.org/sqlite-wasm\": \"^3.47.0-build1\",
    \"linkedom\": \"^0.18.0\"
  },
  \"devDependencies\": {
    \"@tailwindcss/vite\": \"^4.0.0\",
    \"tailwindcss\": \"^4.0.0\",
    \"typescript\": \"5.7.3\",
    \"vite\": \"^8.0.0\"
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
        String::from("\"app/**/*.ts\", \"src/**/*.ts\", \"db/**/*.ts\", \"main.ts\"");
    if crate::profile::active_http_shim() == crate::profile::HttpShim::SharedWorker {
        // Worker target has additional entry points alongside main.ts.
        includes.push_str(", \"worker.ts\"");
    }
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
