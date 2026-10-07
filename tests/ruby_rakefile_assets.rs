//! The CRuby target's `rake assets` stages the app's own assets the way
//! Propshaft serves them: each directory under app/assets and
//! app/javascript at /assets/<path within it>. campfire's layout links
//! its stylesheets that way, and before this every one was a 404.

use std::fs;
use std::path::Path;
use std::process::Command;

const RAKEFILE: &str = include_str!("../runtime/spinel/scaffold/ruby_overlay/Rakefile");

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn app_assets_are_staged_at_their_propshaft_paths() {
    let dir = std::env::temp_dir().join(format!("roundhouse-rake-assets-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let root = dir.as_path();
    fs::write(root.join("Rakefile"), RAKEFILE).unwrap();
    write(root, "app/assets/stylesheets/_reset.css", "html{}");
    write(root, "app/assets/images/icons/bell.svg", "<svg/>");
    write(root, "app/assets/sounds/56k.mp3", "mp3");
    write(root, "app/javascript/helpers/dom_helpers.js", "export {}");
    // The scaffold's own task for this path wins over the app's file.
    write(root, "app/assets/stylesheets/application.css", "body{}");

    // No app/assets/tailwind.css and no app/javascript/application.js, so
    // `assets` needs neither npm nor the turbo/stimulus gems.
    let out = Command::new("rake")
        .arg("assets")
        .current_dir(root)
        .output()
        .expect("rake on PATH");
    assert!(
        out.status.success(),
        "rake assets failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let staged = |rel: &str| fs::read_to_string(root.join("static/assets").join(rel)).ok();
    assert_eq!(staged("_reset.css").as_deref(), Some("html{}"));
    assert_eq!(staged("icons/bell.svg").as_deref(), Some("<svg/>"));
    assert_eq!(staged("56k.mp3").as_deref(), Some("mp3"));
    assert_eq!(staged("helpers/dom_helpers.js").as_deref(), Some("export {}"));
    assert_eq!(staged("application.css").as_deref(), Some(""));
    let _ = fs::remove_dir_all(&dir);
}
