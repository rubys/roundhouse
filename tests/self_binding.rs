//! `#: self as T` (RBS inline) and `T.bind(self, T)` (Sorbet) say what
//! `self` is from that statement to the end of the enclosing body: Rails
//! code writes them in `included do`, DSL and `define_method` blocks whose
//! receiver the block is evaluated against. The typer reads the bound
//! type for implicit- and explicit-self sends; `untyped` makes `self`
//! gradual; a type nobody declares is reported. The binding runs no
//! code, so the emitted Ruby keeps only the statements around it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::emit::ruby;
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;
use roundhouse::App;

const WIDGET: &str = "class Widget\n  #: -> String\n  def label\n    \"w\"\n  end\nend\n";

const BUILDER: &str = "class Builder\n  #: { () -> void } -> void\n  def self.configure(&block)\n    Widget.new.instance_exec(&block)\n  end\nend\n";

fn tree(config: &str) -> HashMap<PathBuf, Vec<u8>> {
    [("app/lib/widget.rb", WIDGET), ("app/lib/builder.rb", BUILDER), ("app/lib/config.rb", config)]
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn analyzed(config: &str) -> App {
    let mut app = ingest_app_from_tree(tree(config)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// The rendered diagnostics for `config`, one per line.
fn check(config: &str) -> String {
    let app = analyzed(config);
    diagnose(&app).iter().map(|d| d.render(&app.sources)).collect::<Vec<_>>().join("\n")
}

/// The type of every send named `method` in `Config`'s methods, in order.
fn send_types(app: &App, method: &str) -> Vec<Option<Ty>> {
    fn walk(expr: &Expr, method: &str, out: &mut Vec<Option<Ty>>) {
        if matches!(&*expr.node, ExprNode::Send { method: m, .. } if m.as_str() == method) {
            out.push(expr.ty.clone());
        }
        expr.node.for_each_child(&mut |child| walk(child, method, out));
    }
    let mut out = Vec::new();
    let config = app.library_classes.iter().find(|c| c.name.0.as_str() == "Config").expect("Config");
    for m in &config.methods {
        walk(&m.body, method, &mut out);
    }
    out
}

#[test]
fn an_implicit_self_send_resolves_against_the_bound_type() {
    let config = "class Config\n  #: -> void\n  def narrowed\n    Builder.configure do\n      #: self as Widget\n      label.upcase\n    end\n  end\n\n  #: -> String\n  def plain\n    #: self as Widget\n    self.label.upcase\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "label"), vec![Some(Ty::Str), Some(Ty::Str)]);
    assert_eq!(send_types(&app, "upcase"), vec![Some(Ty::Str), Some(Ty::Str)]);
}

/// Without the binding the same send has nothing to resolve against.
#[test]
fn an_unbound_block_leaves_the_send_untyped() {
    let config = "class Config\n  #: -> void\n  def narrowed\n    Builder.configure do\n      label.upcase\n    end\n  end\nend\n";
    let app = analyzed(config);
    assert_ne!(send_types(&app, "label"), vec![Some(Ty::Str)]);
}

/// The bound type is checked like any other: a method it does not answer
/// stays unresolved instead of being typed.
#[test]
fn sends_on_the_bound_type_are_checked() {
    let config = "class Config\n  #: -> void\n  def narrowed\n    Builder.configure do\n      #: self as Widget\n      label.frobnicate\n    end\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "label"), vec![Some(Ty::Str)]);
    assert_ne!(send_types(&app, "frobnicate"), vec![Some(Ty::Str)]);
}

#[test]
fn t_bind_binds_self_like_the_comment() {
    let config = "class Config\n  #: -> void\n  def narrowed\n    Builder.configure do\n      T.bind(self, Widget)\n      label.upcase\n    end\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "label"), vec![Some(Ty::Str)]);
}

#[test]
fn self_as_untyped_makes_self_gradual() {
    let config = "class Config\n  #: -> void\n  def loose\n    Builder.configure do\n      #: self as untyped\n      anything.goes\n    end\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "goes"), vec![Some(Ty::Untyped)]);
}

#[test]
fn an_undeclared_bound_type_is_reported() {
    let config = "class Config\n  #: -> void\n  def missing\n    Builder.configure do\n      #: self as Missing::Thing\n      label.upcase\n    end\n  end\nend\n";
    let text = check(config);
    assert!(text.contains("config.rb:5:7: error[unsupported]: declared type"), "{text}");
    assert!(text.contains("Missing::Thing has no indexed declaration"), "{text}");
}

/// The binding holds from its statement on, inside nested blocks too, and
/// not before it or in another method: `own` is Config's, `label` is
/// Widget's.
#[test]
fn the_binding_covers_the_rest_of_its_body_only() {
    let config = "class Config\n  #: -> String\n  def own\n    \"c\"\n  end\n\n  #: -> void\n  def mixed\n    own.upcase\n    #: self as Widget\n    [1].each { label.upcase }\n  end\n\n  #: -> void\n  def after\n    own.upcase\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "own"), vec![Some(Ty::Str), Some(Ty::Str)]);
    assert_eq!(send_types(&app, "label"), vec![Some(Ty::Str)]);
}

/// A binding retypes `self` only: the method it sits in is still the
/// class's that the `def` is written in, and the bound class does not
/// gain it.
#[test]
fn a_bound_method_keeps_its_lexical_owner() {
    let config = "class Config\n  #: -> String\n  def run\n    #: self as Widget\n    label.upcase\n  end\nend\n\nclass Caller\n  #: -> void\n  def call\n    Config.new.run.upcase\n    Widget.new.run\n  end\nend\n";
    let app = analyzed(config);
    let mut runs = Vec::new();
    for class in app.library_classes.iter().filter(|c| c.name.0.as_str() == "Caller") {
        for m in &class.methods {
            fn walk(expr: &Expr, out: &mut Vec<Option<Ty>>) {
                if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "run") {
                    out.push(expr.ty.clone());
                }
                expr.node.for_each_child(&mut |child| walk(child, out));
            }
            walk(&m.body, &mut runs);
        }
    }
    // `Config#run` answers its String; `Widget` never gained `run`.
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0], Some(Ty::Str));
    assert_ne!(runs[1], Some(Ty::Str));
}

/// Above a guard clause the binding covers the guard and what follows it.
#[test]
fn a_binding_above_a_guard_clause_covers_the_rest() {
    let config = "class Config\n  #: (bool) -> void\n  def guarded(skip)\n    #: self as Widget\n    return if skip\n    label.upcase\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "label"), vec![Some(Ty::Str)]);
}

/// The comment above a modifier statement is read once, for the whole
/// statement, though the modifier's body begins where it does.
#[test]
fn a_binding_above_a_modifier_is_read_once() {
    let config = "class Config\n  #: -> void\n  def modified\n    #: self as Missing::Thing\n    label.upcase if true\n  end\nend\n";
    let text = check(config);
    assert_eq!(text.matches("Missing::Thing has no indexed declaration").count(), 1, "{text}");
}

/// A body whose last line also closes its block is not a modifier body.
#[test]
fn a_binding_in_a_brace_block_closed_on_its_last_line_is_read() {
    let config = "class Config\n  #: -> void\n  def braced\n    [1].each {\n      #: self as Widget\n      label.upcase }\n  end\nend\n";
    let app = analyzed(config);
    assert_eq!(send_types(&app, "label"), vec![Some(Ty::Str)]);
}

/// `singleton(X)` and `T.class_of(X)` bind `self` to the class object, so
/// the Module protocol answers on it; an instance binding does not.
#[test]
fn a_class_object_binding_puts_self_on_the_class_side() {
    let config = "class Config\n  #: -> Integer\n  def rbs\n    #: self as singleton(Widget)\n    instance_methods(false).size\n  end\n\n  #: -> Integer\n  def sorbet\n    T.bind(self, T.class_of(Widget))\n    instance_methods(false).size\n  end\n\n  #: -> Integer\n  def instance\n    #: self as Widget\n    instance_methods(false).size\n  end\nend\n";
    let text = check(config);
    let refusals: Vec<&str> = text.lines().filter(|l| l.contains("instance_methods requires a proven class/module object receiver")).collect();
    assert_eq!(refusals.len(), 1, "{text}");
    assert!(refusals[0].contains("config.rb:17:5:"), "{text}");
}

fn emitted_config(config: &str) -> String {
    let app = analyzed(config);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("config.rb"))
        .map(|f| f.content)
        .expect("config.rb")
}

/// Neither form leaves a statement behind; a `T.bind` whose value is the
/// block's keeps its value, `self`.
#[test]
fn the_binding_emits_nothing_but_a_used_value() {
    let config = "class Config\n  #: -> void\n  def narrowed\n    Builder.configure do\n      #: self as Widget\n      label.upcase\n      T.bind(self, Widget)\n      label.downcase\n    end\n  end\n\n  #: -> untyped\n  def answer\n    T.bind(self, Widget)\n  end\nend\n";
    let out = emitted_config(config);
    assert!(out.contains("label.upcase"), "{out}");
    assert!(out.contains("label.downcase"), "{out}");
    assert!(!out.contains("T.bind"), "{out}");
    let bare_self = out.lines().filter(|l| l.trim() == "self").count();
    assert_eq!(bare_self, 1, "only `answer`'s value remains:\n{out}");
}

/// Every target but the Ruby family emits the sends after a binding on
/// the method's own receiver, so it refuses the binding by name; the Ruby
/// family emits it as nothing and builds.
#[test]
fn only_the_ruby_family_emits_a_self_binding() {
    let root = std::env::temp_dir().join(format!("rh_self_binding_targets_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"title\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  def shout\n    #: self as Article\n    title.to_s.upcase\n  end\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let build = |target: &str| {
        let out = root.join(format!("out-{target}"));
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target])
            .arg(&root)
            .arg("-o")
            .arg(&out)
            .output()
            .unwrap();
        (output.status.success(), String::from_utf8_lossy(&output.stderr).into_owned(), out)
    };
    for target in ["rust", "typescript", "go", "crystal"] {
        let (ok, text, _) = build(target);
        assert!(!ok, "{target}: {text}");
        assert!(
            text.contains(&format!("article.rb:3:5: error[unsupported]: self binding not supported ({target})")),
            "{target}: {text}"
        );
    }
    let (ok, text, out) = build("ruby");
    let emitted = std::fs::read_to_string(out.join("app/models/article.rb")).unwrap_or_default();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(ok && !text.contains("self binding"), "{text}");
    assert!(emitted.contains("def shout\n    self.title.to_s.upcase\n  end"), "{emitted}");
}

/// Only the binding shapes bind: a `T.cast(self, X)` statement that the
/// sequence discards is an assertion on a value nobody reads, not a
/// retyping of `self`, and no target refuses it as a binding.
#[test]
fn a_discarded_cast_of_self_binds_nothing() {
    fn bindings(expr: &Expr, out: &mut usize) {
        if roundhouse::expr::is_self_binding(expr) {
            *out += 1;
        }
        expr.node.for_each_child(&mut |child| bindings(child, out));
    }
    let config = "class Config\n  #: -> void\n  def cast_first\n    T.cast(self, Widget)\n    label\n  end\nend\n";
    let app = analyzed(config);
    assert_ne!(send_types(&app, "label"), vec![Some(Ty::Str)]);
    let mut found = 0;
    for class in &app.library_classes {
        for m in &class.methods {
            bindings(&m.body, &mut found);
        }
    }
    assert_eq!(found, 0);
}
