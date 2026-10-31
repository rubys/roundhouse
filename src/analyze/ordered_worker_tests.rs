//! Process isolation keeps worker controls and hash seeds out of other tests.
//! Initial-pass witnesses distinguish barriers from an eventually equal answer.
use super::*;
use std::path::PathBuf;

fn app(files: &[(&str, &str)]) -> App {
    crate::ingest::ingest_app_from_tree(
        files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
            .collect(),
    )
    .unwrap()
}
fn id(s: &str) -> ClassId {
    ClassId(Symbol::from(s))
}
fn sym(s: &str) -> Symbol {
    Symbol::from(s)
}
fn initial(a: &mut Analyzer, app: &mut App) {
    let views = app.views.iter().map(|v| v.name.clone()).collect();
    let includes = app
        .library_classes
        .iter()
        .filter(|c| c.is_module)
        .map(|c| (c.name.clone(), c.includes.clone()))
        .collect();
    let methods = controller_concern_method_snapshot(app, &includes);
    let parents = app
        .controllers
        .iter()
        .filter_map(|c| c.parent.clone().map(|p| (c.name.clone(), Some(p))))
        .collect();
    crate::lower::i18n_translate::with_plan_cache(|| {
        a.run_typing_passes(
            app,
            &Default::default(),
            &views,
            &methods,
            &includes,
            &parents,
            TypingMode::Production { dirty: None },
        )
    });
}
fn snapshot(a: &Analyzer, app: &App) -> Vec<u8> {
    // No semantic normalization: keep union/record spelling, map traversal,
    // diagnostics and the call graph's exact reconstructed insertion history.
    let params: Vec<_> = a.inferred_params.iter().collect();
    let callers: Vec<_> = a.callers_by_target.iter().collect();
    let classes: Vec<_> = a
        .classes
        .iter()
        .map(|(id, c)| (id, &c.instance_methods, &c.class_methods))
        .collect();
    serde_json::to_vec(&(
        classes,
        params,
        callers,
        &app.models,
        &app.controllers,
        &app.library_classes,
        &app.views,
        &app.view_ivar_types,
        &app.controller_resolutions,
        diagnose(app),
        format!("{:?}", a.fixpoint_rounds()),
    ))
    .unwrap()
}

// Macro substitution deliberately retains the factory call's spans.
fn macro_app(padding: usize) -> App {
    let pad = (0..padding).map(|i| format!("v_{i} = {i}\n")).collect::<String>();
    app(&[
        ("lib/translation_factory.rb", "class TranslationFactory; def self.translated(scope); define_method(:label) do; I18n.t(:label, scope: scope, name: 7); end; end; end"),
        ("lib/a_slow.rb", &format!("class ASlow < TranslationFactory; def pad; {pad}nil; end; translated [:bad]; end")),
        ("lib/b_fast.rb", "class BFast < TranslationFactory; translated [:good]; end"),
        ("config/locales/en.yml", "en:\n  bad:\n    label: \"%<name>02d\"\n  good:\n    label: hello\n"),
    ])
}

fn macro_label<'a>(app: &'a App, name: &str) -> &'a crate::dialect::MethodDef {
    app.library_classes.iter().find(|c| c.name == id(name)).unwrap()
        .methods.iter().find(|m| m.name == sym("label")).unwrap()
}

fn macro_call(expr: &Expr) -> &Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } => macro_call(exprs.last().unwrap()),
        ExprNode::Send { method, .. } if method.as_str() == "t" => expr,
        _ => panic!("expected the expanded translation call: {expr:?}"),
    }
}

fn emitted_labels(app: &App) -> Vec<u8> {
    let code = format!("class ASlow; {}; end; class BFast; {}; end; p [ASlow.new.label, BFast.new.label]",
        crate::emit::ruby::emit_method(macro_label(app, "ASlow")),
        crate::emit::ruby::emit_method(macro_label(app, "BFast")));
    let run = std::process::Command::new("ruby").args(["-e", &code]).output().unwrap();
    assert!(run.status.success(), "{code}\n{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(run.stdout, b"[\"7\", \"hello\"]\n");
    run.stdout
}

#[test]
fn worker_child() {
    let Ok(out) = std::env::var("ORDERED_ANALYSIS_TEST_OUT") else {
        return;
    };
    let case = std::env::var("ORDERED_ANALYSIS_TEST_CASE").unwrap();
    let bytes = match case.as_str() {
        "context" => {
            let before = crate::naming::app_inflections();
            let custom = crate::naming::AppInflections {
                irregular: vec![("quux".into(), "quuxen".into())],
                acronym: vec!["API".into()],
                ..Default::default()
            };
            crate::naming::install_app_inflections(custom);
            let mut inputs = vec![(); 24];
            let result = crate::lower::i18n_translate::with_plan_cache(|| {
                parallel::map(&mut inputs, |_| {
                    assert!(crate::lower::i18n_translate::plan_cache_snapshot().is_some());
                    (
                        crate::naming::singularize("quuxen"),
                        crate::naming::camelize("api_client"),
                    )
                })
            });
            assert!(
                result
                    .iter()
                    .all(|r| r == &("quux".into(), "APIClient".into()))
            );
            assert!(crate::lower::i18n_translate::plan_cache_snapshot().is_none());
            crate::naming::install_app_inflections(before);
            serde_json::to_vec(&result).unwrap()
        }
        "models" => {
            let mut app = app(&[
                (
                    "app/models/a_origin.rb",
                    "class AOrigin < ActiveRecord::Base; def self.value; 7; end; end",
                ),
                (
                    "app/models/z_dependent.rb",
                    "class ZDependent < ActiveRecord::Base; def self.value; AOrigin.value; end; end",
                ),
                (
                    "app/models/a_origin/extra.rb",
                    "class AOrigin; def extra; 'extra'; end; end",
                ),
            ]);
            let mut a = Analyzer::new(&app);
            a.classes
                .get_mut(&id("AOrigin"))
                .unwrap()
                .class_methods
                .insert(sym("value"), Ty::Str);
            initial(&mut a, &mut app);
            let dep = app
                .models
                .iter()
                .find(|m| m.name == id("ZDependent"))
                .unwrap();
            assert_eq!(
                dep.methods()
                    .find(|m| m.name == sym("value"))
                    .unwrap()
                    .body
                    .ty,
                Some(Ty::Int),
                "main publishes the preceding model before typing the dependent"
            );
            assert_eq!(
                a.classes[&id("AOrigin")].class_methods[&sym("value")],
                Ty::Int,
                "the preceding model publishes its new return in place"
            );
            snapshot(&a, &app)
        }
        "controllers" => {
            let mut app = app(&[
                (
                    "config/routes.rb",
                    "Rails.application.routes.draw do; get '/c', to: 'child#index'; end",
                ),
                (
                    "app/controllers/parent_controller.rb",
                    "class ParentController < ActionController::Base; before_action :seed; def seed; @ready = 'yes'; end; end",
                ),
                (
                    "app/controllers/parent_controller/extra.rb",
                    "class ParentController; def extra; @extra = {b: 'x', a: 1}; end; end",
                ),
                (
                    "app/controllers/parent_controller/namespace.rb",
                    "class ParentController; module Namespace; end; end",
                ),
                (
                    "app/controllers/child_controller.rb",
                    "class ChildController < ParentController; def index; @seen = @ready.length; @shape = {a: 1, b: 'x'}; end; def other; @shape = {b: 'x', a: 1}; render 'child/index'; end; end",
                ),
                (
                    "app/views/child/index.html.erb",
                    "<%= @seen %><%= @shape %>",
                ),
                (
                    "app/views/layouts/application.html.erb",
                    "<%= @shape %><%= yield %>",
                ),
            ]);
            app.controllers
                .sort_by_key(|c| c.name != id("ChildController"));
            let mut a = Analyzer::new(&app);
            initial(&mut a, &mut app);
            let seeds = a.view_seeds.as_ref().unwrap();
            assert_eq!(
                seeds.action_ivars_by_view[&sym("child/index")][&sym("seen")],
                Ty::Int,
                "Phase B must wait for the parent's complete phase-A metadata"
            );
            assert!(
                a.controller_action_meta_cache[&id("ParentController")]
                    .0
                    .contains_key(&sym("extra"))
            );
            let extra =
                serde_json::to_vec(&(&seeds.action_ivars_by_view, &seeds.layout_ivars_by_view))
                    .unwrap();
            let mut bytes = snapshot(&a, &app);
            bytes.extend(extra);
            bytes
        }
        "mailers" => {
            let mut app = app(&[
                (
                    "app/models/trigger.rb",
                    "class Trigger < ActiveRecord::Base; def send_it; ProbeMailer.with(value: 1).ping; end; end",
                ),
                (
                    "lib/early.rb",
                    "class Early; def read; ProbeMailer.new.params; end; end",
                ),
                (
                    "app/mailers/probe_mailer.rb",
                    "class ProbeMailer < ActionMailer::Base; def ping; @value = params[:value]; mail(to: 'x@y'); end; end",
                ),
                (
                    "lib/late.rb",
                    "class Late; def read; ProbeMailer.new.params; end; end",
                ),
            ]);
            app.library_classes
                .sort_by_key(|c| match c.name.0.as_str() {
                    "Early" => 0,
                    "ProbeMailer" => 1,
                    "Late" => 2,
                    _ => 3,
                });
            let mut a = Analyzer::new(&app);
            a.classes
                .get_mut(&id("ProbeMailer"))
                .unwrap()
                .instance_methods
                .insert(sym("params"), Ty::Str);
            initial(&mut a, &mut app);
            let result = |name| {
                app.library_classes
                    .iter()
                    .find(|c| c.name == id(name))
                    .unwrap()
                    .methods
                    .iter()
                    .find(|m| m.name == sym("read"))
                    .unwrap()
                    .body
                    .ty
                    .clone()
            };
            assert_eq!(
                result("Early"),
                Some(Ty::Str),
                "eager parameter publication changes earlier readers"
            );
            assert!(
                matches!(result("Late"), Some(Ty::Record { .. })),
                "late publication changes later readers"
            );
            snapshot(&a, &app)
        }
        "i18n" => {
            let mut results = Vec::new();
            // Same source spans/key, different app catalogs. A process-global
            // plan cache would make the second app inherit the first's type.
            for (yaml, ty) in [
                ("en:\n  probe:\n    greet: hello\n", Ty::Str),
                ("en:\n  probe:\n    greet: 7\n", Ty::Int),
            ] {
                let mut app = app(&[
                    ("config/locales/en.yml", yaml),
                    (
                        "app/models/alpha.rb",
                        "class Alpha < ActiveRecord::Base; def self.word; I18n.t('probe.greet'); end; end",
                    ),
                    (
                        "app/models/beta.rb",
                        "class Beta < ActiveRecord::Base; def self.word; I18n.t('probe.greet'); end; end",
                    ),
                ]);
                let mut a = Analyzer::new(&app);
                a.analyze(&mut app);
                assert_eq!(a.classes[&id("Alpha")].class_methods[&sym("word")], ty);
                assert_eq!(a.classes[&id("Beta")].class_methods[&sym("word")], ty);
                results.push(snapshot(&a, &app));
            }
            serde_json::to_vec(&results).unwrap()
        }
        "i18n_macro_order" => {
            let mut app = macro_app(0);
            let spans = |name| {
                let call = macro_call(&macro_label(&app, name).body);
                let ExprNode::Send { recv, args, .. } = &*call.node else { unreachable!() };
                recv.iter().map(|r| r.span).chain(args.iter().map(|a| a.span)).collect::<Vec<_>>()
            };
            assert_eq!(spans("ASlow"), spans("BFast"), "the test must exercise cloned spans");
            // Good first works even in serial. With workers the condition
            // variable forces the same cache population order, without sleeps.
            app.library_classes.sort_by_key(|c| c.name != id("BFast"));
            let ready = (std::sync::Mutex::new(false), std::sync::Condvar::new());
            let catalog = app.i18n.clone();
            let answers = crate::lower::i18n_translate::with_plan_cache(|| {
                parallel::map(&mut app.library_classes, |class| {
                    let method = class.methods.iter().find(|m| m.name == sym("label"))?;
                    let good = class.name == id("BFast");
                    if !good {
                        let (lock, wake) = &ready;
                        let mut complete = lock.lock().unwrap();
                        while !*complete { complete = wake.wait(complete).unwrap(); }
                    }
                    let call = macro_call(&method.body);
                    let ExprNode::Send { recv, method, args, block, .. } = &*call.node else { unreachable!() };
                    let result = crate::lower::i18n_translate::resolves(&catalog, recv.as_ref(), method.as_str(), args, block.as_ref(), crate::lower::i18n_translate::Site::Other).unwrap();
                    if good {
                        *ready.0.lock().unwrap() = true;
                        ready.1.notify_all();
                        assert!(result.is_ok());
                    } else {
                        assert!(result.as_ref().is_err_and(|e| e.contains("formatted placeholder")), "a good cached plan hid the refusal: {result:?}");
                    }
                    Some((class.name.clone(), result))
                })
            });
            // A subsequent lowering has its own catalog/cache. This also
            // checks the emitted values for two supported, distinct scopes.
            app.i18n.entries.insert("bad.label".into(), "%{name}".into());
            crate::lower::i18n_translate::apply_i18n_translate_lowering(&mut app);
            let values = emitted_labels(&app);
            serde_json::to_vec(&(answers, values)).unwrap()
        }
        "i18n_constant_catalog" => {
            let mut results = Vec::new();
            for with_constant in [true, false] {
                let make_app = || {
                    let constant = if with_constant { "BOOT = I18n.t(\"probe.label\", name: 7);" } else { "" };
                    app(&[
                        ("lib/probe.rb", &format!("class Probe; {constant} def label; I18n.t(\"probe.label\", name: 7); end; end")),
                        ("lib/other.rb", "class Other; def label; \"other\"; end; end"),
                        ("config/locales/en.yml", "en:\n  probe:\n    label: \"%<name>02d\"\n"),
                    ])
                };
                let mut source = make_app();
                Analyzer::new(&source).analyze(&mut source);
                let call = macro_call(&macro_label(&source, "Probe").body);
                let diagnostic = crate::expr::ExprCold::diagnostic(&call.cold);
                assert!(matches!(diagnostic,
                    Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. })
                        if construct.as_str() == "I18n" && detail.contains("formatted placeholder")),
                    "constant/body catalog mismatch, BOOT={with_constant}: {diagnostic:?}");
                let source_diags = diagnose(&source);
                assert!(source_diags.iter().any(|d| d.message.contains("formatted placeholder")));
                let mut lowered = make_app();
                let mut emit_diags = crate::session::analyze_and_lower(&mut lowered);
                emit_diags.extend(diagnose(&lowered));
                assert!(emit_diags.iter().any(|d| d.message.contains("formatted placeholder")),
                    "emit must refuse the formatted translation, BOOT={with_constant}");
                results.push((with_constant, source_diags, emit_diags));
            }
            serde_json::to_vec(&results).unwrap()
        }
        "i18n_macro_v4" => {
            let mut app = macro_app(500);
            let mut analyzer = Analyzer::new(&app);
            analyzer.analyze(&mut app);
            let diagnostics = diagnose(&app);
            let refusals: Vec<_> = diagnostics.iter().filter(|d| d.message.contains("formatted placeholder")).collect();
            assert_eq!(refusals.len(), 1, "{diagnostics:?}");
            assert!(refusals[0].message.contains("%<name>02d"));
            // Correcting the older serial collision makes the good method
            // clean too. The malformed call alone remains refused.
            serde_json::to_vec(&diagnostics).unwrap()
        }
        _ => panic!("unknown case"),
    };
    std::fs::write(out, bytes).unwrap();
}

fn compare_case(case: &str) {
    compare_case_workers(case, &[1, 2, 4, 8]);
}

fn compare_case_workers(case: &str, workers: &[usize]) {
    let root = std::env::temp_dir().join(format!("roundhouse-ordered-{case}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut expected = None;
    for &workers in workers {
        let out = root.join(workers.to_string());
        let run = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "analyze::ordered_worker_tests::worker_child", "--nocapture"])
            .env("ORDERED_ANALYSIS_TEST_OUT", &out)
            .env("ORDERED_ANALYSIS_TEST_CASE", case)
            .env("ROUNDHOUSE_WORKERS", workers.to_string())
            .env_remove("ROUNDHOUSE_VISIT_ORDER")
            .env_remove("ROUNDHOUSE_TIMINGS")
            .output().unwrap();
        assert!(run.status.success(), "{case}, {workers}: {}{}",
            String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
        let bytes = std::fs::read(out).unwrap();
        if let Some(expected) = &expected {
            assert_eq!(&bytes, expected, "{case}, {workers}");
        } else { expected = Some(bytes); }
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn worker_inflections_and_scoped_plan_context() {
    compare_case("context");
}
#[test]
fn model_publication_keeps_the_sequential_main_schedule() {
    compare_case("models");
}
#[test]
fn complete_parent_metadata_and_ordered_view_events() {
    compare_case("controllers");
}
#[test]
fn mailer_parameters_publish_at_the_original_item() {
    compare_case("mailers");
}
#[test]
fn i18n_plans_are_shared_within_one_app_only() {
    compare_case("i18n");
}

#[test]
fn copied_macro_spans_do_not_share_distinct_translation_plans() {
    compare_case_workers("i18n_macro_order", &[1, 2, 8]);
}
#[test]
fn macro_v4_keeps_the_formatted_translation_refused() {
    compare_case_workers("i18n_macro_v4", &[1, 2, 8]);
}

#[test]
fn constants_and_methods_share_the_app_translation_catalog() {
    compare_case_workers("i18n_constant_catalog", &[1, 2, 8]);
}
