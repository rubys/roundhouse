//! A source constant holding a value from an undeclared gem remains
//! unsupported. It must not become a phantom class or turn its readers
//! into clean support without a declaration and runtime implementation.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static RUN: AtomicUsize = AtomicUsize::new(0);

fn check(files: &[(&str, &str)]) -> String {
    let dir = std::env::temp_dir()
        .join(format!("rh_const_gem_value_{}_{}", std::process::id(), RUN.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, src) in files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, src).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(&dir)
        .output()
        .expect("spawn roundhouse");
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

#[test]
fn a_value_from_an_undeclared_gem_constant_is_unsupported() {
    let model = "class Widget < ApplicationRecord\n  MIN_PRICE = Money.new(1, \"USD\")\n\n  def price\n    MIN_PRICE.currency\n    MIN_PRICE.value.positive?\n    Widget::MIN_PRICE.value\n  end\nend\n";
    let err = check(&[("app/models/application_record.rb", RECORD), ("app/models/widget.rb", model)]);
    assert!(err.contains("error[unsupported]"), "{err}");
    assert!(err.contains("Widget::MIN_PRICE"), "{err}");
    assert!(!err.contains("send_dispatch_failed"), "{err}");
}

/// A written class name is still the class object: the gem class is
/// unregistered, so a constant read of it is not a value.
#[test]
fn a_written_class_name_is_still_a_class_object() {
    let model = "class Widget < ApplicationRecord\n  def price\n    Money.no_such_class_method\n  end\nend\n";
    let err = check(&[("app/models/application_record.rb", RECORD), ("app/models/widget.rb", model)]);
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.contains("constant not supported (all targets): Money"), "{err}");
}

/// A cataloged gem (`htmlentities`) is declared, so its constant is a
/// class and the source constant holding its instance is typed. This is
/// lobsters' `HtmlEncoder`, whose class body runs `HTMLEntities.new` at
/// load: a refusal stub there fails the whole app before any request.
#[test]
fn a_value_from_a_cataloged_gem_constant_is_typed() {
    let encoder = "module HtmlEncoder\n  HTML_ENTITIES = HTMLEntities.new\n\n  class << self\n    def encode(string, type = :decimal)\n      HTML_ENTITIES.encode(string, type)\n    end\n  end\nend\n";
    let err = check(&[("app/models/application_record.rb", RECORD), ("app/models/html_encoder.rb", encoder)]);
    assert!(!err.contains("constant not supported"), "{err}");
}

/// commonmarker is cataloged: lobsters' `Markdowner` parses to a node
/// tree, edits it and renders it back, so its constants are classes and
/// the parse answers a typed node.
#[test]
fn commonmarker_constants_and_nodes_are_typed() {
    let markdowner = "class Markdowner\n  def self.to_raw(text)\n    root = Commonmarker.parse(text, options: {})\n    root.walk do |node|\n      node.delete if node.type == :image\n    end\n    root.to_commonmark\n  end\n\n  def self.mention(user)\n    link_text = Commonmarker::Node.new(:text)\n    link_text.string_content = user\n    link_text\n  end\nend\n";
    let err = check(&[("app/models/application_record.rb", RECORD), ("app/models/markdowner.rb", markdowner)]);
    assert!(!err.contains("constant not supported"), "{err}");
    assert!(!err.contains("send_dispatch_failed"), "{err}");
}

/// Nokogiri is cataloged, and so is its HTML5 parser: `Nokogiri::HTML5`
/// is the module the gem defines (gumbo's, which spinel-nokogiri also
/// carries), so a helper that parses a fragment with it, walks its text
/// nodes and serializes it back reads a declared constant, as the
/// `Nokogiri::HTML` method call already does.
#[test]
fn nokogiri_html5_constants_are_typed() {
    let helper = "module WidgetsHelper\n  def widget_markup(html)\n    fragment = Nokogiri::HTML5.fragment(html)\n    fragment.xpath(\".//text()\").each do |node|\n      node.replace Nokogiri::HTML5.fragment(node.to_html.upcase)\n    end\n    document = Nokogiri::HTML5(html)\n    Nokogiri::HTML5::Document.parse(html)\n    Nokogiri::HTML5::DocumentFragment.parse(html)\n    [fragment.to_html(preserve_newline: true), document]\n  end\nend\n";
    let err = check(&[("app/models/application_record.rb", RECORD), ("app/helpers/widgets_helper.rb", helper)]);
    assert!(!err.contains("constant not supported"), "{err}");
    assert!(!err.contains("send_dispatch_failed"), "{err}");
}
