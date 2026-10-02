//! Unknown-gem attribution from app-declared generated surfaces (#142).
//! State machines are the first generator fixtures, not a new public
//! attribution pass. Names remain unresolved; emit is unchanged.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::attribution::{attribute_ingest_gaps, attribute_unknown_gems};
use roundhouse::analyze::{Analyzer, diagnose};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::ingest::{IngestError, ingest_app_from_tree, survey};

const LOCK: &str = "GEM
  remote: https://rubygems.org/
  specs:
    aasm (6.0.0)
    paper_trail (16.0.0)
    rails (8.1.4)
    state_machines-activerecord (0.200.0)

DEPENDENCIES
  aasm
  paper_trail
  rails
  state_machines-activerecord
";

const ARTICLE: &str = "class Article < ApplicationRecord
  include Moderation

  aasm column: :state do
    state :draft, initial: true
    state :published

    event :publish do
      transitions from: :draft, to: :published
    end
  end

  def headline
    title
  end
end
";

const MODERATION: &str = "module Moderation
  extend ActiveSupport::Concern

  included do
    state_machine :status, initial: :open do
      event :close do
        transition open: :closed
      end
    end
  end
end
";

const SHOW: &str = "class ArticlesController < ApplicationController
  def show
    @article = Article.find(params[:id])
    @article.publish! if @article.may_publish?
    @versions = @article.versions
    @article.close if @article.can_close?
    @drafts = Article.draft
    @typo = @article.titel
    @comment = Comment.new
    @comment.publish!
    render plain: 'ok'
  end
end
";

fn tree(lock: Option<&str>) -> HashMap<PathBuf, Vec<u8>> {
    let mut files = vec![
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("app/models/article.rb", ARTICLE),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\nend\n",
        ),
        ("app/models/concerns/moderation.rb", MODERATION),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/articles_controller.rb", SHOW),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n",
        ),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n    t.string :state\n    t.string :status\n  end\n  create_table :comments do |t|\n    t.string :body\n  end\nend\n",
        ),
    ];
    files.extend(lock.map(|l| ("Gemfile.lock", l)));
    files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect()
}

/// Preserve the existing caller order: ingest gaps, then unknown gems.
fn check(files: HashMap<PathBuf, Vec<u8>>, continue_: bool) -> (Vec<Diagnostic>, Vec<IngestError>) {
    if continue_ {
        survey::activate();
    }
    let result = ingest_app_from_tree(files);
    let gaps = if continue_ {
        survey::drain()
    } else {
        Vec::new()
    };
    let mut app = result.expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let mut diags = diagnose(&app);
    attribute_ingest_gaps(&mut diags, &app, &gaps);
    attribute_unknown_gems(&mut diags, &app);
    (diags, gaps)
}

fn dispatch<'a>(diags: &'a [Diagnostic], method: &str, recv: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| {
            matches!(&d.kind, DiagnosticKind::SendDispatchFailed { method: m, recv_ty }
                if m.as_str() == method && format!("{recv_ty:?}").contains(&format!("\"{recv}\"")))
        })
        .unwrap_or_else(|| panic!("no dispatch failure for `{method}` on {recv}: {diags:#?}"))
}

fn assert_note(d: &Diagnostic, needle: &str) {
    assert_eq!(d.severity, Severity::Info, "{}", d.message);
    assert!(
        d.message.contains(needle),
        "`{needle}` missing from: {}",
        d.message
    );
}

#[test]
fn a_call_to_a_name_the_declaration_generates_is_attributed_to_the_gem() {
    let (diags, _) = check(tree(Some(LOCK)), false);
    let aasm = "matches the declared `aasm` surface in Article";
    assert_note(dispatch(&diags, "publish!", "Article"), aasm);
    assert_note(dispatch(&diags, "publish!", "Article"), "the `aasm` gem");
    assert_note(dispatch(&diags, "may_publish?", "Article"), aasm);
    assert_note(dispatch(&diags, "draft", "Article"), aasm);
    // Declared in a concern's `included do`, called on the includer.
    let machine = "matches the declared `state_machine` surface in Moderation";
    assert_note(dispatch(&diags, "close", "Article"), machine);
    assert_note(
        dispatch(&diags, "close", "Article"),
        "the `state_machines-activerecord` gem",
    );
    assert_note(dispatch(&diags, "can_close?", "Article"), machine);
    // Only names a declaration generates, and only on its receivers.
    assert_eq!(
        dispatch(&diags, "publish!", "Comment").severity,
        Severity::Error
    );
    assert_note(
        dispatch(&diags, "versions", "Article"),
        "the `paper_trail` gem",
    );
    assert_eq!(
        dispatch(&diags, "titel", "Article").severity,
        Severity::Error
    );
}

#[test]
fn existing_ingest_gap_order_takes_precedence_over_all_gem_evidence() {
    let (diags, gaps) = check(tree(Some(LOCK)), true);
    // General ordering question, not a generator-specific override:
    // a file gap claims declared and fixed gem surfaces (and typos).
    for method in [
        "publish!",
        "may_publish?",
        "draft",
        "close",
        "can_close?",
        "versions",
        "titel",
    ] {
        assert_note(
            dispatch(&diags, method, "Article"),
            "ingest gap in app/models/article.rb",
        );
    }
    assert_eq!(
        dispatch(&diags, "publish!", "Comment").severity,
        Severity::Error
    );
    assert!(
        gaps.iter().any(|gap| gap
            .to_string()
            .contains("state-machine DSL block `aasm` is not modeled")),
        "{gaps:?}"
    );
}

#[test]
fn a_declaration_whose_gem_is_absent_claims_nothing() {
    let (diags, _) = check(tree(None), false);
    assert_eq!(
        dispatch(&diags, "publish!", "Article").severity,
        Severity::Error
    );
    assert_eq!(
        dispatch(&diags, "close", "Article").severity,
        Severity::Error
    );

    let aasm_only = LOCK.replace("    state_machines-activerecord (0.200.0)\n", "");
    let (diags, _) = check(tree(Some(&aasm_only)), false);
    assert_eq!(
        dispatch(&diags, "publish!", "Article").severity,
        Severity::Info
    );
    assert_eq!(
        dispatch(&diags, "close", "Article").severity,
        Severity::Error,
        "a declaration whose gem is absent claims nothing"
    );
}

fn source_app(source: &str) -> roundhouse::App {
    let mut files = tree(Some(LOCK));
    files.insert(
        PathBuf::from("app/services/workflows.rb"),
        source.as_bytes().to_vec(),
    );
    ingest_app_from_tree(files).unwrap()
}

fn failed(receiver: roundhouse::ty::Ty, method: &str) -> Diagnostic {
    Diagnostic {
        span: roundhouse::span::Span::synthetic(),
        severity: Severity::Error,
        message: format!("no known method `{method}`"),
        kind: DiagnosticKind::SendDispatchFailed {
            method: method.into(),
            recv_ty: receiver,
        },
    }
}

fn class(name: &str) -> roundhouse::ty::Ty {
    roundhouse::ty::Ty::Class {
        id: roundhouse::ClassId(name.into()),
        args: vec![],
    }
}

#[test]
fn declarations_in_a_parent_or_concern_reach_the_receiver() {
    for (source, owner) in [
        (
            "class WorkflowBase\n  aasm do\n    event :finish\n  end\nend\nclass WorkflowRecord < WorkflowBase\nend\n",
            "WorkflowBase",
        ),
        (
            "module WorkflowDeclarations\n  extend ActiveSupport::Concern\n  included do\n    with_options do\n      aasm do\n        event :finish\n      end\n    end\n  end\nend\nclass WorkflowRecord\n  include WorkflowDeclarations\nend\n",
            "WorkflowDeclarations",
        ),
    ] {
        let app = source_app(source);
        let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
        attribute_unknown_gems(&mut diags, &app);
        assert_note(
            &diags[0],
            &format!("matches the declared `aasm` surface in {owner}"),
        );
    }
}

#[test]
fn names_no_declaration_generates_and_unproven_receivers_stay_errors() {
    use roundhouse::ty::Ty;
    let app = source_app(
        "class WorkflowRecord\n  aasm do\n    event :finish\n  end\nend\nclass Other\nend\n",
    );
    for (receiver, method) in [
        (class("WorkflowRecord"), "finsh!"),
        (class("Other"), "finish!"),
        (
            Ty::Relation {
                of: roundhouse::ClassId("WorkflowRecord".into()),
            },
            "finish!",
        ),
        (
            Ty::Union {
                variants: vec![class("WorkflowRecord"), class("Other")],
            },
            "finish!",
        ),
    ] {
        let mut diags = vec![failed(receiver, method)];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before);
    }
    let mut nullable = vec![failed(
        Ty::Union {
            variants: vec![class("WorkflowRecord"), Ty::Nil],
        },
        "finish!",
    )];
    attribute_unknown_gems(&mut nullable, &app);
    assert_note(&nullable[0], "the `aasm` gem");
}

#[test]
fn reopening_and_unreadable_configuration_do_not_invent_generated_surfaces() {
    for declaration in [
        "state_machine { event :finish }",
        "state_machine machine_name do; event :finish; end",
        "state_machine :status, namespace: computed_namespace do; event :finish; end",
        "state_machine :status, **options do; event :finish; end",
        "state_machine :status, namespace: :first, namespace: :second do; event :finish; end",
        "state_machine :status, namespace: :alarm do; end; state_machine :status do; event :finish; end",
        "state_machine :status, namespace: :alarm do; end; state_machine do; event :finish; end",
        "state_machine :status do; event :finish; end; state_machine :other do; end",
        "state_machine :status do; event :finish; end; state_machine",
        "with_options namespace: :alarm do; state_machine :status do; event :finish; end; end",
        "aasm namespace: computed_namespace do; event :finish; end",
        "aasm do; event :finish; end; aasm",
    ] {
        let app = source_app(&format!("class WorkflowRecord\n  {declaration}\nend\n"));
        let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before, "must not guess: {declaration}");
    }
    let app = source_app(
        "class WorkflowBase\n  state_machine :status, namespace: :alarm do; end\nend\nclass WorkflowRecord < WorkflowBase\n  state_machine :status do; event :finish; end\nend\n",
    );
    let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
    attribute_unknown_gems(&mut diags, &app);
    assert_eq!(
        diags[0].severity,
        Severity::Error,
        "inherited reopening is not an isolated declaration"
    );
}

#[test]
fn unsupported_same_owner_declarations_veto_isolation() {
    for (source, receiver) in [
        (
            "class WorkflowRecord\n  self.state_machine :status, namespace: :alarm do; end\n  state_machine :status do; event :finish; end\nend\n",
            "WorkflowRecord",
        ),
        (
            "class WorkflowRecord\n  if true; state_machine :status, namespace: :alarm do; end; end\n  state_machine :status do; event :finish; end\nend\n",
            "WorkflowRecord",
        ),
        (
            "class WorkflowRecord; end\nWorkflowRecord.state_machine :status, namespace: :alarm do; end\nclass WorkflowRecord\n  state_machine :status do; event :finish; end\nend\n",
            "WorkflowRecord",
        ),
        (
            "if true\n  class WorkflowRecord\n    state_machine :status, namespace: :alarm do; end\n  end\nend\nclass WorkflowRecord\n  state_machine :status do; event :finish; end\nend\n",
            "WorkflowRecord",
        ),
        (
            "module Workflows\n  class Record; end\n  class Workflows::Record\n    state_machine :status, namespace: :alarm do; end\n  end\n  class Record\n    state_machine :status do; event :finish; end\n  end\nend\n",
            "Workflows::Record",
        ),
        (
            "module Outer\n  module Workflows\n    class Record; end\n  end\n  module Container\n    class Workflows::Record\n      state_machine :status, namespace: :alarm do; end\n    end\n  end\n  module Workflows\n    class Record\n      state_machine :status do; event :finish; end\n    end\n  end\nend\n",
            "Outer::Workflows::Record",
        ),
        (
            r#"module Outer
  module Workflows
    class Record
      class Child; end
    end
  end
  module Container
    class Workflows::Record
      class Child
        state_machine :status, namespace: :alarm do; end
      end
    end
  end
  module Workflows
    class Record
      class Child
        state_machine :status do; event :finish; end
      end
    end
  end
end
"#,
            "Outer::Workflows::Record::Child",
        ),
    ] {
        let app = source_app(source);
        let mut diags = vec![failed(class(receiver), "finish!")];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before, "must not assume isolation: {source}");
    }
    let app = source_app(
        "class WorkflowRecord\n  state_machine :status do; event :finish; end\nend\nmodule Other\n  if true; class WorkflowRecord; end; end\nend\n",
    );
    let mut unaffected = vec![failed(class("WorkflowRecord"), "finish!")];
    attribute_unknown_gems(&mut unaffected, &app);
    assert_note(
        &unaffected[0],
        "matches the declared `state_machine` surface",
    );
}

#[test]
fn unsupported_integration_configuration_claims_no_adapter_surface() {
    let methods = [
        "with_status",
        "with_statuses",
        "without_status",
        "without_statuses",
        "status_event",
        "status_event=",
        "status_event_transition",
        "status_event_transition=",
    ];
    for integration in ["nil", "false"] {
        let app = source_app(&format!(
            "class WorkflowRecord < ApplicationRecord\n  state_machine :status, integration: {integration} do; event :finish; end\nend\n"
        ));
        for method in methods {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            let before = diags.clone();
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(
                diags, before,
                "explicit integration {integration} does not generate {method}"
            );
        }
    }
    let app = source_app(
        "class WorkflowRecord < ApplicationRecord\n  state_machine :status do; event :finish; end\nend\n",
    );
    for method in methods {
        let mut diags = vec![failed(class("WorkflowRecord"), method)];
        attribute_unknown_gems(&mut diags, &app);
        assert_note(&diags[0], "matches the declared `state_machine` surface");
    }
}

#[test]
fn integration_dependent_names_require_the_receivers_adapter_evidence() {
    let source = "module WorkflowDeclarations\n  extend ActiveSupport::Concern\n  included do\n    state_machine :status do; event :finish; end\n    aasm do; state :draft; end\n  end\nend\nclass Plain\n  include WorkflowDeclarations\nend\nclass Persisted < ApplicationRecord\n  include WorkflowDeclarations\nend\n";
    let app = source_app(source);
    for method in [
        "with_status",
        "without_statuses",
        "status_event",
        "status_event_transition=",
        "draft",
    ] {
        let mut plain = vec![failed(class("Plain"), method)];
        let before = plain.clone();
        attribute_unknown_gems(&mut plain, &app);
        assert_eq!(plain, before, "plain receiver does not supply {method}");
        let mut persisted = vec![failed(class("Persisted"), method)];
        attribute_unknown_gems(&mut persisted, &app);
        assert_note(&persisted[0], "matches the declared");
    }
    for method in ["finish!", "can_finish?", "draft?"] {
        let mut plain = vec![failed(class("Plain"), method)];
        attribute_unknown_gems(&mut plain, &app);
        assert_note(&plain[0], "matches the declared");
    }
    let mut core_only = app.clone();
    core_only
        .gem_lock
        .as_mut()
        .unwrap()
        .specs
        .retain(|(name, _)| name != "state_machines-activerecord");
    // The provider still exists, but its ActiveRecord adapter does not.
    core_only
        .gem_lock
        .as_mut()
        .unwrap()
        .specs
        .push(("state_machines".into(), "0.202.0".into()));
    let mut scoped = vec![failed(class("Persisted"), "with_status")];
    let before = scoped.clone();
    attribute_unknown_gems(&mut scoped, &core_only);
    assert_eq!(scoped, before);
}

#[test]
fn an_included_wrapper_requires_established_concern_identity() {
    let app = source_app(
        "module WorkflowDeclarations\n  def self.included(base = nil); end\n  included do; aasm { event :finish }; end\nend\nclass WorkflowRecord\n  include WorkflowDeclarations\nend\n",
    );
    let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
    let before = diags.clone();
    attribute_unknown_gems(&mut diags, &app);
    assert_eq!(diags, before);

    for (marker, accepted) in [
        ("ActiveSupport::Concern", false),
        ("::ActiveSupport::Concern", true),
    ] {
        let app = source_app(&format!(
            "module WorkflowDeclarations\n  extend {marker}\n  included do; aasm {{ event :finish }}; end\n  module ActiveSupport\n    module Concern; end\n  end\nend\nclass WorkflowRecord; include WorkflowDeclarations; end\n"
        ));
        let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(
            diags[0].severity == Severity::Info,
            accepted,
            "Concern marker {marker} with a local shadow"
        );
    }
}

#[test]
fn rejected_owners_still_expose_rooted_configuration_reopens() {
    for configuration in [
        "::WorkflowRecord.state_machine :status, namespace: :alarm do; end",
        "nil",
    ] {
        let app = source_app(&format!(
            "class WorkflowRecord; end\nmodule Shell; end\nmodule Wrapper\n  class Shell::Holder\n    {configuration}\n  end\nend\nclass WorkflowRecord\n  state_machine :status do; event :finish; end\nend\n"
        ));
        let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        if configuration == "nil" {
            assert_note(&diags[0], "matches the declared");
        } else {
            assert_eq!(diags, before);
        }
    }
}

#[test]
fn visible_configuration_mutation_does_not_claim_the_old_names() {
    for body in [
        "state_machine.config.namespace = :alarm; event :finish",
        "event :finish do; state_machine.config.namespace = :alarm; end",
    ] {
        let app = source_app(&format!(
            "class WorkflowRecord < ApplicationRecord\n  aasm do; {body}; end\nend\n"
        ));
        for method in ["finish!", "may_finish?"] {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            let before = diags.clone();
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(diags, before, "configuration mutation: {body}");
        }
    }
}

#[test]
fn unexecuted_or_mutated_event_contexts_claim_no_states() {
    for body in [
        "event all do; transition phantom: :closed; end",
        "event({}) do; transition phantom: :closed; end",
        "transition phantom: :closed, on: []",
        "transition phantom: :closed, on: all",
        "event :finish do; transition phantom: :closed; reset; transition open: :closed; end",
        "event :finish do; transition phantom: :closed; transition open: :closed, if: (reset; :ready?); end",
        "state :open do; machine.instance_variable_set(:@namespace, :alarm); end; event :finish",
        "instance_variable_set(:@namespace, :alarm); event :finish",
    ] {
        let app = source_app(&format!(
            "class WorkflowRecord\n  state_machine :status do; {body}; end\nend\n"
        ));
        for method in ["phantom?", "finish!"] {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            let before = diags.clone();
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(diags, before, "unsupported event context: {body}");
        }
    }
    for body in [
        "transition phantom: :closed, on: :finish",
        "event :finish do; transition phantom: :closed; transition open: :closed, if: -> { reset; :ready? }; end",
    ] {
        let app = source_app(&format!(
            "class WorkflowRecord\n  state_machine :status do; {body}; end\nend\n"
        ));
        for method in ["phantom?", "closed?", "finish!"] {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            attribute_unknown_gems(&mut diags, &app);
            assert_note(&diags[0], "matches the declared `state_machine` surface");
        }
    }
}

#[test]
fn constant_binding_shadows_do_not_prove_concern_ownership() {
    for (marker, accepted) in [
        ("ActiveSupport::Concern", false),
        ("::ActiveSupport::Concern", true),
    ] {
        let app = source_app(&format!(
            "module FauxSupport\n  module Concern\n    def included(base = nil); end\n  end\nend\nmodule WorkflowDeclarations\n  ActiveSupport = FauxSupport\n  extend {marker}\n  included do; state_machine :status do; event :finish; end; end\nend\nclass WorkflowRecord; include WorkflowDeclarations; end\n"
        ));
        let mut diags = vec![failed(class("WorkflowRecord"), "finish!")];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(
            diags[0].severity == Severity::Info,
            accepted,
            "binding shadow {marker}"
        );
    }
}

#[test]
fn inferred_plural_names_do_not_normalize_literal_spelling() {
    let app = source_app(
        "class WorkflowRecord < ApplicationRecord\n  state_machine :reviewStatus, attribute: :status do; event :finish; end\nend\n",
    );
    for method in ["with_review_statuses", "without_review_statuses"] {
        let mut diags = vec![failed(class("WorkflowRecord"), method)];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before);
    }
    let mut singular = vec![failed(class("WorkflowRecord"), "with_reviewStatus")];
    attribute_unknown_gems(&mut singular, &app);
    assert_note(&singular[0], "matches the declared");
}

#[test]
fn opaque_transition_arguments_veto_the_entire_declared_surface() {
    for transition in [
        "transition from: :phantom, from: :open, to: :closed",
        "transition from: :phantom, to: :closed, **{ from: :open }",
        "transition({ from: :phantom, **options })",
        "transition options, from: :phantom, to: :closed",
    ] {
        let app = source_app(&format!(
            "class WorkflowRecord\n  state_machine :status do; event(:finish) {{ {transition} }}; end\nend\n"
        ));
        for method in ["phantom?", "finish!"] {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            let before = diags.clone();
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(diags, before, "unproven declaration: {transition}");
        }
    }
}

#[test]
fn all_literal_names_in_a_state_declaration_reach_the_surface() {
    let app = source_app(
        "class WorkflowRecord < ApplicationRecord\n  aasm do; state :draft, :published; end\nend\n",
    );
    for method in ["draft", "draft?", "published", "published?"] {
        let mut diags = vec![failed(class("WorkflowRecord"), method)];
        attribute_unknown_gems(&mut diags, &app);
        assert_note(&diags[0], "matches the declared `aasm` surface");
    }
}

#[test]
fn literal_configuration_changes_the_declared_surface() {
    let app = source_app(
        "class WorkflowRecord < ApplicationRecord\n  state_machine :status, plural: :workflow_statuses do; end\nend\n",
    );
    for method in ["with_workflow_statuses", "without_workflow_statuses"] {
        let mut diags = vec![failed(class("WorkflowRecord"), method)];
        attribute_unknown_gems(&mut diags, &app);
        assert_note(&diags[0], "matches the declared `state_machine` surface");
    }
    for method in ["with_statuses", "without_statuses"] {
        let mut diags = vec![failed(class("WorkflowRecord"), method)];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before, "custom plural does not generate {method}");
    }
    for action in ["nil", "false"] {
        let app = source_app(&format!(
            "class WorkflowRecord < ApplicationRecord\n  state_machine :status, action: {action} do; event :finish; end\nend\n"
        ));
        for method in [
            "status_event",
            "status_event=",
            "status_event_transition",
            "status_event_transition=",
        ] {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            let before = diags.clone();
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(diags, before, "action {action} does not generate {method}");
        }
        let mut event = vec![failed(class("WorkflowRecord"), "finish!")];
        attribute_unknown_gems(&mut event, &app);
        assert_note(&event[0], "matches the declared `state_machine` surface");
    }
    for (declaration, method) in [
        (
            "aasm({ 'namespace' => :rev }) do; event :finish; end",
            "finish_rev!",
        ),
        (
            "aasm create_scopes: computed_setting do; state :finish; end",
            "finish",
        ),
        (
            "aasm create_scopes: computed_setting do; state :finish; end",
            "finish?",
        ),
    ] {
        let app = source_app(&format!("class WorkflowRecord\n  {declaration}\nend\n"));
        let mut diags = vec![failed(class("WorkflowRecord"), method)];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before, "unsupported configuration: {declaration}");
    }
}

#[test]
fn transition_guards_and_executable_blocks_do_not_invent_names() {
    for guard in [
        "if_state",
        "unless_state",
        "if_all_states",
        "unless_all_states",
        "if_any_state",
        "unless_any_state",
    ] {
        let app = source_app(&format!(
            "class WorkflowRecord\n  state_machine :status do\n    event :finish do\n      transition open: :closed, {guard}: {{ status: :open }}\n    end\n    after_transition do; state :phantom; event :fake; end\n  end\nend\n"
        ));
        for method in [
            format!("{guard}?"),
            "phantom?".to_string(),
            "fake!".to_string(),
        ] {
            let mut diags = vec![failed(class("WorkflowRecord"), &method)];
            let before = diags.clone();
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(diags, before, "not a declared state/event: {method}");
        }
        for method in ["open?", "closed?", "finish!"] {
            let mut diags = vec![failed(class("WorkflowRecord"), method)];
            attribute_unknown_gems(&mut diags, &app);
            assert_note(&diags[0], "matches the declared `state_machine` surface");
        }
    }
}

#[test]
fn attribution_is_idempotent_and_does_not_change_types_diagnostics_or_emission() {
    let mut app = ingest_app_from_tree(tree(Some(LOCK))).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let before = app.clone();
    let raw = diagnose(&app);
    let emitted = roundhouse::emit::ruby::emit_lowered_models(&app);
    let mut attributed = raw.clone();
    attribute_unknown_gems(&mut attributed, &app);
    assert_eq!(
        dispatch(&raw, "publish!", "Article").severity,
        Severity::Error
    );
    assert_note(
        dispatch(&attributed, "publish!", "Article"),
        "runtime method availability is unverified",
    );
    assert_eq!(
        dispatch(&raw, "publish!", "Article").kind,
        dispatch(&attributed, "publish!", "Article").kind
    );
    let once = attributed.clone();
    attribute_unknown_gems(&mut attributed, &app);
    assert_eq!(attributed, once);
    assert_eq!(
        raw.iter().map(|d| (&d.kind, d.span)).collect::<Vec<_>>(),
        attributed
            .iter()
            .map(|d| (&d.kind, d.span))
            .collect::<Vec<_>>()
    );
    assert_eq!(app, before);
    assert_eq!(diagnose(&app), raw);
    assert_eq!(roundhouse::emit::ruby::emit_lowered_models(&app), emitted);
}

#[test]
#[ignore = "requires pinned native gems listed in tests/gem_generated_surface_probe.rb"]
fn native_ownership_and_configuration_boundaries_match_attribution() {
    use std::collections::BTreeSet;
    use std::io::Write;
    use std::process::{Command, Stdio};

    let cases: &[(&str, &[(&str, bool, bool)])] = &[
        (
            "class Native\n  include AASM\n  aasm do; state :draft; event :finish; end\n  state_machine :status do; event :close; end\nend\n",
            &[
                ("draft", false, false),
                ("draft?", true, true),
                ("finish!", true, true),
                ("with_status", false, false),
                ("status_event", false, false),
                ("close!", true, true),
            ],
        ),
        (
            "module Workflow\n  extend ActiveSupport::Concern\n  included do\n    aasm do; state :draft; end\n    state_machine :status do; event :close; end\n  end\nend\nclass Native < ApplicationRecord; include Workflow; end\n",
            &[
                ("draft", true, true),
                ("with_status", true, true),
                ("status_event", true, true),
                ("close!", true, true),
            ],
        ),
        (
            "module Workflow\n  def self.included(base = nil); end\n  included do; aasm { event :finish }; end\nend\nclass Native < ApplicationRecord; include Workflow; end\n",
            &[("finish!", false, false)],
        ),
        (
            "class Native < ApplicationRecord; end\nmodule Shell; end\nmodule Wrapper\n  class Shell::Holder\n    ::Native.state_machine :status, namespace: :alarm do; end\n  end\nend\nclass Native\n  state_machine :status do; event :finish; end\nend\n",
            &[("finish!", false, false), ("finish_alarm!", true, false)],
        ),
        (
            "class Native < ApplicationRecord\n  aasm do\n    state_machine.config.namespace = :alarm\n    event :finish\n  end\nend\n",
            &[("finish!", false, false), ("finish_alarm!", true, false)],
        ),
        (
            "class Native < ApplicationRecord\n  aasm do; event :finish do; state_machine.config.namespace = :alarm; end; end\nend\n",
            &[("finish!", false, false), ("finish_alarm!", true, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; transition phantom: :closed, on: []; end\nend\n",
            &[("phantom?", false, false), ("closed?", false, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; transition phantom: :closed, on: all; end\nend\n",
            &[("phantom?", false, false), ("closed?", false, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; event({}) do; transition phantom: :closed; end; end\nend\n",
            &[("phantom?", false, false), ("closed?", false, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; transition phantom: :closed, on: :finish; end\nend\n",
            &[
                ("phantom?", true, true),
                ("closed?", true, true),
                ("finish!", true, true),
            ],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; event all do; transition phantom: :closed; end; end\nend\n",
            &[("phantom?", false, false), ("closed?", false, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do\n    event :finish do; transition phantom: :closed; reset; transition open: :closed; end\n  end\nend\n",
            &[("phantom?", false, false), ("open?", true, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; state :open do; machine.instance_variable_set(:@namespace, :alarm); end; event :finish; end\nend\n",
            &[("finish!", false, false), ("finish_alarm!", true, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; event :finish do; transition phantom: :closed; transition open: :closed, if: (reset; :ready?); end; end\nend\n",
            &[("phantom?", false, false), ("open?", true, false)],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :status do; event :finish do; transition phantom: :closed; transition open: :closed, if: -> { reset; :ready? }; end; end\nend\n",
            &[
                ("phantom?", true, true),
                ("open?", true, true),
                ("finish!", true, true),
            ],
        ),
        (
            "class Native < ApplicationRecord\n  state_machine :reviewStatus, attribute: :status do; event :finish; end\nend\n",
            &[
                ("with_review_statuses", false, false),
                ("with_reviewStatus", true, true),
                ("with_reviewStatuses", true, false),
            ],
        ),
        (
            "module FauxSupport\n  module Concern\n    def included(base = nil); end\n  end\nend\nmodule Workflow\n  ActiveSupport = FauxSupport\n  extend ActiveSupport::Concern\n  included do; state_machine :status do; event :finish; end; end\nend\nclass Native < ApplicationRecord; include Workflow; end\n",
            &[("finish!", false, false)],
        ),
        (
            "module FauxSupport\n  module Concern\n    def included(base = nil); end\n  end\nend\nmodule Workflow\n  ActiveSupport = FauxSupport\n  extend ::ActiveSupport::Concern\n  included do; state_machine :status do; event :finish; end; end\nend\nclass Native < ApplicationRecord; include Workflow; end\n",
            &[("finish!", true, true)],
        ),
    ];
    for (source, controls) in cases {
        let mut child = Command::new("ruby")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/gem_generated_surface_probe.rb"
            ))
            .arg("Native")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("pinned native Ruby environment");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{source}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let native: BTreeSet<String> = serde_json::from_slice(&output.stdout).unwrap();
        let app = source_app(source);
        for (method, native_has, attributed) in *controls {
            assert_eq!(
                native.contains(*method),
                *native_has,
                "native {method}: {source}"
            );
            let mut diags = vec![failed(class("Native"), method)];
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(
                diags[0].severity == Severity::Info,
                *attributed,
                "attribution {method}: {source}"
            );
        }
    }
}

#[test]
fn declared_surface_sites_use_existing_ivar_attribution() {
    let mut files = tree(Some(LOCK));
    files.insert(PathBuf::from("app/controllers/articles_controller.rb"), "class ArticlesController < ApplicationController\n  def show\n    @article = Article.find(params[:id])\n    @workflow_result = @article.publish!\n  end\nend\n".as_bytes().to_vec());
    files.insert(
        PathBuf::from("app/views/articles/show.html.erb"),
        b"<%= @workflow_result %>".to_vec(),
    );
    let (diags, _) = check(files, false);
    let ivar = diags.iter().find(|diag| matches!(&diag.kind, DiagnosticKind::IvarUnresolved { name } if name.as_str() == "workflow_result")).expect("unresolved view ivar");
    assert_note(ivar, "@workflow_result is assigned from the `aasm` gem");
}
