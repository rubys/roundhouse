//! roundhouse#1048 end-to-end: a trailing-comma Ruby continuation in a
//! HAML `=` line (`= select_tag :status,` + an indented next line) must
//! not crash `roundhouse check --continue` — the joined line's mapped
//! segment used to overshoot the first source line's end, so a
//! diagnostic span inside the continuation translated to a template
//! offset that could split a multi-byte character, panicking
//! `SourceFile::line_col`. See tests/view_engines.rs for the narrower
//! in-process regression.

use std::process::Command;

fn check(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("check")
        .args(args)
        .output()
        .expect("spawn roundhouse");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn continuation_with_a_multibyte_literal_does_not_panic() {
    let root = std::env::temp_dir().join(format!(
        "roundhouse-1048-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (path, source) in [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def show\n    @post = Post.find(params[:id])\n  end\nend\n",
        ),
        (
            "app/views/posts/show.html.haml",
            "= select_tag :status,\n    options_for_select(['アクティブ', @post.unknown])\n",
        ),
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define do\n  create_table \"posts\" do |t|\n    t.string \"status\"\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :posts, only: :show\nend\n",
        ),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let app = root.to_str().unwrap();
    let (code, _stdout, stderr) = check(&["--continue", app]);
    std::fs::remove_dir_all(&root).ok();

    assert!(
        !stderr.contains("panicked"),
        "roundhouse must not panic on the continuation: {stderr}"
    );
    assert_eq!(code, Some(1), "expected a clean (non-signal) failing exit: {stderr}");
    assert!(
        stderr.contains("show.html.haml:1:22: error[send_dispatch_failed]: no known method `unknown` on Post"),
        "{stderr}"
    );
}
