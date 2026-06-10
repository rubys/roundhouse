//! Ecosystem files for the emitted Swift project — the SPM scaffold.
//! The analog of `src/emit/kotlin/package.rs` (which emits the Gradle
//! scaffold). Locked stack: Swift Package Manager, Hummingbird 2.x
//! (HTTP), the system SQLite3 C API via a CSQLite systemLibrary target
//! (DB) — see `docs/swift-migration-plan.md`.

use std::path::PathBuf;

use crate::emit::EmittedFile;

// Swift 5 language mode (tools-version 5.10): no Sendable
// strict-concurrency checking on the deliberately thread-confined runtime
// state. Kept in sync with `swift-reference/Package.swift`.
const PACKAGE_SWIFT: &str = r#"// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "roundhouse-app",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/hummingbird-project/hummingbird.git", from: "2.5.0"),
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.65.0"),
    ],
    targets: [
        .systemLibrary(
            name: "CSQLite",
            providers: [.apt(["libsqlite3-dev"])]
        ),
        .executableTarget(
            name: "App",
            dependencies: [
                .product(name: "Hummingbird", package: "hummingbird"),
                .product(name: "NIOPosix", package: "swift-nio"),
                "CSQLite",
            ]
        ),
"#;

// Appended when the app carries test modules: XCTest target depending
// on the executable (supported since Swift 5.5 — the test bundle
// @testable-imports App; SwiftPM handles the executable's main). Only
// declared when Tests/AppTests/ exists — SPM errors on a declared
// target with no source directory.
const PACKAGE_SWIFT_TEST_TARGET: &str = r#"        .testTarget(
            name: "AppTests",
            dependencies: ["App"]
        ),
"#;

const PACKAGE_SWIFT_CLOSE: &str = r#"    ]
)
"#;

// `import SQLite3` is Apple-only; this systemLibrary target is the
// cross-platform spelling of the system SQLite3 C API. Linux hosts need
// `libsqlite3-dev` (the `.apt` provider hint above).
const CSQLITE_MODULEMAP: &str = "module CSQLite [system] {\n    header \"shim.h\"\n    link \"sqlite3\"\n    export *\n}\n";

const CSQLITE_SHIM_H: &str = "#include <sqlite3.h>\n";

const GITIGNORE: &str = "/.build/\n/storage/\n";

/// The SPM scaffold files (`has_tests` appends the AppTests target).
pub fn scaffold(has_tests: bool) -> Vec<EmittedFile> {
    let mut package = PACKAGE_SWIFT.to_string();
    if has_tests {
        package.push_str(PACKAGE_SWIFT_TEST_TARGET);
    }
    package.push_str(PACKAGE_SWIFT_CLOSE);
    vec![
        EmittedFile {
            path: PathBuf::from("Package.swift"),
            content: package,
        },
        EmittedFile {
            path: PathBuf::from("Sources/CSQLite/module.modulemap"),
            content: CSQLITE_MODULEMAP.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/CSQLite/shim.h"),
            content: CSQLITE_SHIM_H.to_string(),
        },
        EmittedFile {
            path: PathBuf::from(".gitignore"),
            content: GITIGNORE.to_string(),
        },
    ]
}
