//! Emitted SPM scaffold must keep Hummingbird on the ServiceContextModule line
//! and declare that product explicitly.
//!
//! Showcase `swift build -c release` under a reused `/tmp/rh-swift-pass2` tree
//! resolved Hummingbird 2.26.x but missed `ServiceContextModule`. Cap below
//! 2.27 (tools-version 6.2 / broken swift-websocket on 6.2.1) and force the
//! product into the App target. Gate is text-only — no Swift toolchain needed.

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::swift;

const HUMMINGBIRD_RANGE: &str = r#""2.26.0"..<"2.27.0""#;
const SERVICE_CONTEXT_PRODUCT: &str =
    r#".product(name: "ServiceContextModule", package: "swift-service-context")"#;

#[test]
fn emitted_package_swift_caps_hummingbird_and_declares_service_context() {
    let files = swift::emit(&App::new());
    let package = files
        .iter()
        .find(|f| f.path == PathBuf::from("Package.swift"))
        .expect("emit must produce Package.swift");
    assert!(
        package.content.contains(HUMMINGBIRD_RANGE),
        "Package.swift must cap Hummingbird below 2.27:\n{}",
        package.content
    );
    assert!(
        package.content.contains("swift-service-context.git"),
        "Package.swift must declare swift-service-context:\n{}",
        package.content
    );
    assert!(
        package.content.contains(SERVICE_CONTEXT_PRODUCT),
        "App target must depend on ServiceContextModule:\n{}",
        package.content
    );
}

#[test]
fn swift_reference_package_stays_in_sync_with_emit_pin() {
    let reference = include_str!("../swift-reference/Package.swift");
    assert!(
        reference.contains(HUMMINGBIRD_RANGE),
        "swift-reference/Package.swift must match emit Hummingbird range"
    );
    assert!(
        reference.contains(SERVICE_CONTEXT_PRODUCT),
        "swift-reference/Package.swift must declare ServiceContextModule"
    );
}
