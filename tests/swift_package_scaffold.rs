//! Emitted SPM scaffold keeps Hummingbird at 2.28 or later, which no longer
//! imports ServiceContextModule (2.26 did, and a reused showcase SPM tree
//! under `/tmp/rh-swift-pass2` missed it). Gate is text-only — no Swift
//! toolchain needed.

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::swift;

const HUMMINGBIRD_FLOOR: &str =
    r#".package(url: "https://github.com/hummingbird-project/hummingbird.git", from: "2.28.0")"#;

#[test]
fn emitted_package_swift_floors_hummingbird_at_2_28() {
    let files = swift::emit(&App::new());
    let package = files
        .iter()
        .find(|f| f.path == PathBuf::from("Package.swift"))
        .expect("emit must produce Package.swift");
    assert!(
        package.content.contains(HUMMINGBIRD_FLOOR),
        "Package.swift must take Hummingbird from 2.28.0:\n{}",
        package.content
    );
    assert!(
        !package.content.contains("swift-service-context"),
        "Package.swift no longer needs swift-service-context:\n{}",
        package.content
    );
}

#[test]
fn swift_reference_package_stays_in_sync_with_emit_pin() {
    let reference = include_str!("../swift-reference/Package.swift");
    assert!(
        reference.contains(HUMMINGBIRD_FLOOR),
        "swift-reference/Package.swift must match emit Hummingbird floor"
    );
}
