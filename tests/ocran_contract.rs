//! The command-line surface OCRAN's `--roundhouse` mode drives
//! (largo/ocran, lib/ocran/roundhouse_builder.rb). OCRAN packages a Rails
//! app by running `roundhouse --target spinel -o DIR APP`, the asset step
//! and `spin build`, and on failure shows parts of
//! `roundhouse check --continue APP`. Those invocations and the lines it
//! reads back are a contract with a tool outside this repo: changing one
//! breaks OCRAN with nothing here going red, so they are pinned here. Change
//! one deliberately, together with OCRAN.

use std::path::{Path, PathBuf};
use std::process::Command;

fn roundhouse(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(args)
        .output()
        .expect("spawn roundhouse");
    // OCRAN reads stdout and stderr as one stream (Open3.capture2e).
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-ocran-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write_app(root: &Path, files: &[(&str, &str)]) {
    for (path, source) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
}

const CLEAN_APP: &[(&str, &str)] = &[
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def index\n    @articles = Article.all\n  end\nend\n"),
    ("app/views/articles/index.html.erb", "<% @articles.each do |article| %>\n  <p><%= article.title %></p>\n<% end %>\n"),
    ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
    ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :index\nend\n"),
];

/// `roundhouse --target spinel [--survey --allow-unsupported] -o DIR APP`
/// writes a spin project whose Makefile has the `assets` target OCRAN runs
/// before `spin build`.
#[test]
fn transpile_to_spinel_writes_a_spin_project_with_an_assets_step() {
    let root = scratch("transpile");
    let app = root.join("app");
    write_app(&app, CLEAN_APP);

    for (label, extra) in [("plain", &[][..]), ("survey", &["--survey", "--allow-unsupported"][..])] {
        let out = root.join(label);
        let mut args = vec!["--target", "spinel"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["-o", out.to_str().unwrap(), app.to_str().unwrap()]);
        let (code, text) = roundhouse(&args);
        assert_eq!(code, 0, "{label}: {text}");
        assert!(out.join("spin.toml").is_file(), "{label}: no spin.toml\n{text}");
        let makefile = std::fs::read_to_string(out.join("Makefile")).expect("Makefile");
        assert!(
            makefile.lines().any(|l| l.starts_with("assets:")),
            "{label}: the Makefile has no `assets:` target"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// What OCRAN greps out of `roundhouse check --continue APP`: the summary
/// and gem-census lines (`^roundhouse-check:`, with an `N unknown` count),
/// the diagnostics (`: error[`), and the `Survey:` punch list.
#[test]
fn check_continue_prints_the_lines_ocran_reads() {
    let root = scratch("check");
    let mut files = CLEAN_APP.to_vec();
    files.retain(|(path, _)| *path != "app/controllers/articles_controller.rb");
    files.extend_from_slice(&[
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show\n    render json: {article: ArticleResource.new(Article.find(params[:id])).to_h}\n  end\nend\n"),
        ("app/resources/application_resource.rb", "class ApplicationResource\n  include Alba::Resource\nend\n"),
        ("app/resources/article_resource.rb", "class ArticleResource < ApplicationResource\n  attributes :id, :title\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
        ("Gemfile.lock", include_str!("../fixtures/gem-capabilities/historical/locks/alba.lock")),
    ]);
    write_app(&root, &files);

    let (code, text) = roundhouse(&["check", "--continue", root.to_str().unwrap()]);
    assert_ne!(code, 0, "an app with an error must not exit 0: {text}");
    let lines: Vec<&str> = text.lines().collect();
    let summary: Vec<&&str> = lines.iter().filter(|l| l.starts_with("roundhouse-check:")).collect();
    assert!(
        summary.iter().any(|l| l.contains(" gems: ") && l.contains(" unknown")),
        "no `roundhouse-check: N gems: ... N unknown` census line: {text}"
    );
    assert!(
        summary.iter().any(|l| l.contains(" error(s)")),
        "no `roundhouse-check:` summary line: {text}"
    );
    assert!(lines.iter().any(|l| l.contains(": error[")), "no `: error[` diagnostic: {text}");
    assert!(lines.iter().any(|l| l.contains("Survey:")), "no `Survey:` section: {text}");
    std::fs::remove_dir_all(root).unwrap();
}
