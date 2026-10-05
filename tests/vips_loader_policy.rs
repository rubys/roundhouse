//! libvips loader policy from `config/initializers` (#426 follow-up).
//!
//! An initializer's `Vips.block_untrusted(true)` / `Vips.block("<op>",
//! true)` is lifted onto `Rails::Application` and applied when the
//! image processor loads. `vips_foreign_find_load` on libvips 8.14
//! still names a blocked loader; the processor wraps it so a policy
//! probe matches what load would refuse — Magick for BMP/PSD/ICO, SVG
//! for SVG, and any class `Vips.block` named.
//!
//! The wrap is a general Rails-app contract (any app that stores user
//! uploads and sets that policy), not a Campfire special case.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;
use roundhouse::App;

fn app_with(initializer: &str) -> App {
    let tree = vec![
        (
            std::path::PathBuf::from("config/application.rb"),
            b"module Demo\n  class Application < Rails::Application\n  end\nend\n".to_vec(),
        ),
        (
            std::path::PathBuf::from("config/initializers/vips.rb"),
            initializer.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/models/thing.rb"),
            b"class Thing < ApplicationRecord\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn lifted_readers(app: &App) -> Vec<String> {
    match &app.rails_application {
        Some(lc) => lc
            .methods
            .iter()
            .map(|m| m.name.as_str().to_string())
            .collect(),
        None => Vec::new(),
    }
}

fn method_body(app: &App, name: &str) -> String {
    let lc = app
        .rails_application
        .as_ref()
        .expect("Rails::Application reopen");
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == name)
        .unwrap_or_else(|| panic!("expected {name}, got {:?}", lifted_readers(app)));
    format!("{:?}", m.body)
}

#[test]
fn block_untrusted_true_lifts() {
    let app = app_with("Vips.block_untrusted(true)\n");
    assert!(
        lifted_readers(&app).contains(&"vips_block_untrusted".to_string()),
        "expected vips_block_untrusted: {:?}",
        lifted_readers(&app)
    );
    let body = method_body(&app, "vips_block_untrusted");
    assert!(body.contains("true"), "got: {body}");
}

#[test]
fn bare_block_untrusted_true_lifts_without_parens() {
    let app = app_with("Vips.block_untrusted true\n");
    assert!(
        lifted_readers(&app).contains(&"vips_block_untrusted".to_string()),
        "{:?}",
        lifted_readers(&app)
    );
}

#[test]
fn block_untrusted_false_does_not_lift() {
    let app = app_with("Vips.block_untrusted(false)\n");
    assert!(
        !lifted_readers(&app).contains(&"vips_block_untrusted".to_string()),
        "false is libvips' default, not an override: {:?}",
        lifted_readers(&app)
    );
}

#[test]
fn commented_policy_does_not_lift() {
    let app =
        app_with("# Vips.block_untrusted(true)\n# Vips.block(\"VipsForeignLoadPdf\", true)\n");
    let readers = lifted_readers(&app);
    assert!(
        !readers.contains(&"vips_block_untrusted".to_string()),
        "{readers:?}"
    );
    assert!(
        !readers.contains(&"vips_blocked_operations".to_string()),
        "{readers:?}"
    );
}

#[test]
fn named_block_lifts_the_operation() {
    let app = app_with("Vips.block(\"VipsForeignLoadPdf\", true)\n");
    assert!(
        lifted_readers(&app).contains(&"vips_blocked_operations".to_string()),
        "{:?}",
        lifted_readers(&app)
    );
    let body = method_body(&app, "vips_blocked_operations");
    assert!(body.contains("VipsForeignLoadPdf"), "got: {body}");
}

#[test]
fn single_quoted_block_and_a_false_arm() {
    let app = app_with(
        "Vips.block('VipsForeignLoadOpenslide', true)\nVips.block(\"VipsForeignLoadMagick\", false)\n",
    );
    let body = method_body(&app, "vips_blocked_operations");
    assert!(body.contains("VipsForeignLoadOpenslide"), "got: {body}");
    assert!(
        !body.contains("VipsForeignLoadMagick"),
        "false is not policy: {body}"
    );
}

#[test]
fn untrusted_and_named_blocks_lift_together() {
    let app =
        app_with("Vips.block_untrusted(true)\nVips.block(\"VipsForeignLoadOpenslide\", true)\n");
    let readers = lifted_readers(&app);
    assert!(
        readers.contains(&"vips_block_untrusted".to_string()),
        "{readers:?}"
    );
    assert!(
        readers.contains(&"vips_blocked_operations".to_string()),
        "{readers:?}"
    );
}

/// A variant app emits the ruby-vips processor, which wraps find_load
/// through VipsExt so a blocked loader is not selected.
#[test]
fn variant_app_emits_a_find_load_wrap_that_honors_the_policy() {
    let overlay = emit_and_run::empty_app()
        .write(
            "Gemfile",
            "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"ruby-vips\"\n",
        )
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Archive\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )
        .write(
            "config/initializers/vips.rb",
            "Vips.block_untrusted(true)\nVips.block(\"VipsForeignLoadOpenslide\", true)\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/user.rb",
            "class User < ApplicationRecord\n  has_one_attached :avatar do |attachable|\n    attachable.variant :thumb, resize_to_limit: [128, 128]\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        );
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    assert!(
        errors.is_empty(),
        "analysis or emit reports errors:\n{}",
        errors.join("\n")
    );
    let application = std::fs::read_to_string(emitted.join("config/application.rb"))
        .expect("emitted config/application.rb");
    assert!(
        application.contains("def vips_block_untrusted") && application.contains("true"),
        "initializer must lift onto the Application reopen:\n{application}"
    );
    assert!(
        application.contains("VipsForeignLoadOpenslide"),
        "named block must lift:\n{application}"
    );
    let processor = std::fs::read_to_string(emitted.join("runtime/active_storage_processor.rb"))
        .expect("emitted processor");
    assert!(
        processor.contains("require \"vips\""),
        "variant app must swap in the ruby-vips processor:\n{processor}"
    );
    assert!(
        processor.contains("VipsExt.sp_vips_find_load"),
        "find_load wrap must reach C through VipsExt, not alias_method:\n{processor}"
    );
    assert!(
        processor.contains("__rh_vips_loader_hidden?"),
        "blocked loaders must be filtered:\n{processor}"
    );
    assert!(
        processor.contains("VipsForeignLoadMagick") && processor.contains("VipsForeignLoadSvg"),
        "untrusted prefixes must include Magick and Svg:\n{processor}"
    );
    assert!(
        !processor.contains("alias_method :"),
        "wrapping the Ruby finder in place re-enters the wrapper on the spinel package:\n{processor}"
    );
}

/// Emitted Ruby: Magick/Svg are not selected after `block_untrusted`, a
/// PNG still is, and a named `Vips.block` hides that class too.
///
/// Needs the ruby-vips gem and a loadable libvips, the same
/// prerequisite as `emit_and_run` needing sqlite3. The unit job
/// installs both; a harness that skipped here would pass CI while a
/// machine with the gem hid the gap.
#[test]
fn find_load_hides_blocked_loaders_on_emitted_ruby() {
    let overlay = emit_and_run::empty_app()
        .write(
            "Gemfile",
            "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"ruby-vips\"\n",
        )
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Archive\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )
        .write(
            "config/initializers/vips.rb",
            "Vips.block_untrusted(true)\nVips.block(\"VipsForeignLoadOpenslide\", true)\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/user.rb",
            "class User < ApplicationRecord\n  has_one_attached :avatar do |attachable|\n    attachable.variant :thumb, resize_to_limit: [128, 128]\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        );
    overlay
        .run_ruby(
            r#"
require "tempfile"
raise "policy off" unless Rails.application.vips_block_untrusted
raise "openslide not blocked" unless Rails.application.vips_blocked_operations.include?("VipsForeignLoadOpenslide")

def probe(bytes)
  Tempfile.create(%w[loader_probe .img], binmode: true) do |file|
    file.write bytes
    file.flush
    Vips.vips_foreign_find_load(file.path)
  end
end

png = Vips::Image.black(8, 8).add(128).cast("uchar").write_to_buffer(".png")
got = probe(png)
raise "png loader #{got.inspect}" unless got == "VipsForeignLoadPngFile"

bmp = "BM" + [0, 0, 54].pack("V3") + "\x00" * 40
raise "bmp #{probe(bmp).inspect}" unless probe(bmp).nil?

svg = %q(<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"/>)
raise "svg #{probe(svg).inspect}" unless probe(svg).nil?

psd = "8BPS" + [1].pack("n") + "\x00" * 26
raise "psd #{probe(psd).inspect}" unless probe(psd).nil?

puts "vips find_load policy contract passed"
"#,
        )
        .assert_passes();
}
