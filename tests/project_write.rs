//! Re-emitting identical project files must not invalidate native builds.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use roundhouse::project::{write_binary_assets, write_to_dir};

fn scratch(tag: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "roundhouse-write-{tag}-{}-{unique}",
        std::process::id()
    ))
}

fn age(path: &Path) -> SystemTime {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn text_writes_preserve_identical_files_and_replace_changed_bytes() {
    let dest = scratch("text");
    let mut files = vec![
        ("app/main.rb".to_string(), "puts 'old'\n".to_string()),
        ("spin.toml".to_string(), "[package]\n".to_string()),
        ("empty.txt".to_string(), String::new()),
    ];
    write_to_dir(&files, &dest).unwrap();
    let times: Vec<_> = files.iter().map(|(p, _)| age(&dest.join(p))).collect();
    fs::write(dest.join("unlisted.txt"), b"leave me alone").unwrap();
    let unlisted_time = age(&dest.join("unlisted.txt"));

    write_to_dir(&files, &dest).unwrap();
    for ((path, bytes), time) in files.iter().zip(&times) {
        assert_eq!(fs::read(dest.join(path)).unwrap(), bytes.as_bytes());
        assert_eq!(
            fs::metadata(dest.join(path)).unwrap().modified().unwrap(),
            *time,
            "{path}"
        );
    }

    // Equal size and timestamp are not evidence of equal contents.
    files[0].1 = "puts 'new'\n".to_string();
    files.push(("nested/new.txt".to_string(), "new file".to_string()));
    write_to_dir(&files, &dest).unwrap();
    assert_eq!(fs::read(dest.join("app/main.rb")).unwrap(), b"puts 'new'\n");
    assert_ne!(
        fs::metadata(dest.join("app/main.rb"))
            .unwrap()
            .modified()
            .unwrap(),
        times[0]
    );
    assert_eq!(fs::read(dest.join("nested/new.txt")).unwrap(), b"new file");
    assert_eq!(
        fs::metadata(dest.join("spin.toml"))
            .unwrap()
            .modified()
            .unwrap(),
        times[1]
    );
    assert_eq!(
        fs::read(dest.join("unlisted.txt")).unwrap(),
        b"leave me alone"
    );
    assert_eq!(
        fs::metadata(dest.join("unlisted.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        unlisted_time
    );
    fs::remove_dir_all(dest).unwrap();
}

#[test]
fn binary_writes_preserve_mtimes_counts_and_text_precedence() {
    let dest = scratch("binary");
    let emitted = vec![("public/logo.png".to_string(), "emitter wins".to_string())];
    let mut assets = vec![
        ("public/logo.png".to_string(), vec![0xff, 0x80]),
        ("test/files/pixel.bmp".to_string(), vec![0xff, 0x00, 0x80]),
        ("public/empty.bin".to_string(), vec![]),
    ];
    // Precedence depends on the emitted path set, not destination existence.
    assert_eq!(write_binary_assets(&assets, &emitted, &dest).unwrap(), 2);
    assert!(!dest.join("public/logo.png").exists());
    write_to_dir(&emitted, &dest).unwrap();
    let pixel_time = age(&dest.join("test/files/pixel.bmp"));
    let empty_time = age(&dest.join("public/empty.bin"));
    let text_time = age(&dest.join("public/logo.png"));

    assert_eq!(write_binary_assets(&assets, &emitted, &dest).unwrap(), 2);
    assert_eq!(
        fs::read(dest.join("test/files/pixel.bmp")).unwrap(),
        [0xff, 0x00, 0x80]
    );
    assert_eq!(
        fs::metadata(dest.join("test/files/pixel.bmp"))
            .unwrap()
            .modified()
            .unwrap(),
        pixel_time
    );
    assert_eq!(
        fs::metadata(dest.join("public/empty.bin"))
            .unwrap()
            .modified()
            .unwrap(),
        empty_time
    );

    assets[1].1 = vec![0xff, 0x01, 0x80];
    assets.push(("new/icon.bin".to_string(), vec![0xfe, 0x02]));
    assert_eq!(write_binary_assets(&assets, &emitted, &dest).unwrap(), 3);
    assert_eq!(
        fs::read(dest.join("test/files/pixel.bmp")).unwrap(),
        [0xff, 0x01, 0x80]
    );
    assert_ne!(
        fs::metadata(dest.join("test/files/pixel.bmp"))
            .unwrap()
            .modified()
            .unwrap(),
        pixel_time
    );
    assert_eq!(fs::read(dest.join("new/icon.bin")).unwrap(), [0xfe, 0x02]);
    assert_eq!(
        fs::metadata(dest.join("public/empty.bin"))
            .unwrap()
            .modified()
            .unwrap(),
        empty_time
    );
    assert_eq!(
        fs::read(dest.join("public/logo.png")).unwrap(),
        b"emitter wins"
    );
    assert_eq!(
        fs::metadata(dest.join("public/logo.png"))
            .unwrap()
            .modified()
            .unwrap(),
        text_time
    );
    fs::remove_dir_all(dest).unwrap();
}

#[test]
fn empty_text_output_still_creates_destination_and_io_errors_propagate() {
    let dest = scratch("errors");
    write_to_dir(&[], &dest).unwrap();
    assert!(dest.is_dir());
    fs::create_dir(dest.join("blocked")).unwrap();
    let text = vec![("blocked".to_string(), "content".to_string())];
    let binary = vec![("blocked".to_string(), vec![0xff])];
    for result in [
        write_to_dir(&text, &dest),
        write_binary_assets(&binary, &[], &dest).map(|_| ()),
    ] {
        assert!(
            result
                .unwrap_err()
                .starts_with(&format!("write {}:", dest.join("blocked").display()))
        );
    }
    fs::write(dest.join("parent"), b"not a directory").unwrap();
    assert!(
        write_to_dir(&[("parent/file".into(), "bytes".into())], &dest)
            .unwrap_err()
            .starts_with(&format!("mkdir {}:", dest.join("parent").display()))
    );
    fs::remove_dir_all(dest).unwrap();
}

#[cfg(unix)]
#[test]
fn unreadable_but_writable_destinations_keep_the_existing_write_behavior() {
    use std::os::unix::fs::PermissionsExt;

    let dest = scratch("permissions");
    let text = vec![("file".to_string(), "before".to_string())];
    write_to_dir(&text, &dest).unwrap();
    let path = dest.join("file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();
    // Privileged runners can read despite mode bits; either way writing works.
    write_to_dir(&[("file".into(), "after!".into())], &dest).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read(path).unwrap(), b"after!");
    fs::remove_dir_all(dest).unwrap();
}

#[test]
fn repeated_strict_cli_emission_reports_materialized_files_not_rewrites() {
    use std::process::Command;

    let dest = scratch("cli");
    let emit = || {
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", "spinel"])
            .arg(roundhouse::fixtures::real_blog())
            .arg("-o")
            .arg(&dest)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "{stderr}");
        assert!(stderr.contains("roundhouse: emitted "), "{stderr}");
        stderr
    };
    let first = emit();
    let paths = [
        "spin.toml",
        "bin/blog.rb",
        "app/controllers/articles_controller.rb",
    ];
    let times: Vec<_> = paths.iter().map(|p| age(&dest.join(p))).collect();
    assert_eq!(emit(), first);
    for (path, time) in paths.iter().zip(times) {
        assert_eq!(
            fs::metadata(dest.join(path)).unwrap().modified().unwrap(),
            time,
            "{path}"
        );
    }
    fs::remove_dir_all(dest).unwrap();
}
