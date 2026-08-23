//! `send_file path, content_type:` → `send_data File.binread(path),
//! type:` (`lower::send_file`), plus the response-terminal recognition
//! that goes with it.
//!
//! # Why the read is at the CALL SITE
//!
//! `runtime/ruby/` does no file I/O anywhere, and that is a property
//! rather than an accident: every file under it transpiles to EVERY
//! target, so a `File.binread` in `action_controller/base.rb` is a
//! primitive nine strict runtimes have to grow before an app that never
//! sends a file can compile. The runtime's RBS has no `File` type to
//! name either — the version that lived in base.rb tripped
//! `runtime_src_integration`'s untyped ceiling by exactly one. Lowered
//! at the call site, `File.binread` lands in the app's own emitted code,
//! where the analyzer's stdlib registry already types it.
//!
//! # And why `send_data` is a TERMINAL
//!
//! Recognizing the rewritten name is not cosmetic. campfire's account
//! logo responds through a two-hop private helper (`send_stock_icon` ->
//! `send_png_file` -> `send_file`), and the implicit-render synthesis
//! only guards its tail on `performed?` when it can SEE a terminal in
//! the body. Unguarded, `head :no_content` ran over the PNG the helper
//! had just written — a 204 with an empty body, which the test reported
//! as "buffer is not in a known format": the image library's complaint,
//! naming nothing about the response.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(controller: &str) -> String {
    emitted_with_view(controller, None)
}

fn emitted_with_view(controller: &str, view: Option<(&str, &str)>) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/post.rb"),
            b"class Post < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :files\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/files_controller.rb"),
            controller.as_bytes().to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut files = files;
    if let Some((path, body)) = view {
        files.insert(PathBuf::from(path), body.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("files_controller.rb"))
        .map(|f| f.content)
        .expect("files_controller emitted")
}

/// The options Rails spells two ways collapse to the one `send_data`
/// takes, and a Symbol disposition becomes the String its RBS declares.
#[test]
fn send_file_grounds_to_send_data_over_a_call_site_read() {
    let src = emitted(
        r#"class FilesController < ApplicationController
  def show
    send_file "app/assets/icon.png", content_type: "image/png", disposition: :inline
  end

  def plain
    send_file "app/assets/icon.png"
  end
end
"#,
    );
    assert!(
        src.contains(
            "send_data(File.binread(\"app/assets/icon.png\"), type: \"image/png\", disposition: \"inline\")"
        ),
        "content_type/disposition ground to send_data's own options:\n{src}"
    );
    assert!(
        src.contains(
            "send_data(File.binread(\"app/assets/icon.png\"), type: \"application/octet-stream\", disposition: \"attachment\")"
        ),
        "the bare form takes Rails' own defaults:\n{src}"
    );
    assert!(!src.contains("send_file"), "no send_file survives:\n{src}");
}

/// An option this lowering does not reproduce leaves the call ALONE, so
/// it fails by name rather than answering a response quietly missing a
/// header.
#[test]
fn an_unmodeled_option_declines_the_rewrite() {
    let src = emitted(
        r#"class FilesController < ApplicationController
  def show
    send_file "app/assets/icon.png", filename: "logo.png"
  end
end
"#,
    );
    assert!(src.contains("send_file"), "the call keeps its source shape:\n{src}");
    assert!(!src.contains("File.binread"), "and nothing is read:\n{src}");
}

/// A body that responds through a private helper gets the GUARDED tail,
/// so the synthesized default render cannot overwrite what the helper
/// already sent.
#[test]
fn a_helper_that_sends_a_file_makes_the_tail_guarded() {
    let src = emitted_with_view(
        r#"class FilesController < ApplicationController
  def show
    if params[:id]
      send_icon
    end
  end

  private
    def send_icon
      send_file "app/assets/icon.png", content_type: "image/png"
    end
end
"#,
        Some(("app/views/files/show.html.erb", "<p>nothing to send</p>\n")),
    );
    let show = src.split("def show").nth(1).unwrap_or("");
    assert!(
        show.contains("performed?"),
        "the synthesized tail is guarded on performed?:\n{src}"
    );
}
