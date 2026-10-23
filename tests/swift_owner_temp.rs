//! A local bound from a controller's `@ivar` on Swift. The ivar is
//! declared implicitly unwrapped (`var article: Article!`, Swift's
//! lateinit), and a member read on it unwraps it implicitly. A local
//! inferred from it is a plain `Article?` instead, so the association-
//! build temp `let rhBuildOwner0 = article` (#1181) made the next
//! `rhBuildOwner0.id` a compile error (`value of optional type
//! 'Article?' must be unwrapped`). The local now takes the ivar's
//! non-optional type, which unwraps it as the member read did. The
//! compile itself is `swift_toolchain` / `emit_and_run`'s ignored Swift
//! tests, which CI runs.

use roundhouse::project::BuildTarget;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

#[test]
fn an_owner_temp_from_an_implicitly_unwrapped_ivar_is_not_optional() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:#?}");
    let controller = std::fs::read_to_string(emitted.join("Sources/App/app/controllers/CommentsController.swift")).expect("CommentsController.swift");
    assert!(controller.contains("var article: Article!"), "{controller}");
    assert!(controller.contains("let rhBuildOwner0: Article = article"), "{controller}");
    assert!(controller.contains("rhBuildOwner0.id"), "{controller}");
}

/// Only the build's owner temp: a model's lazy cache reader binds its
/// implicitly-unwrapped ivar to a local precisely to test it for nil
/// (`let cached = featuredImageCache; if !(cached == nil)`), so that local
/// stays optional.
#[test]
fn a_local_that_tests_an_ivar_for_nil_stays_optional() {
    let out = std::env::temp_dir().join(format!("roundhouse-swift-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "swift", "--allow-unsupported", "-o"])
        .arg(&out)
        .arg(roundhouse::fixtures::store())
        .output()
        .expect("run roundhouse");
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let product = std::fs::read_to_string(out.join("Sources/App/app/models/Product.swift")).expect("Product.swift");
    std::fs::remove_dir_all(&out).ok();
    assert!(product.contains("let cached = featuredImageCache"), "{product}");
}
