//! Rails reaches a private method only through an implicit or a literal
//! `self` receiver; `article.secret` from outside raises NoMethodError.
//! The typer dispatched by name alone, so the call typed and `check` was
//! clean while the program raised.

use super::emit_and_run;

fn app(caller: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        &format!("  validates :title, presence: true\n\n  def self.helper\n    1\n  end\n  private_class_method :helper\n\n  def reveal\n{caller}\n  end\n\n  private\n\n  def secret\n    title\n  end\n"),
    )
}

#[test]
fn a_private_method_called_on_another_receiver_is_an_error() {
    for caller in ["    Article.new.secret", "    Article.helper"] {
        let errors = app(caller).check_errors();
        assert!(errors.iter().any(|e| e.contains("private method `") && e.contains("only `self` reaches it")), "{caller}: {errors:#?}");
    }
}

#[test]
fn self_and_implicit_receivers_reach_a_private_method() {
    for caller in ["    secret", "    self.secret", "    self.class.send(:helper)"] {
        let errors = app(caller).check_errors();
        assert!(errors.is_empty(), "{caller}: {errors:#?}");
    }
}

#[test]
fn a_module_s_private_class_method_is_an_error_from_outside() {
    let errors = emit_and_run::real_blog()
        .write("lib/toolkit.rb", "module Toolkit\n  def self.secret\n    1\n  end\n  private_class_method :secret\n\n  def self.open\n    secret\n  end\nend\n")
        .edit("app/models/article.rb", "  validates :title, presence: true\n", "  validates :title, presence: true\n\n  def self.peek\n    Toolkit.secret + Toolkit.open\n  end\n")
        .check_errors();
    assert_eq!(errors.iter().filter(|e| e.contains("private method `secret` called on Toolkit")).count(), 1, "{errors:#?}");
}
