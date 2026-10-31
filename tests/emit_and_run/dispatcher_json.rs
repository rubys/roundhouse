use super::{dispatcher_json_contract as contract, emit_and_run};

fn observations(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON response observation"))
        .collect()
}

#[test]
fn callback_and_rescue_json_responses_run() {
    let run = contract::app().run_ruby(contract::PROGRAM);
    run.assert_passes();
    assert_eq!(observations(&run.stdout), observations(contract::EXPECTED));
}

#[test]
fn callback_and_rescue_json_responses_match_rails() {
    let helper =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let mut script = format!(
        "require {:?}\nexit 77 unless RailsOracle.available?(\"actionpack\")\n\
         RailsOracle.activate(\"actionpack\")\nrequire \"action_controller\"\n\
         require \"action_controller/api\"\nrequire \"rack/mock\"\n",
        helper.to_str().unwrap(),
    );
    for (path, source) in contract::SOURCES {
        if path.starts_with("app/") {
            script.push_str(source);
        }
    }
    script.push_str(contract::PROGRAM);
    let oracle = emit_and_run::ruby()
        .args(["-e", &script])
        .output()
        .expect("Rails oracle");
    if oracle.status.code() == Some(77) {
        eprintln!("skipping dispatcher JSON oracle: actionpack for RH_RAILS_ORACLE is unavailable");
        return;
    }
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let expected = String::from_utf8(oracle.stdout).unwrap();
    assert_eq!(observations(&expected), observations(contract::EXPECTED));
    let run = contract::app().run_ruby(contract::PROGRAM);
    run.assert_passes();
    assert_eq!(observations(&run.stdout), observations(&expected));
}

fn review_fixture(name: &str, oracle: bool) {
    use std::{fs, path::Path};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("tests/support/dispatcher_cases").join(name);
    let program = fs::read_to_string(source.join("program.rb")).unwrap();
    let controllers = fs::read_to_string(source.join("controllers.txt")).unwrap();
    let mut script = String::new();
    let mut expected = observations(&fs::read_to_string(source.join("rails.jsonl")).unwrap());
    if oracle {
        script.push_str(&format!(
            "require {:?}\nexit 77 unless RailsOracle.available?(\"actionpack\")\n\
             RailsOracle.activate(\"actionpack\")\nrequire \"action_controller\"\n\
             require \"action_controller/api\"\nrequire \"rack/mock\"\n\
             ActionController::Base.view_paths = [{:?}]\n",
            root.join("tests/support/rails_oracle.rb"), source.join("app/views"),
        ));
        for path in controllers.lines() {
            script.push_str(&format!("load {:?}\n", source.join(path)));
        }
        script.push_str(&program);
        let output = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
        if output.status.code() == Some(77) {
            eprintln!("skipping dispatcher {name} oracle: RailsOracle actionpack is unavailable");
            return;
        }
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let actual = observations(&String::from_utf8(output.stdout).unwrap());
        assert_eq!(actual, expected, "Rails fixture observations changed");
        expected = actual;
    }
    fn files(path: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { files(&path, out); } else { out.push(path); }
        }
    }
    let mut paths = Vec::new();
    for subtree in ["app", "config", "db"] { files(&source.join(subtree), &mut paths); }
    paths.sort();
    let mut app = emit_and_run::empty_app();
    for path in paths {
        app = app.write(path.strip_prefix(&source).unwrap().to_str().unwrap(),
                        &fs::read_to_string(&path).unwrap());
    }
    script.clear();
    for path in controllers.lines().filter(|p| !p.contains("/concerns/")) {
        script.push_str(&format!("require File.expand_path({path:?}, Dir.pwd)\n"));
    }
    script.push_str(&program);
    let run = app.run_ruby(&script);
    run.assert_passes();
    let actual = observations(&run.stdout);
    assert_eq!(actual.len(), expected.len());
    // The review fixtures retain five pre-existing differences from Rails:
    // status-only rendering (API and HTML), redirect body, the implicit
    // HTML after callback, and the child's deferred super response.
    // Keep their complete baseline observations explicit; never count them
    // as Rails parity or silently omit an observation from the comparison.
    // The extra named-layout control also stays unsupported: only the
    // application layout is emitted, so preserve its original residue.
    let gaps: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(source.join("baseline-gaps.json")).unwrap()).unwrap();
    for (actual, rails) in actual.iter().zip(&expected) {
        let case = actual["case"].as_str().unwrap();
        if let Some(baseline) = gaps.get(case) {
            assert_eq!(actual, baseline, "baseline gap changed: {case}; Rails: {rails}");
            eprintln!("known baseline gap {case}; Rails: {rails}; emitted: {actual}");
        } else {
            assert_eq!(actual, rails, "dispatcher response: {case}");
        }
    }
}

#[test]
fn dispatcher_review_responses_match_rails() { review_fixture("cases", true); }

#[test]
fn dispatcher_deferred_action_tail_matches_rails() { review_fixture("tail-cases", true); }

#[test]
fn dispatcher_callback_template_matches_rails() { review_fixture("template-cases", true); }

#[test]
fn dispatcher_deferred_action_tail_runs() { review_fixture("tail-cases", false); }

#[test]
fn dispatcher_callback_template_runs() { review_fixture("template-cases", false); }

#[test]
fn dispatcher_render_classes_match_rails() { review_fixture("render-classes", true); }
