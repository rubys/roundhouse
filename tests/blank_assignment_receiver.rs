//! `(x = expr).present?` — an ASSIGNMENT receiver for a blank predicate.
//!
//! The nilable groundings evaluate the receiver twice
//! (`!x.nil? && !x.empty?`), so `apply_blank_lowering` demands an
//! effect-free reader and otherwise leaves the call as dynamic dispatch
//! with a `blank_unlowered` warning. An assignment fails that test — but
//! it does not need to pass it: the groundings all short-circuit, so the
//! assignment can run once in the FIRST position and later positions can
//! read the name it just bound.
//!
//! lobsters' LoginController does exactly this
//! (`if (rd = session[:redirect_to]).present?`), and leaving it dynamic
//! made `POST /login` raise `undefined method 'present?' for nil` under
//! spinel AOT — no login, so the whole logged-in benchmark workload was
//! unreachable.

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_blank_lowering;

fn controller_body(src: &str) -> String {
    let tree = vec![(
        std::path::PathBuf::from("app/controllers/things_controller.rb"),
        src.as_bytes().to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    apply_blank_lowering(&mut app);
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("controller ingested");
    format!("{:?}", c.body)
}

const WRAP: &str = "class ThingsController < ApplicationController\n  def index\n    %s\n  end\nend\n";

#[test]
fn assignment_receiver_grounds_and_assigns_exactly_once() {
    let body = controller_body(
        &WRAP.replace("%s", "if (rd = session[:redirect_to]).present?\n      redirect_to rd\n    end"),
    );
    // Grounded, not left as a dynamic `present?` send.
    assert!(
        !body.contains("\"present?\""),
        "present? should be grounded:\n{body}"
    );
    // The assignment survives — and appears exactly once, so the session
    // read is not duplicated.
    let assigns = body.matches("redirect_to\"").count();
    assert!(assigns >= 1, "session read should survive:\n{body}");
    assert_eq!(
        body.matches("Assign").count(),
        1,
        "the assignment must be evaluated exactly once:\n{body}"
    );
    // Later occurrences read the bound name.
    assert!(body.contains("\"rd\""), "expected a read of `rd`:\n{body}");
}

#[test]
fn an_effectful_assigned_value_still_refuses() {
    // `(x = save!).present?` is not safe to ground: the VALUE has
    // effects, so the pass must keep the dynamic call rather than
    // reorder the write.
    let body = controller_body(
        &WRAP.replace("%s", "if (x = @thing.save!).present?\n      head :ok\n    end"),
    );
    assert!(
        body.contains("\"present?\""),
        "an effectful assigned value must stay dynamic:\n{body}"
    );
}
