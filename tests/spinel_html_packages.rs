//! The spinel tree's HTML and Markdown packages, wired on demand.
//!
//! An app that names `Nokogiri` gets spinel-nokogiri (libxml2 with
//! Nokogiri's patches, carried): the raising façade at
//! `runtime/nokogiri_facade.rb` becomes `require "nokogiri"` and the
//! manifest declares the package. One that names `Commonmarker` gets
//! spinel-commonmarker (cmark-gfm, carried), required through
//! `runtime/gem_facades.rb` since the app's own gem require does not
//! survive the emit. With both, lobsters' Markdowner façade stands aside
//! and the real class serves. An app that names neither keeps the façade
//! and declares nothing.

use std::path::Path;

use roundhouse::ingest::ingest_app;
use roundhouse::project::{target_files, BuildTarget};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

const MARKDOWNER: &str = r#"class Markdowner
  def self.to_html(text)
    html = Commonmarker.to_html(text.to_s)
    doc = Nokogiri::HTML(html)
    doc.css("a").each { |a| a[:rel] = "ugc" }
    doc.at_css("body").inner_html
  end
end
"#;

fn spinel_files(tag: &str, extra: Option<&str>) -> Vec<(String, String)> {
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let dir = std::env::temp_dir().join(format!("roundhouse-html-packages-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["app", "config", "db"] {
        copy_dir(&fixture.join(sub), &dir.join(sub));
    }
    if let Some(body) = extra {
        std::fs::write(dir.join("app/models/markdowner.rb"), body).unwrap();
    }
    let app = ingest_app(&dir).expect("ingest");
    let files = target_files(&app, &dir, BuildTarget::Spinel).expect("spinel files");
    let _ = std::fs::remove_dir_all(&dir);
    files
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("{path} missing")).1
}

#[test]
fn an_app_naming_nokogiri_and_commonmarker_gets_both_packages() {
    let files = spinel_files("both", Some(MARKDOWNER));
    let manifest = file(&files, "spin.toml");
    assert!(manifest.contains("nokogiri = { git = \"https://github.com/rubys/spinel-nokogiri\""), "{manifest}");
    assert!(manifest.contains("commonmarker = { git = \"https://github.com/rubys/spinel-commonmarker\""), "{manifest}");
    assert!(file(&files, "runtime/nokogiri_facade.rb").contains("require \"nokogiri\""));
    assert!(!files.iter().any(|(p, _)| p == "runtime/nokogiri_facade.rbs"));
    assert!(file(&files, "runtime/gem_facades.rb").contains("require \"commonmarker\""));
}

#[test]
fn an_app_naming_neither_keeps_the_facade_and_declares_nothing() {
    let files = spinel_files("neither", None);
    let manifest = file(&files, "spin.toml");
    assert!(!manifest.contains("spinel-nokogiri"), "{manifest}");
    assert!(!manifest.contains("spinel-commonmarker"), "{manifest}");
    assert!(file(&files, "runtime/nokogiri_facade.rb").contains("module Nokogiri"));
    assert!(!file(&files, "runtime/gem_facades.rb").contains("require \"commonmarker\""));
}
