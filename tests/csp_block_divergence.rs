//! rubys/roundhouse#58 (CSP option 2, rubys): until `content_security_policy`
//! is modelled, a block whose directives this ingest can read — the block
//! param, called with a known CSP directive name and symbol/string/simple-
//! literal arguments, nothing else in the block — becomes a recorded
//! DIVERGENCE (`content_security_policy: not emitted, the lowered app sends
//! no CSP header`) rather than the blocking "controller class-body block not
//! recognized" gap. The lowered app still sends no CSP header either way
//! (the directive builders aren't modelled); this only stops a predictable
//! block from keeping every route on an including controller stuck on
//! Rails. Anything stranger (a dynamic directive, conditionals, other code
//! in the block) keeps today's gap, in a controller's own class body or a
//! concern's `included do`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-csp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const DIVERGENCE: &str = "content_security_policy: not emitted, the lowered app sends no CSP header";
const GENERIC_GAP: &str = "controller class-body block not recognized: `content_security_policy`";

/// A parseable `content_security_policy` block declared directly in a
/// controller's own class body: the survey shows the named divergence, not
/// the generic gap, and the route clears.
#[test]
fn a_parseable_csp_block_in_a_controller_records_a_divergence_not_a_gap() {
    let src = scratch("ctrl-src");
    let out = scratch("ctrl-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  content_security_policy do |policy|
    policy.default_src :self, :https
    policy.script_src :self
    policy.base_uri false
  end

  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains(DIVERGENCE), "{stderr}");
    assert!(!stderr.contains(GENERIC_GAP), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The same parseable block, declared inside a concern's `included do`
/// and mixed into the controller via `include`: still the named
/// divergence, not a gap, and the route still clears. Before this fix a
/// concern's `content_security_policy` block was silently dropped with no
/// survey entry at all; this both names it and keeps it from blocking.
#[test]
fn a_parseable_csp_block_in_a_concerns_included_do_records_a_divergence_not_a_gap() {
    let src = scratch("concern-src");
    let out = scratch("concern-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/concerns/csp_setup.rb", br#"module CspSetup
  extend ActiveSupport::Concern

  included do
    content_security_policy do |policy|
      policy.default_src :none
      policy.frame_ancestors :none
      policy.script_src false
    end
  end
end
"#);
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  include CspSetup

  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains(DIVERGENCE), "{stderr}");
    assert!(!stderr.contains(GENERIC_GAP), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A stranger form — a directive called with a method-call argument
/// rather than a literal — is not the recognized shape: it keeps today's
/// generic "class-body block not recognized" gap, and the gap still blocks
/// every route on this controller (unchanged regression check).
#[test]
fn a_dynamic_csp_block_in_a_controller_still_records_the_gap_and_still_blocks() {
    let src = scratch("dynamic-src");
    let out = scratch("dynamic-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  content_security_policy do |policy|
    policy.script_src nonce_source
  end

  def index
    @name = "hi"
  end

  private

  def nonce_source
    "https://cdn.example.com"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains(GENERIC_GAP), "{stderr}");
    assert!(!stderr.contains(DIVERGENCE), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("widgets#index")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("ingest gap") && residue.contains(GENERIC_GAP), "{residue}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A `content_security_policy only: :show do |p| … end` whose block is
/// otherwise as plain as the recognized shape (a real app's controllers
/// scope a policy to one action exactly this way): the `only:` keyword
/// scopes the policy to one action, which this ingest does not model,
/// so it must not be mistaken for the bare, every-action form. It
/// keeps today's gap and still blocks.
#[test]
fn a_scoped_only_csp_block_in_a_controller_still_records_the_gap_and_still_blocks() {
    let src = scratch("only-src");
    let out = scratch("only-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  content_security_policy only: :index do |policy|
    policy.frame_ancestors(false)
  end

  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains(GENERIC_GAP), "{stderr}");
    assert!(!stderr.contains(DIVERGENCE), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The same `only:`-scoped shape via a concern's `included do`: still no
/// divergence recorded (matching the unchanged, silent-drop treatment
/// every other unrecognized concern statement gets).
#[test]
fn a_scoped_only_csp_block_in_a_concerns_included_do_records_no_divergence() {
    let src = scratch("only-concern-src");
    let out = scratch("only-concern-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/concerns/csp_setup.rb", br#"module CspSetup
  extend ActiveSupport::Concern

  included do
    content_security_policy only: :index do |policy|
      policy.frame_ancestors(false)
    end
  end
end
"#);
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  include CspSetup

  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains(DIVERGENCE), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}
