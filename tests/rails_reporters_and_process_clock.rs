//! Registry-only reporters and undefined application tracers are not executable support.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn ingest_index(body: &str) -> roundhouse::App {
    let controller = format!(
        "class XController < ApplicationController\n  def index\n{body}\n    head :ok\n  end\nend\n"
    );
    let files: [(&str, &str); 4] = [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/x_controller.rb", &controller),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/x\", to: \"x#index\"\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn diagnostics_for(body: &str) -> Vec<String> {
    let mut app = ingest_index(body);
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect()
}

fn assign_ty(app: &roundhouse::App, name: &str) -> Ty {
    let ctrl = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "XController")
        .expect("XController");
    let action = ctrl
        .actions()
        .find(|a| a.name.as_str() == "index")
        .expect("index");
    let ExprNode::Seq { exprs } = &*action.body.node else {
        panic!("expected Seq body");
    };
    for e in exprs {
        if let ExprNode::Assign {
            target: roundhouse::expr::LValue::Var { name: n, .. },
            value,
        } = &*e.node
        {
            if n.as_str() == name {
                return value
                    .ty
                    .clone()
                    .unwrap_or_else(|| panic!("no ty on {name}"));
            }
        }
    }
    panic!("no assignment to {name}");
}

#[test]
fn unimplemented_reporters_and_tracers_are_refused() {
    let diags = diagnostics_for(
        r#"    Rails.event.notify("thing", message: "m")
    Rails.event.tagged(a: 1) { Rails.event.debug("d", message: "x") }
    Rails.error.report(StandardError.new("x"), handled: true)
    Rails.error.handle(StandardError) { 1 }
    Rails.error.set_context(a: 1)
    ShopifyTracer.in_span("x") { |s| 1 }
    1"#,
    );
    for name in ["event", "error", "ShopifyTracer"] {
        assert!(diags.iter().any(|d| d.contains(name)), "missing {name}: {diags:?}");
    }
    // Process.pid / clock_gettime / CLOCK_MONOTONIC are registered stdlib
    // (Campfire tip web-push + video previewer). They must not linger as
    // residuals beside the still-unsupported reporters above.
    assert!(
        diags.iter().all(|d| !d.contains("Process")),
        "Process should resolve: {diags:?}"
    );
}

#[test]
fn process_clock_and_timeout_consts_resolve() {
    let diags = diagnostics_for(
        r#"    t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    ms = Process.clock_gettime(Process::CLOCK_MONOTONIC, :millisecond)
    fm = Process.clock_gettime(Process::CLOCK_MONOTONIC, :float_millisecond)
    n = Process.pid
    begin
      Timeout.timeout(0.01) { sleep 1 }
    rescue Timeout::Error
      n = n + 1
    end
    IO.popen(["true"], in: IO::NULL, err: IO::NULL) { }
    begin
      raise SystemCallError, "x"
    rescue SystemCallError, OpenSSL::SSL::SSLError, Vips::Error
      n = n + 1
    end
    [t + 1.0, ms + 1, fm + 1.0, n]"#,
    );
    for name in [
        "Process", "Timeout", "Timeout::Error", "IO", "IO::NULL",
        "SystemCallError", "OpenSSL::SSL::SSLError", "Vips::Error",
    ] {
        assert!(
            diags.iter().all(|d| !d.contains(name)),
            "{name} should resolve: {diags:?}"
        );
    }
}

#[test]
fn process_clock_gettime_unit_narrows() {
    // Catalog must not register a fixed Float for clock_gettime — the
    // send special-case owns unit narrowing (`:millisecond` → Int).
    let mut app = ingest_index(
        r#"    t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    ms = Process.clock_gettime(Process::CLOCK_MONOTONIC, :millisecond)
    fm = Process.clock_gettime(Process::CLOCK_MONOTONIC, :float_millisecond)
    n = Process.pid
    [t, ms, fm, n]"#,
    );
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    assert_eq!(assign_ty(&app, "t"), Ty::Float);
    assert_eq!(assign_ty(&app, "ms"), Ty::Int);
    assert_eq!(assign_ty(&app, "fm"), Ty::Float);
    assert_eq!(assign_ty(&app, "n"), Ty::Int);
}
