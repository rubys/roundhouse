//! `lower-duration`: what it rewrites, what it declines and why, and (where
//! Ruby and Active Support are available) that every rewritten expression
//! returns what the original does, in several zones and either side of DST
//! changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::fractal::passes::LowerDuration;
use roundhouse::emit::fractal::passes::lower_duration::VERIFIED_ACTIVESUPPORT_VERSION;
use roundhouse::emit::fractal::{self, Pass, Plan};

const MODEL: &str = "app/models/event.rb";

fn lockfile(activesupport_version: &str) -> String {
    format!("GEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport ({activesupport_version})\n\nBUNDLED WITH\n   2.6.9\n")
}

fn app_directory(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let app = std::env::temp_dir().join(format!("roundhouse-fractal-lower-duration-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&app);
    for (path, content) in files {
        let path = app.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    app
}

fn app_with_model(name: &str, model: &str) -> PathBuf {
    app_directory(name, &[(MODEL, model), ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION))])
}

/// Plan `app/models/event.rb` with the pass the way the CLI does:
/// ingest what the scope decides, recording gaps (`--survey`) rather than
/// stopping. The pass must get to read the file, so a skipped file fails
/// the test.
fn build_plan(app: &Path) -> Plan {
    let pass = LowerDuration::default();
    let passes: [&dyn Pass; 1] = [&pass];
    let root = app.canonicalize().unwrap();
    let compiled = [MODEL.to_string()];
    let fractal::Ingest::Files { files, .. } = fractal::scope::decide(&root, &compiled, &passes, false).unwrap() else {
        panic!("lower-duration needs no more than the compiled files")
    };
    let filesystem = roundhouse::vfs::FsVfs::new();
    let vfs = roundhouse::vfs::ScopedVfs::new(&filesystem, files);
    roundhouse::ingest::survey::activate();
    let ingested = roundhouse::ingest::app::ingest_app_with_vfs(&vfs, &root);
    let gaps = roundhouse::ingest::survey::drain();
    let mut ingested = ingested.unwrap();
    Analyzer::new(&ingested).analyze(&mut ingested);
    let untrusted: Vec<String> = gaps
        .iter()
        .filter_map(|gap| match gap {
            roundhouse::ingest::IngestError::Unsupported { file, .. } | roundhouse::ingest::IngestError::Parse { file, .. } => Some(file.clone()),
            _ => None,
        })
        .collect();
    let plan = fractal::plan(&ingested, &root, &compiled, &untrusted, &passes).unwrap();
    assert!(plan.report.skipped.is_empty(), "{:?}", plan.report.skipped);
    plan
}

const SIMPLE: &str = r##"# frozen_string_literal: true

class Event
  START = 14.days.from_now

  def recent?(at)
    at > 5.days.ago
  end

  def window
    where(created_at: 2.hours.ago..1.minute.since)
  end

  scope :lapsed, -> { where(ends_at: ...7.days.ago) }

  def reminders(fast)
    log "starts at #{1.month.from_now}", fast ? 1.fortnight.after : 1.year.after
    remind 30.seconds.until, -3.weeks.before, 0.days.ago.to_date
  end
end
"##;

#[test]
fn a_literal_duration_chain_becomes_the_call_its_duration_would_make() {
    let app = app_with_model("simple", SIMPLE);
    let plan = build_plan(&app);
    assert_eq!(plan.report.declined.len(), 1, "{:?}", plan.report.declined);
    assert_eq!(plan.report.declined[0].at, "app/models/event.rb:4:11");
    assert!(plan.report.declined[0].reason.contains("runs once, as the file loads"), "{}", plan.report.declined[0].reason);
    let expected = SIMPLE
        .replace("5.days.ago", "(::Time.zone ? ::Time.zone.now.plus_without_duration(-432000) : ::Time.now.advance(days: -5))")
        .replace("2.hours.ago", "(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -7200, ::Time.zone) : ::Time.now + -7200)")
        .replace("7.days.ago", "(::Time.zone ? ::Time.zone.now.plus_without_duration(-604800) : ::Time.now.advance(days: -7))")
        .replace("1.minute.since", "(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + 60, ::Time.zone) : ::Time.now + 60)")
        .replace("1.month.from_now", "::Time.current.advance(months: 1)")
        .replace("1.fortnight.after", "(::Time.zone ? ::Time.zone.now.plus_without_duration(1209600) : ::Time.now.advance(weeks: 2))")
        .replace("1.year.after", "::Time.current.advance(years: 1)")
        .replace("30.seconds.until", "(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -30, ::Time.zone) : ::Time.now + -30)")
        .replace("-3.weeks.before", "(::Time.zone ? ::Time.zone.now.plus_without_duration(1814400) : ::Time.now.advance(weeks: 3))")
        .replace("0.days.ago", "(::Time.zone ? ::Time.zone.now.plus_without_duration(0) : ::Time.now.advance(days: 0))");
    assert_eq!(plan.report.rewrites.len(), 10, "{:?}", plan.report.rewrites);
    assert_eq!(plan.rewritten, [(MODEL.to_string(), expected.clone())]);
    assert_eq!(expected.lines().count(), SIMPLE.lines().count());
    assert_eq!(plan.report.rewrites[0].at, "app/models/event.rb:7:10");
    assert_eq!(plan.report.rewrites[0].pass, "lower-duration");
}

#[test]
fn every_chain_it_cannot_reproduce_exactly_is_declined_with_a_reason() {
    let cases: &[(&str, &str)] = &[
        ("count.days.ago", "something other than an integer literal"),
        ("1.5.days.ago", "something other than an integer literal"),
        ("CONSTANT.days.ago", "something other than an integer literal"),
        ("5.days&.ago", "calls the direction method with `&.`"),
        ("5&.days.ago", "calls the unit method with `&.`"),
        ("5.days.ago(at)", "passes a time to `ago`"),
        ("5.days.since(at)", "passes a time to `since`"),
        ("5.days.ago { }", "passes a block to the direction method"),
        ("5.days(&block).ago", "passes a block to the unit method"),
        ("5.days.\n      ago", "spans more than one line"),
        ("10_000_000_000.days.ago", "does not fit in 32 bits"),
    ];
    for (expression, reason) in cases {
        let model = format!("class Event\n  def at\n    {expression}\n  end\nend\n");
        let app = app_with_model("declined", &model);
        let plan = build_plan(&app);
        assert!(plan.rewritten.is_empty(), "{expression}: {:?}", plan.report.rewrites);
        assert_eq!(plan.report.declined.len(), 1, "{expression}: {:?}", plan.report.declined);
        assert_eq!(plan.report.declined[0].pass, "lower-duration");
        assert!(plan.report.declined[0].reason.contains(reason), "{expression}: {}", plan.report.declined[0].reason);
    }
}

#[test]
fn other_calls_named_like_a_unit_or_direction_are_left_alone() {
    let model = "class Event\n  def at\n    [days.ago, plan.days, 5.days, 5.days.to_i, ago, 5.ago]\n  end\nend\n";
    let app = app_with_model("unrelated", model);
    let plan = build_plan(&app);
    assert!(plan.rewritten.is_empty() && plan.report.declined.is_empty(), "{:?} {:?}", plan.report.rewrites, plan.report.declined);
}

#[test]
fn a_refinement_an_unverified_active_support_or_an_app_definition_declines() {
    let refined = app_with_model("refined", "using Shorthand\nclass Event\n  def at = 5.days.ago\nend\n");
    let plan = build_plan(&refined);
    assert!(plan.rewritten.is_empty());
    assert!(plan.report.declined[0].reason.contains("activates a refinement with `using`"), "{:?}", plan.report.declined);

    let model = "class Event\n  def at = [5.days.ago, 5.hours.ago]\nend\n";
    let without_lockfile = app_directory("no-lockfile", &[(MODEL, model)]);
    let older = app_directory("older-rails", &[(MODEL, model), ("Gemfile.lock", &lockfile("8.1.0"))]);
    for (app, reason) in [(without_lockfile, "does not resolve activesupport"), (older, "resolves activesupport 8.1.0")] {
        let plan = build_plan(&app);
        assert!(plan.rewritten.is_empty(), "{reason}");
        assert_eq!(plan.report.declined.len(), 2, "{reason}: {:?}", plan.report.declined);
        assert!(plan.report.declined.iter().all(|decline| decline.reason.contains(reason)), "{reason}: {:?}", plan.report.declined);
    }

    // An app `Numeric#days` blocks only the sites that rely on it.
    let own_days = app_directory(
        "own-days",
        &[
            (MODEL, model),
            ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION)),
            ("config/initializers/days.rb", "class Numeric\n  def days = ActiveSupport::Duration.days(self * 2)\nend\n"),
        ],
    );
    let plan = build_plan(&own_days);
    assert_eq!(plan.report.rewrites.len(), 1, "{:?}", plan.report.rewrites);
    assert_eq!(plan.report.rewrites[0].to, "(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -18000, ::Time.zone) : ::Time.now + -18000)");
    assert_eq!(plan.report.declined.len(), 1, "{:?}", plan.report.declined);
    assert!(
        plan.report.declined[0].reason.contains("the app defines `days` where it could run instead of Active Support's (config/initializers/days.rb:2)"),
        "{}",
        plan.report.declined[0].reason
    );

    // An app `TimeWithZone#plus_without_duration` blocks only the days the
    // rewrite would forward to it.
    let own_plus = app_directory(
        "own-plus",
        &[
            (MODEL, model),
            ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION)),
            ("config/initializers/time.rb", "module ActiveSupport\n  class TimeWithZone\n    def plus_without_duration(other) = super\n  end\nend\n"),
        ],
    );
    let plan = build_plan(&own_plus);
    assert_eq!(plan.report.rewrites.len(), 1, "{:?}", plan.report.rewrites);
    assert!(plan.report.rewrites[0].to.contains("::Time.now.utc + -18000"), "{}", plan.report.rewrites[0].to);
    assert_eq!(plan.report.declined.len(), 1, "{:?}", plan.report.declined);
    assert!(plan.report.declined[0].reason.contains("`plus_without_duration`") && plan.report.declined[0].reason.contains("config/initializers/time.rb:3"), "{}", plan.report.declined[0].reason);

    // An app `Time.current` blocks both: the rewrites no longer call it.
    let own_current = app_directory(
        "own-current",
        &[(MODEL, model), ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION)), ("lib/clock.rb", "class << Time\n  def current = now\nend\n")],
    );
    let plan = build_plan(&own_current);
    assert!(plan.rewritten.is_empty(), "{:?}", plan.report.rewrites);
    assert!(plan.report.declined.iter().all(|decline| decline.reason.contains("`current`") && decline.reason.contains("lib/clock.rb:2")), "{:?}", plan.report.declined);

    // An app `Duration#ago`, mixed in from a module, blocks every `ago`.
    let own_ago = app_directory(
        "own-ago",
        &[
            (MODEL, model),
            ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION)),
            ("lib/ago.rb", "module LoggedAgo\n  def ago(*) = super\nend\nActiveSupport::Duration.prepend(LoggedAgo)\n"),
        ],
    );
    let plan = build_plan(&own_ago);
    assert!(plan.rewritten.is_empty());
    assert_eq!(plan.report.declined.len(), 2, "{:?}", plan.report.declined);
    assert!(plan.report.declined.iter().all(|decline| decline.reason.contains("(lib/ago.rb:2)")), "{:?}", plan.report.declined);
}

#[test]
fn a_file_without_a_duration_chain_reads_nothing_beyond_itself() {
    let app = app_directory("no-chain", &[(MODEL, "class Event\n  def at = Time.current\nend\n")]);
    let plan = build_plan(&app);
    assert!(plan.rewritten.is_empty() && plan.report.declined.is_empty());
}

/// Load two files defining `Sites.all`, the same expressions before and after
/// the rewrite, and compare what Ruby can observe of each result in several
/// zones (and with none, so `Time.current` is the local `Time.now`), at
/// instants either side of DST changes, at the end of a month and on a leap
/// day. Exits 1 and prints the first mismatches if any differ.
const COMPARE: &str = r##"
require "active_support"
require "active_support/time"
require "active_support/testing/time_helpers"
include ActiveSupport::Testing::TimeHelpers

observe = lambda do |path|
  Object.send(:remove_const, :Sites) if defined?(Sites)
  load path
  observed = []
  [nil, "UTC", "America/Chicago", "Australia/Lord_Howe", "Asia/Kolkata", "Europe/London"].each do |zone|
    Time.zone = zone
    ["2026-03-08 07:59:59.123456789 UTC", "2026-03-08 08:00:00 UTC", "2026-03-13 08:30:00 UTC", "2026-11-01 06:30:00 UTC",
     "2026-11-05 06:30:00 UTC", "2026-01-31 12:00:00 UTC", "2024-02-29 23:59:59 UTC", "2026-10-04 15:15:00 UTC"].each do |instant|
      travel_to(Time.parse(instant), with_usec: true) do
        Sites.all.each do |site|
          value = site.call
          observed << [zone, instant, value.class, value.inspect, value.utc_offset, value.utc?, value.nsec, value.zone]
        end
      end
    end
  end
  observed
end
original = observe.(ARGV.fetch(0))
rewritten = observe.(ARGV.fetch(1))
mismatches = original.zip(rewritten).reject { |left, right| left == right }
puts "#{original.size} observations, #{mismatches.size} mismatches"
mismatches.first(5).each { |left, right| puts "  #{left.inspect}\n  #{right.inspect}" }
exit(mismatches.empty? ? 0 : 1)
"##;

#[test]
fn every_rewritten_expression_returns_what_the_original_does() {
    // Needs Ruby and Active Support on the load path; the Nix build has
    // neither.
    let available = Command::new("ruby").args(["-e", "require 'active_support'; require 'active_support/time'; require 'active_support/testing/time_helpers'"]).output();
    if !available.is_ok_and(|output| output.status.success()) {
        eprintln!("ruby with active_support/time is not available; skipping the runtime comparison");
        return;
    }
    let units = [
        "second", "seconds", "minute", "minutes", "hour", "hours", "day", "days", "week", "weeks", "fortnight", "fortnights", "month", "months", "year", "years",
    ];
    let directions = ["ago", "until", "before", "since", "from_now", "after"];
    let mut sites = Vec::new();
    for number in ["0", "1", "5", "-3", "31", "1_000"] {
        for unit in units {
            for direction in directions {
                sites.push(format!("-> {{ {number}.{unit}.{direction} }}"));
            }
        }
    }
    let model = format!("module Sites\n  def self.all = [\n    {}\n  ]\nend\n", sites.join(",\n    "));
    let app = app_with_model("runtime", &model);
    let plan = build_plan(&app);
    assert!(plan.report.declined.is_empty(), "{:?}", plan.report.declined);
    assert_eq!(plan.report.rewrites.len(), sites.len());
    let rewritten = app.join("rewritten.rb");
    fs::write(&rewritten, &plan.rewritten[0].1).unwrap();
    let script = app.join("compare.rb");
    fs::write(&script, COMPARE).unwrap();
    let output = Command::new("ruby").env("TZ", "America/New_York").arg(&script).arg(app.join(MODEL)).arg(&rewritten).output().unwrap();
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{report}{}", String::from_utf8_lossy(&output.stderr));
    assert!(report.contains(&format!("{} observations, 0 mismatches", sites.len() * 6 * 8)), "{report}");
}

#[test]
fn the_comparison_catches_a_lowering_that_counts_days_in_seconds() {
    // The comparison is only worth something if it fails for a plausible
    // wrong lowering: 5 days as 432,000 seconds differs across a DST change.
    let available = Command::new("ruby").args(["-e", "require 'active_support'; require 'active_support/time'; require 'active_support/testing/time_helpers'"]).output();
    if !available.is_ok_and(|output| output.status.success()) {
        eprintln!("ruby with active_support/time is not available; skipping the runtime comparison");
        return;
    }
    let app = app_directory("seconds", &[]);
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("original.rb"), "module Sites\n  def self.all = [-> { 5.days.ago }]\nend\n").unwrap();
    fs::write(app.join("compare.rb"), COMPARE).unwrap();
    // Seconds from the zone's UTC time, rather than from its wall clock, are
    // as wrong as seconds from `Time.current`.
    for wrong in ["::Time.current.since(-432000)", "(::Time.zone ? ::Time.zone.now + -432000 : ::Time.now.advance(days: -5))"] {
        fs::write(app.join("seconds.rb"), format!("module Sites\n  def self.all = [-> {{ {wrong} }}]\nend\n")).unwrap();
        let output = Command::new("ruby").env("TZ", "America/New_York").arg(app.join("compare.rb")).arg(app.join("original.rb")).arg(app.join("seconds.rb")).output().unwrap();
        assert!(!output.status.success(), "{wrong}: {}", String::from_utf8_lossy(&output.stdout));
    }
}
