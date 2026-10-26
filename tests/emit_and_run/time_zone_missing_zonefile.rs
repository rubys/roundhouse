use super::emit_and_run;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A `TZDIR` tree that mirrors `/usr/share/zoneinfo` except for one
/// zone file, deliberately left out. Reproduces, on any host (#1062:
/// Debian 13 without the `tzdata-legacy` package lacks `Europe/Kiev`
/// and `Asia/Rangoon`), the situation where one of the 152 names
/// `ZONE_NAMES` maps to has no backing zone file, without depending on
/// which legacy names this host's own tzdata happens to omit.
///
/// `missing` is an `Area/Location` pair (one slash); every other entry
/// of the real tree, including the rest of `missing`'s own directory,
/// is reached through a symlink, so this is cheap and does not copy
/// zoneinfo's data.
fn zoneinfo_missing(missing: &str) -> PathBuf {
    let real = Path::new("/usr/share/zoneinfo");
    assert!(
        real.join(missing).is_file(),
        "fixture needs the real {missing} to exist so it can be omitted"
    );
    let (area, location) = missing.split_once('/').expect("missing is \"Area/Location\"");

    // Tests in this binary run in parallel: a per-process id alone
    // collides when two tests omit the same zone, as `scratch_dir` in
    // `support/emit_and_run.rs` notes for its own counter.
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "roundhouse-zoneinfo-missing-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        missing.replace('/', "-")
    ));
    std::fs::create_dir_all(&dir).expect("mkdir scratch zoneinfo");

    for entry in std::fs::read_dir(real).expect("read /usr/share/zoneinfo") {
        let entry = entry.expect("dirent");
        let name = entry.file_name();
        if name.to_string_lossy() == area {
            // This is `missing`'s own directory: rebuild it file by
            // file, symlinking every entry except `location`.
            let area_dir = dir.join(&name);
            std::fs::create_dir_all(&area_dir).expect("mkdir area dir");
            for sub in std::fs::read_dir(entry.path()).expect("read area dir") {
                let sub = sub.expect("dirent");
                if sub.file_name().to_string_lossy() == location {
                    continue;
                }
                std::os::unix::fs::symlink(sub.path(), area_dir.join(sub.file_name()))
                    .expect("symlink zone file");
            }
        } else {
            std::os::unix::fs::symlink(entry.path(), dir.join(&name)).expect("symlink top-level entry");
        }
    }

    assert!(!dir.join(missing).exists(), "setup bug: {missing} should be absent from the scratch tree");
    assert!(dir.join("Asia/Tokyo").is_file(), "setup bug: an unrelated zone should still resolve");
    dir
}

/// `TimeZone.all` (`runtime/ruby/active_support_ext.rb`) must skip a
/// name whose zone file is missing, as Rails 8.1's `zones_map` does
/// (`activesupport/lib/active_support/values/time_zone.rb`: `zones_map`
/// keeps only the names where `self[name]` resolves, and `[]` itself
/// rescues `TZInfo::InvalidTimezoneIdentifier` into `nil`), instead of
/// raising for the whole list (#1062).
#[test]
fn time_zone_all_skips_a_name_with_no_zone_file() {
    let tzdir = zoneinfo_missing("Pacific/Apia"); // "Samoa" in ZONE_NAMES
    let script = format!(
        r#"
ENV["TZDIR"] = "/usr/share/zoneinfo"
full_names = ActiveSupport::TimeZone.all.map(&:name)
raise "fixture needs Samoa to resolve with the full TZDIR" unless full_names.include?("Samoa")

ENV["TZDIR"] = {tzdir:?}
names = ActiveSupport::TimeZone.all.map(&:name)

raise "Samoa should be left out" if names.include?("Samoa")
only_in_full = full_names - names
raise "expected exactly Samoa to disappear, got #{{only_in_full.inspect}}" unless only_in_full == ["Samoa"]
raise "no zone should appear that wasn't already there" unless (names - full_names).empty?
raise "other zones still resolve" unless names.include?("Tokyo")

raise "lookup should return nil" unless ActiveSupport::TimeZone.lookup("Samoa").nil?
raise "lookup still works for a present zone" unless ActiveSupport::TimeZone.lookup("Tokyo").name == "Tokyo"

raise "find_zone! should still raise" unless begin
  ActiveSupport.find_zone!("Samoa")
  false
rescue ArgumentError => e
  e.message == "Invalid Timezone: Samoa"
end

puts "TimeZone.all skips a missing zone file"
"#,
    );

    let run = emit_and_run::real_blog().run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("TimeZone.all skips a missing zone file"), "{}", run.stdout);

    let _ = std::fs::remove_dir_all(&tzdir);
}

/// The other `ActiveSupport.find_zone!`-based callers an app can reach
/// (`config.time_zone=`/`set_default_zone`, `Time.use_zone`,
/// `Integer#in_time_zone`) raise for an unresolvable name, matching
/// Rails 8.1's own `Time.find_zone!` (`core_ext/time/zones.rb`): no
/// change needed there, only coverage that they still do once `all`
/// stops raising.
#[test]
fn other_zone_callers_still_raise_for_a_missing_zone_file() {
    let tzdir = zoneinfo_missing("Pacific/Apia");
    let script = format!(
        r#"
ENV["TZDIR"] = {tzdir:?}

raise "set_default_zone should raise" unless begin
  ActiveSupport.set_default_zone("Samoa")
  false
rescue ArgumentError => e
  e.message == "Invalid Timezone: Samoa"
end

raise "use_zone should raise" unless begin
  ActiveSupport.use_zone("Samoa") {{ }}
  false
rescue ArgumentError => e
  e.message == "Invalid Timezone: Samoa"
end

raise "in_time_zone should raise" unless begin
  ActiveSupport.in_time_zone(Time.at(0), "Samoa")
  false
rescue ArgumentError => e
  e.message == "Invalid Timezone: Samoa"
end

puts "other zone callers still raise"
"#,
    );

    let run = emit_and_run::real_blog().run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("other zone callers still raise"), "{}", run.stdout);

    let _ = std::fs::remove_dir_all(&tzdir);
}
