//! Dropped model declarations belong on the ingest-gap ledger without
//! changing analysis or emitted behavior. AASM and state_machine are
//! the currently recognized declaration fixtures, not runtime support
//! or a claim that every unknown class-body statement is reported.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{Analyzer, attribution::attribute_ingest_gaps, diagnose};
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::dialect::ModelBodyItem;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{IngestError, ingest_app_from_tree, survey};

fn tree(model: &str, concern: &str) -> HashMap<PathBuf, Vec<u8>> {
    [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/article.rb", model),
        ("app/models/ticket.rb", "class Ticket < ApplicationRecord\n  include Workflow\nend\n"),
        ("app/models/concerns/workflow.rb", concern),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show\n    @article = Article.find(params[:id])\n    @article.publish! if @article.may_publish?\n    render plain: 'ok'\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n  end\n  create_table :tickets do |t|\n    t.string :state\n  end\nend\n"),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect()
}

fn entries(gaps: &[IngestError]) -> Vec<(&str, &str)> {
    gaps.iter()
        .map(|g| match g {
            IngestError::Unsupported { file, message } => (file.as_str(), message.as_str()),
            other => panic!("unexpected gap: {other}"),
        })
        .collect()
}

#[test]
fn dropped_declarations_are_reported_once_without_changing_analysis_or_emission() {
    let files = tree(
        r#"class Article < ApplicationRecord
  include Workflow
  aasm column: :state do
    state :draft, initial: true
    state :published
    event :publish do
      transitions from: :draft, to: :published
    end
  end
  aasm :review do
    state :pending, initial: true
  end
  state_machine :status, initial: :open do
    event :close do
      transition open: :closed
    end
  end
  def headline
    title
  end
end
"#,
        r#"module Workflow
  extend ActiveSupport::Concern
  included do
    aasm :delivery do
      state :waiting, initial: true
      state :delivered
      event :deliver do
        transitions from: :waiting, to: :delivered
      end
    end
    with_options do
      state_machine :delivery_status, initial: :waiting do
        event :deliver do
          transition waiting: :delivered
        end
      end
    end
  end
end
"#,
    );
    // Deliberately no Gemfile.lock: these entries name unmodeled DSL
    // shapes, not proven ownership by a particular installed gem.
    let mut strict = ingest_app_from_tree(files.clone()).expect("strict ingest");
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    let mut surveyed = result.expect("survey ingest");
    let mut actual = entries(&gaps);
    actual.sort();
    assert_eq!(
        actual,
        vec![
            (
                "app/models/article.rb",
                "state-machine DSL block `aasm` is not modeled"
            ),
            (
                "app/models/article.rb",
                "state-machine DSL block `aasm` is not modeled"
            ),
            (
                "app/models/article.rb",
                "state-machine DSL block `state_machine` is not modeled"
            ),
            (
                "app/models/concerns/workflow.rb",
                "state-machine DSL block `aasm` is not modeled"
            ),
            (
                "app/models/concerns/workflow.rb",
                "state-machine DSL block `state_machine` is not modeled"
            ),
        ]
    );
    assert_eq!(surveyed, strict, "the ledger must not change ingested IR");
    let declarations = |body: &[ModelBodyItem]| {
        body.iter()
            .filter(|item| {
                matches!(item, ModelBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, block: Some(_), .. }
                if matches!(method.as_str(), "aasm" | "state_machine")))
            })
            .count()
    };
    let article = surveyed
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    assert_eq!(
        declarations(&article.body),
        3,
        "model declarations still stay Unknown"
    );
    let ticket = surveyed
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Ticket")
        .unwrap();
    assert_eq!(
        declarations(&ticket.body),
        0,
        "unsupported concern items are not spliced"
    );
    Analyzer::new(&strict).analyze(&mut strict);
    Analyzer::new(&surveyed).analyze(&mut surveyed);
    assert_eq!(surveyed, strict, "no typed methods are added");
    let raw = diagnose(&surveyed);
    assert_eq!(raw, diagnose(&strict));
    let unresolved = raw
        .iter()
        .position(|d| matches!(&d.kind, DiagnosticKind::SendDispatchFailed { method, .. } if method.as_str() == "may_publish?"))
        .expect("the unmodeled call must remain unresolved");
    assert_eq!(raw[unresolved].severity, Severity::Error);
    // Existing file/class gap attribution does affect presentation.
    // This is a coverage note, not a newly supported generated method.
    let mut attributed = raw.clone();
    attribute_ingest_gaps(&mut attributed, &surveyed, &gaps);
    assert_eq!(attributed[unresolved].severity, Severity::Info);
    assert_eq!(attributed[unresolved].kind, raw[unresolved].kind);
    assert!(
        attributed[unresolved]
            .message
            .contains("state-machine DSL block")
    );
    roundhouse::session::analyze_and_lower(&mut strict);
    roundhouse::session::analyze_and_lower(&mut surveyed);
    assert_eq!(
        roundhouse::emit::ruby::emit_lowered_models(&surveyed),
        roundhouse::emit::ruby::emit_lowered_models(&strict)
    );
}

#[test]
fn ordinary_calls_do_not_create_declaration_gaps() {
    survey::activate();
    let result = ingest_app_from_tree(tree(
        r#"class Article < ApplicationRecord
  aasm
  state_machine :state
  aasm(&-> {})
  state_machine(&-> {})
  self.aasm do; end
  Registry.state_machine do; end
  another_dsl do; end
  def ordinary
    aasm do; end
    state_machine do; end
  end
end
"#,
        "module Workflow\n  extend ActiveSupport::Concern\nend\n",
    ));
    let gaps = survey::drain();
    result.expect("non-declaration controls ingest");
    assert!(gaps.is_empty(), "unexpected gaps: {gaps:?}");
}

#[test]
fn declaration_reporting_preserves_original_ingest_failures() {
    let files = tree(
        "class Article < ApplicationRecord\n  aasm do\n    `unsupported_command`\n  end\nend\n",
        "module Workflow\nend\n",
    );
    let strict_error = ingest_app_from_tree(files.clone()).expect_err("strict ingest still fails");
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    result.expect("survey recovers");
    let actual = entries(&gaps);
    assert_eq!(
        actual
            .iter()
            .filter(|entry| **entry
                == (
                    "app/models/article.rb",
                    "state-machine DSL block `aasm` is not modeled"
                ))
            .count(),
        1,
        "one declaration entry even when its body fails"
    );
    let declaration = actual
        .iter()
        .position(|(_, message)| *message == "state-machine DSL block `aasm` is not modeled")
        .unwrap();
    assert!(
        actual
            .iter()
            .skip(declaration + 1)
            .any(|(_, message)| strict_error.to_string().contains(message)),
        "original body failure recorded after the declaration: {strict_error}"
    );
}
