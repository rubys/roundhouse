//! Straight-to-ISeq delivery for `--target ruby` (ROUNDSNAP=1).
//!
//! Proves the emitted tree contains units → manifest/iseq, vendors the
//! roundsnap gem, omits app/runtime `.rb`, and boots on MRI without those files.

use std::path::Path;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::app::ingest_app;
use roundhouse::project::{self, BuildTarget};

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("rh-roundsnap-{name}-{}", std::process::id()));
    if p.exists() {
        std::fs::remove_dir_all(&p).ok();
    }
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn enable_roundsnap() {
    // SAFETY: single-threaded test process before any parallel workers.
    unsafe {
        std::env::set_var("ROUNDSNAP", "1");
        std::env::remove_var("ROUNDHOUSE_RUBY_ISEQ");
        std::env::remove_var("ROUNDSNAP_KEEP_SOURCE");
        std::env::remove_var("ROUNDHOUSE_ISEQ_KEEP_SOURCE");
    }
}

#[test]
fn tiny_blog_roundsnap_artifact_shape() {
    enable_roundsnap();

    let fixture = Path::new("fixtures/tiny-blog");
    assert!(fixture.is_dir(), "fixtures/tiny-blog missing");

    let mut app = ingest_app(fixture).expect("ingest tiny-blog");
    Analyzer::new(&app).analyze(&mut app);
    let files = project::target_files(&app, fixture, BuildTarget::Ruby).expect("target_files");
    assert!(
        files.iter().any(|(p, _)| p == "units.json"),
        "expected units.json in Roundsnap file set"
    );
    assert!(
        files
            .iter()
            .any(|(p, _)| p == "vendor/roundsnap/lib/roundsnap.rb"),
        "expected vendored roundsnap gem"
    );
    assert!(
        !files.iter().any(|(p, _)| p.starts_with("app/") && p.ends_with(".rb")),
        "app/*.rb should be omitted from the text file set"
    );
    let boot = files
        .iter()
        .find(|(p, _)| p == "boot.rb")
        .expect("boot.rb present");
    assert!(
        boot.1.contains("Roundsnap::Loader"),
        "thin boot should use the roundsnap loader"
    );

    let scratch = scratch_dir("tiny-shape");
    project::write_to_dir(&files, &scratch).expect("write_to_dir");
    project::finalize_roundsnap(&scratch).expect("finalize_roundsnap");

    assert!(scratch.join("manifest.json").is_file());
    assert!(scratch.join("iseq").is_dir());
    assert!(!scratch.join("app/models.rb").is_file());
    assert!(!scratch.join("units.json").is_file());
    assert!(
        !scratch.join("iseq/boot.iseq").is_file(),
        "classic boot must not be compiled as a unit beside thin boot.rb"
    );

    let manifest = std::fs::read_to_string(scratch.join("manifest.json")).unwrap();
    assert!(
        !manifest.contains("\"boot\":"),
        "manifest must not include dual-boot unit `boot`: {}",
        &manifest[..manifest.len().min(400)]
    );
    assert!(
        manifest.contains("\"file\": \"tiny-blog/") || manifest.contains("app/models"),
        "manifest should carry original or emit file paths: {}",
        &manifest[..manifest.len().min(400)]
    );
}

#[test]
#[ignore = "needs real-blog fixture + bundle; run with --ignored"]
fn real_blog_roundsnap_boots_without_app_rb() {
    enable_roundsnap();

    let fixture = roundhouse::fixtures::real_blog();
    let mut app = ingest_app(fixture).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);
    let files = project::target_files(&app, fixture, BuildTarget::Ruby).expect("target_files");
    let scratch = scratch_dir("real-blog");
    project::write_to_dir(&files, &scratch).expect("write");
    project::finalize_roundsnap(&scratch).expect("finalize_roundsnap");

    assert!(scratch.join("manifest.json").is_file());
    assert!(!scratch.join("app/models/article.rb").is_file());

    let bundle = Command::new("bundle")
        .arg("install")
        .current_dir(&scratch)
        .env("BUNDLE_PATH", scratch.join(".bundle"))
        .output()
        .expect("bundle install");
    assert!(
        bundle.status.success(),
        "bundle install failed\n{}\n{}",
        String::from_utf8_lossy(&bundle.stdout),
        String::from_utf8_lossy(&bundle.stderr)
    );

    let boot = Command::new("bundle")
        .arg("exec")
        .arg("ruby")
        .arg("-e")
        .arg("require_relative \"boot\"; puts \"ROUNDSNAP_BOOT_OK\"")
        .current_dir(&scratch)
        .env("BUNDLE_GEMFILE", scratch.join("Gemfile"))
        .env("BUNDLE_PATH", scratch.join(".bundle"))
        .output()
        .expect("boot");
    let stdout = String::from_utf8_lossy(&boot.stdout);
    let stderr = String::from_utf8_lossy(&boot.stderr);
    assert!(
        boot.status.success() && stdout.contains("ROUNDSNAP_BOOT_OK"),
        "Roundsnap boot failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
