//! Emitted SPM scaffold must pin Hummingbird past the ServiceContextModule era.
//!
//! Showcase `swift build -c release` under a reused `/tmp/rh-swift-pass2` tree
//! resolved Hummingbird 2.26.x (which `import ServiceContextModule`) while CI's
//! fresher cache stayed green. The floor is `from: "2.28.0"` in the shared emit
//! home (`src/emit/swift/package.rs`); `swift-reference/Package.swift` stays in
//! sync. This gate does not need a Swift toolchain — only the emitted text.

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::swift;

const HUMMINGBIRD_PIN: &str = r#"from: "2.28.0""#;

#[test]
fn emitted_package_swift_pins_hummingbird_past_service_context_module() {
    let files = swift::emit(&App::new());
    let package = files
        .iter()
        .find(|f| f.path == PathBuf::from("Package.swift"))
        .expect("emit must produce Package.swift");
    assert!(
        package.content.contains(HUMMINGBIRD_PIN),
        "Package.swift must pin Hummingbird ≥2.28.0 (dropped ServiceContextModule):\n{}",
        package.content
    );
    assert!(
        package
            .content
            .contains("hummingbird-project/hummingbird.git"),
        "Package.swift must still declare the Hummingbird package"
    );
}

#[test]
fn swift_reference_package_stays_in_sync_with_emit_pin() {
    let reference = include_str!("../swift-reference/Package.swift");
    assert!(
        reference.contains(HUMMINGBIRD_PIN),
        "swift-reference/Package.swift must match src/emit/swift/package.rs Hummingbird pin"
    );
}
