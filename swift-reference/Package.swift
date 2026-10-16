// swift-tools-version:5.10
// Swift 5 language mode (plan decision 5): no Sendable strict-concurrency
// checking on the deliberately thread-confined runtime state.
import PackageDescription

let package = Package(
    name: "swift-reference",
    platforms: [.macOS(.v14)],
    dependencies: [
        // Hummingbird 2.x is the locked HTTP server (plan decision 1).
        // Cap below 2.27 (tools-version 6.2 / broken swift-websocket on 6.2.1);
        // keep in sync with src/emit/swift/package.rs. Explicit
        // ServiceContextModule so reused SPM trees materialize the module
        // Hummingbird 2.26 imports.
        .package(url: "https://github.com/hummingbird-project/hummingbird.git", "2.26.0"..<"2.27.0"),
        .package(url: "https://github.com/apple/swift-service-context.git", from: "1.1.0"),
        // Direct import of NIOPosix (ThreadSpecificVariable, NIOThreadPool);
        // already in Hummingbird's tree, declared here for the direct import.
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.65.0"),
    ],
    targets: [
        // The system SQLite3 C API (plan decision 3). `import SQLite3` is
        // Apple-only; this systemLibrary target + module.modulemap is the
        // cross-platform spelling. Linux hosts need `libsqlite3-dev`.
        .systemLibrary(
            name: "CSQLite",
            providers: [.apt(["libsqlite3-dev"])]
        ),
        .executableTarget(
            name: "App",
            dependencies: [
                .product(name: "Hummingbird", package: "hummingbird"),
                .product(name: "ServiceContextModule", package: "swift-service-context"),
                .product(name: "NIOPosix", package: "swift-nio"),
                "CSQLite",
            ]
        ),
    ]
)
