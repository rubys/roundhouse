//! A controller method's `*rest`, anonymous `**` and `...` formals.
//! `Action` used to have no slot for any of them: `*rest` and the
//! positionals after it were dropped (the emitted `def pick` then raised
//! `ArgumentError` at every call that passed arguments), and `**` / `...`
//! were refused at ingest. (Kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict; same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const HELPERS: &str = "class ArticlesController < ApplicationController
  def formals_probe
    render plain: [
      pick, pick(:a), pick(:a, :b, :c), first_of(:x, :y, :z),
      tagged(\"t\"), tagged(\"t\", 1, 2),
      labelled(name: \"n\"), labelled(name: \"n\", size: 3),
      forwarded(name: \"f\", size: 4)
    ].join(\"|\")
  end

  private

  def pick(*keys)
    keys.map(&:to_s).join(\"-\")
  end

  def first_of(head, *rest)
    \"#{head}+#{rest.length}\"
  end

  def tagged(prefix, *)
    prefix
  end

  def labelled(**)
    Label.new(**).to_s
  end

  def forwarded(...)
    Label.new(...).to_s
  end
";

const LABEL: &str = "class Label
  def initialize(name:, size: 1)
    @name = name
    @size = size
  end

  def to_s
    \"#{@name}*#{@size}\"
  end
end
";

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/label.rb", LABEL)
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            HELPERS,
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/formals_probe\", to: \"articles#formals_probe\"\n",
        )
}

#[test]
fn controller_rest_and_anonymous_formals_run() {
    overlay()
        .run_ruby(
            r#"
require_relative "app/controllers/articles_controller"
controller = ArticlesController.new
controller.process_action(:formals_probe)
want = "|a|a-b-c|x+2|t|t|n*1|n*3|f*4"
raise "formals: #{controller.body.inspect}" unless controller.body == want
puts "controller formals ok"
"#,
        )
        .assert_passes();
}

/// `**` into a destination whose keywords ingest flattened to positionals
/// (`def initialize(size: 1)` becomes `size = 1`) binds the whole packet
/// to `size`: the emitted Ruby answered `s{:size=>5}` for `s5` while
/// `check` was clean. Retaining the destination keyword declaration now
/// preserves the controller's anonymous packet without a Hash misbinding.
#[test]
fn controller_keyword_forwarding_into_optional_keywords_runs() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/flat.rb",
            "class Flat\n  attr_reader :size\n  def initialize(size: 1)\n    @size = size\n  end\nend\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  private\n\n  def flat(**)\n    Flat.new(**)\n  end\n\n  public\n\n",
        )
        .run_ruby("require_relative 'app/controllers/articles_controller'; controller = ArticlesController.new; raise 'keyword bound a Hash' unless controller.send(:flat, size: 5).size == 5; raise 'default changed' unless controller.send(:flat).size == 1");
    run.assert_passes(
    );
}

/// The ruby family carries the formals; every other target says it
/// cannot, as it does for the same formals on a model method. Spinel
/// carries `*rest` and `**` but not `...`.
#[test]
fn controller_formals_are_reported_where_no_target_carries_them() {
    use roundhouse::project::BuildTarget;
    let mut app = roundhouse::ingest::ingest_app_from_tree(
        [
            ("app/models/label.rb", LABEL),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
            ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
            ("app/controllers/articles_controller.rb", &format!("{HELPERS}end\n")),
            ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/formals_probe\", to: \"articles#formals_probe\"\nend\n"),
            ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ]
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect(),
    )
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let reported = |target: BuildTarget| -> Vec<String> {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            let _ = roundhouse::project::target_files(&app, std::path::Path::new("."), target);
        });
        diags
            .iter()
            .map(roundhouse::diagnostic::Diagnostic::to_string)
            .filter(|d| {
                d.contains("rest parameter")
                    || d.contains("keyword rest declaration")
                    || d.contains("full argument forwarding")
            })
            .collect()
    };
    for target in [BuildTarget::Typescript, BuildTarget::Rust, BuildTarget::Python, BuildTarget::Go] {
        let got = reported(target);
        for needle in ["`*keys` on `pick`", "`*rest` on `first_of`", "`*` on `tagged`", "keyword rest declaration", "full argument forwarding"] {
            assert!(got.iter().any(|d| d.contains(needle)), "{target:?} must report {needle}: {got:?}");
        }
    }
    let spinel = reported(BuildTarget::Spinel);
    assert!(spinel.iter().any(|d| d.contains("full argument forwarding")), "{spinel:?}");
    assert!(
        !spinel.iter().any(|d| d.contains("rest parameter") || d.contains("keyword rest declaration")),
        "{spinel:?}"
    );
    assert!(reported(BuildTarget::Ruby).is_empty());
}
