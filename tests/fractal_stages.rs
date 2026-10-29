//! The pass explorer's view of a build (`fractal::stages`), over an app
//! held in memory the way the browser holds it: stage 0 is the source,
//! each later stage the file after one more pass, with what that pass
//! rewrote and declined. The passes here are test-only renames, which pin
//! the view and `Context::in_memory`; one test runs the registered passes
//! as the explorer does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::fractal::{self, Context, Edit, Pass, Proposal, Scope, Unit, stages::Stages};
use roundhouse::span::SourceFile;

const ARTICLE_RB: &str = "app/models/article.rb";

const ARTICLE: &str = r#"class Article < ApplicationRecord
  def state = :todo

  def next_state
    :later
  end
end
"#;

const BASE: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

fn lock(version: &str) -> String {
    format!("GEM\n  remote: https://rubygems.org/\n  specs:\n    activerecord ({version})\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  activerecord\n\nBUNDLED WITH\n   2.6.9\n")
}

/// Rewrites the symbol `:<from>` to `:<to>_<activerecord version>`, so a
/// stage shows which lockfile the pass read. Declines when the lockfile
/// pins no activerecord, or the app defines a method named `<from>`.
struct Rename {
    name: &'static str,
    from: &'static str,
    to: &'static str,
}

impl Pass for Rename {
    fn name(&self) -> &'static str {
        self.name
    }

    fn summary(&self) -> &'static str {
        "Renames a symbol (test only)."
    }

    fn scope(&self) -> Scope {
        Scope::Files(Vec::new())
    }

    fn run(&self, context: &Context<'_>, unit: &Unit<'_>) -> Proposal {
        let mut proposal = Proposal::default();
        let symbol = format!(":{}", self.from);
        for (start, _) in unit.text.match_indices(&symbol) {
            let end = start + symbol.len();
            if unit.text[end..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let span = unit.span(start, end);
            if let Some(sites) = context.defined_methods().get(self.from) {
                proposal.declined.push((span, format!("the app defines `{}` itself ({})", self.from, sites.join(", "))));
            } else if let Some(version) = context.gem_version("activerecord") {
                proposal.edits.push(Edit { span, text: format!(":{}_{}", self.to, version.replace('.', "_")) });
            } else {
                proposal.declined.push((span, "Gemfile.lock pins no activerecord".to_string()));
            }
        }
        proposal
    }
}

const TODO: Rename = Rename { name: "rename-todo", from: "todo", to: "done" };
const LATER: Rename = Rename { name: "rename-later", from: "later", to: "soon" };

/// Run `passes` over `files` of the in-memory app `src`, as the wasm's
/// `fractal_stages` export does with the registered passes.
fn stages(src: &[(&str, &str)], files: &[&str], untrusted: &[&str], passes: &[&dyn Pass]) -> Stages {
    let sources: Vec<SourceFile> = src.iter().map(|(p, t)| SourceFile { path: p.to_string(), text: t.to_string() }).collect();
    let lockfile = src.iter().find(|(p, _)| *p == "Gemfile.lock").map(|(_, t)| t.to_string());
    let tree: HashMap<PathBuf, Vec<u8>> = src.iter().map(|(p, t)| (PathBuf::from(p), t.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let context = fractal::Context::in_memory(&app, Path::new(""), sources, lockfile.as_deref());
    let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
    let untrusted: Vec<String> = untrusted.iter().map(|f| f.to_string()).collect();
    fractal::stages::stages(&app, &context, &files, &untrusted, passes).expect("stages")
}

fn content<'a>(s: &'a Stages, stage: usize, path: &str) -> &'a str {
    &s.stages[stage].files.iter().find(|f| f.path == path).expect("file in stage").content
}

#[test]
fn the_registered_passes_read_the_lockfile_and_sources_from_memory() {
    let registered = fractal::pass::registered();
    let passes: Vec<&dyn Pass> = registered.iter().map(|pass| pass.as_ref()).collect();
    assert_eq!(passes.iter().map(|pass| pass.name()).collect::<Vec<_>>(), ["inline-delegate", "lower-duration"]);
    let model = "class Article < ApplicationRecord\n  delegate :name, to: :author\n\n  def author = nil\nend\n";
    let lock = "GEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.2.0.alpha)\n\nBUNDLED WITH\n   2.6.9\n";
    let s = stages(&[(ARTICLE_RB, model), ("app/models/application_record.rb", BASE), ("Gemfile.lock", lock)], &[ARTICLE_RB], &[], &passes);
    assert_eq!(s.stages.len(), 3);
    assert_eq!(s.stages[1].pass, "inline-delegate");
    assert_eq!(s.stages[1].rewrites.len(), 1, "{:?}", s.stages[1].declined);
    assert!(content(&s, 1, ARTICLE_RB).contains("  def name(...);  _ = author;  _.name(...);"), "{}", content(&s, 1, ARTICLE_RB));

    // An app file that defines `delegate` where a class body could call it
    // blocks the pass, read from the in-memory sources. A pass keeps what it
    // learned about the app for one build, so the build gets fresh passes.
    let registered = fractal::pass::registered();
    let passes: Vec<&dyn Pass> = registered.iter().map(|pass| pass.as_ref()).collect();
    let patch = "class Module\n  def delegate(*) = nil\nend\n";
    let s = stages(
        &[(ARTICLE_RB, model), ("app/models/application_record.rb", BASE), ("lib/patch.rb", patch), ("Gemfile.lock", lock)],
        &[ARTICLE_RB],
        &[],
        &passes,
    );
    assert!(s.stages[1].rewrites.is_empty());
    assert!(s.stages[1].declined[0].reason.contains("lib/patch.rb:2"), "{:?}", s.stages[1].declined);
}

/// The explorer's Event example (`wasm/fractal/fractal.js`): keep
/// the two in step.
const EVENT: &str = r#"# frozen_string_literal: true

class Event < ApplicationRecord
  LISTED_UNTIL = 14.days.from_now

  scope :past, -> { where(ends_at: ...7.days.ago) }

  def starting_soon?
    starts_at < 3.days.from_now
  end

  def recently_edited?
    edited_at.present? && edited_at > 2.hours.ago
  end

  def registration_closes = 1.fortnight.after

  def announced_at = notice_days.days.ago

  def reminder_at = 30.minutes.ago(starts_at)
end
"#;

#[test]
fn the_explorer_event_example_lowers_its_literal_duration_chains() {
    let registered = fractal::pass::registered();
    let passes: Vec<&dyn Pass> = registered.iter().map(|pass| pass.as_ref()).collect();
    let lock = "GEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.2.0.alpha)\n\nBUNDLED WITH\n   2.6.9\n";
    let path = "app/models/event.rb";
    let s = stages(&[(path, EVENT), ("app/models/application_record.rb", BASE), ("Gemfile.lock", lock)], &[path], &[], &passes);
    assert!(s.skipped.is_empty(), "{:?}", s.skipped);
    assert_eq!(s.stages[2].pass, "lower-duration");
    let rewrites: Vec<(&str, &str)> = s.stages[2].rewrites.iter().map(|rewrite| (rewrite.from.as_str(), rewrite.to.as_str())).collect();
    assert_eq!(
        rewrites,
        [
            ("7.days.ago", "(::Time.zone ? ::Time.zone.now.plus_without_duration(-604800) : ::Time.now.advance(days: -7))"),
            ("3.days.from_now", "(::Time.zone ? ::Time.zone.now.plus_without_duration(259200) : ::Time.now.advance(days: 3))"),
            ("2.hours.ago", "(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -7200, ::Time.zone) : ::Time.now + -7200)"),
            ("1.fortnight.after", "(::Time.zone ? ::Time.zone.now.plus_without_duration(1209600) : ::Time.now.advance(weeks: 2))"),
        ]
    );
    let declined: Vec<&str> = s.stages[2].declined.iter().map(|decline| decline.reason.as_str()).collect();
    assert_eq!(declined.len(), 3, "{declined:?}");
    assert!(declined[0].contains("runs once, as the file loads"), "{}", declined[0]);
    assert!(declined[1].contains("something other than an integer literal"), "{}", declined[1]);
    assert!(declined[2].contains("passes a time to `ago`"), "{}", declined[2]);
    assert_eq!(content(&s, 2, path).lines().count(), EVENT.lines().count());
}

#[test]
fn with_no_passes_the_only_stage_is_the_source() {
    let lock = lock("8.1.4");
    let s = stages(&[(ARTICLE_RB, ARTICLE), ("app/models/application_record.rb", BASE), ("Gemfile.lock", &lock)], &[ARTICLE_RB], &[], &[]);
    assert_eq!(s.stages.len(), 1);
    assert_eq!(s.stages[0].pass, "source");
    assert_eq!(content(&s, 0, ARTICLE_RB), ARTICLE);
    assert!(s.passes.is_empty() && s.skipped.is_empty());
}

#[test]
fn stage_zero_is_the_source_and_each_pass_adds_its_rewrites() {
    let lock = lock("8.1.4");
    let s = stages(&[(ARTICLE_RB, ARTICLE), ("app/models/application_record.rb", BASE), ("Gemfile.lock", &lock)], &[ARTICLE_RB], &[], &[&TODO, &LATER]);
    assert_eq!(s.stages.iter().map(|st| st.pass.as_str()).collect::<Vec<_>>(), ["source", "rename-todo", "rename-later"]);
    assert_eq!(content(&s, 0, ARTICLE_RB), ARTICLE);

    // Stage n is passes 1..=n applied together; each lists only its own rewrites.
    let first = content(&s, 1, ARTICLE_RB);
    assert!(first.contains("def state = :done_8_1_4") && first.contains("    :later\n"), "{first}");
    let second = content(&s, 2, ARTICLE_RB);
    assert!(second.contains("def state = :done_8_1_4") && second.contains("    :soon_8_1_4\n"), "{second}");
    assert_eq!(second.lines().count(), ARTICLE.lines().count());
    assert_eq!(s.stages[1].rewrites.iter().map(|r| r.from.as_str()).collect::<Vec<_>>(), [":todo"]);
    assert_eq!(s.stages[2].rewrites.iter().map(|r| r.from.as_str()).collect::<Vec<_>>(), [":later"]);
    assert!(s.stages[1].rewrites.iter().all(|r| r.pass == "rename-todo"));
    assert!(s.stages[2].rewrites.iter().all(|r| r.pass == "rename-later"));
    assert!(s.skipped.is_empty());
}

#[test]
fn the_lockfile_comes_from_memory() {
    let lock = lock("7.0.8");
    let s = stages(&[(ARTICLE_RB, ARTICLE), ("app/models/application_record.rb", BASE), ("Gemfile.lock", &lock)], &[ARTICLE_RB], &[], &[&TODO]);
    assert!(content(&s, 1, ARTICLE_RB).contains("def state = :done_7_0_8"));

    let s = stages(&[(ARTICLE_RB, ARTICLE), ("app/models/application_record.rb", BASE)], &[ARTICLE_RB], &[], &[&TODO]);
    let last = s.stages.last().unwrap();
    assert!(last.rewrites.is_empty());
    assert_eq!(content(&s, 1, ARTICLE_RB), ARTICLE);
    assert!(last.declined.iter().any(|d| d.reason.contains("pins no activerecord")), "{:?}", last.declined);
}

#[test]
fn app_definitions_count_only_in_program_sources() {
    let lock = lock("8.1.4");
    let helper = "module ArticleHelpers\n  def todo = true\nend\n";
    for (path, shadows) in [
        ("app/models/article_helpers.rb", true),
        ("app/models/public/article_helpers.rb", true),
        ("app/models/test/article_helpers.rb", true),
        ("test/support/article_helpers.rb", false),
        ("node_modules/article_helpers.rb", false),
        ("app/node_modules/article_helpers.rb", false),
        ("app/.cache/article_helpers.rb", false),
    ] {
        let s = stages(
            &[(ARTICLE_RB, ARTICLE), ("app/models/application_record.rb", BASE), (path, helper), ("Gemfile.lock", &lock)],
            &[ARTICLE_RB],
            &[],
            &[&TODO],
        );
        let last = s.stages.last().unwrap();
        assert_eq!(last.rewrites.iter().any(|r| r.from == ":todo"), !shadows, "{path}: {:?}", last.declined);
        if shadows {
            assert!(last.declined.iter().any(|d| d.reason.contains("defines `todo` itself")), "{path}: {:?}", last.declined);
        }
    }
}

#[test]
fn an_untrusted_file_is_skipped_and_stays_the_source() {
    let lock = lock("8.1.4");
    let s = stages(&[(ARTICLE_RB, ARTICLE), ("app/models/application_record.rb", BASE), ("Gemfile.lock", &lock)], &[ARTICLE_RB], &[ARTICLE_RB], &[&TODO]);
    assert_eq!(s.skipped.len(), 1);
    assert_eq!(s.skipped[0].file, ARTICLE_RB);
    for (i, stage) in s.stages.iter().enumerate() {
        assert_eq!(content(&s, i, ARTICLE_RB), ARTICLE);
        assert!(stage.rewrites.is_empty() && stage.declined.is_empty());
    }
}

#[test]
fn a_file_ingest_did_not_read_is_an_error() {
    let lock = lock("8.1.4");
    let sources: Vec<SourceFile> = vec![SourceFile { path: ARTICLE_RB.into(), text: ARTICLE.into() }];
    let tree: HashMap<PathBuf, Vec<u8>> = [(PathBuf::from(ARTICLE_RB), ARTICLE.as_bytes().to_vec())].into_iter().collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let context = fractal::Context::in_memory(&app, Path::new(""), sources, Some(&lock));
    let err = fractal::stages::stages(&app, &context, &["app/models/missing.rb".to_string()], &[], &[&TODO]).err().expect("error");
    assert!(err.contains("app/models/missing.rb"), "{err}");
}
