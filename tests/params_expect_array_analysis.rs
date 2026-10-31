use std::collections::HashMap;
use std::path::PathBuf;

fn source_errors(body: &str) -> Vec<String> {
    let app = analyzed_app(body, "", "");
    roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .map(|d| d.to_string())
        .collect()
}

fn analyzed_app(body: &str, own_params: &str, inherited_params: &str) -> roundhouse::App {
    let controller = format!(
        "class AnswersController < ApplicationController\n{own_params}  def create\n{body}\n  end\nend\n"
    );
    let parent = format!(
        "class ApplicationController < ActionController::Base\n{inherited_params}end\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            parent.as_str(),
        ),
        ("app/controllers/answers_controller.rb", controller.as_str()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :answers, only: :create\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn array_expect_is_an_array_before_controller_lowering() {
    for receiver in ["params", "@params"] {
        let initialization = if receiver == "@params" {
            "    @params = params\n"
        } else {
            ""
        };
        let errors = source_errors(&format!(
            "{initialization}    answers = {receiver}.expect(answers: [[:header, :other, {{ labels: [] }}]])\n    render json: answers.map(&:to_h)",
        ));
        assert!(errors.is_empty(), "{receiver}: {errors:?}");
    }
}

#[test]
fn self_params_does_not_gain_the_params_array_contract() {
    use roundhouse::ty::Ty;
    fn expect_type(expr: &roundhouse::expr::Expr) -> Option<roundhouse::ty::Ty> {
        if matches!(&*expr.node, roundhouse::expr::ExprNode::Send { method, .. } if method.as_str() == "expect") {
            return expr.ty.clone();
        }
        let mut result = None;
        expr.node.for_each_child(&mut |child| {
            if result.is_none() {
                result = expect_type(child);
            }
        });
        result
    }
    let definition = "  def params\n    {\"answers\" => [{\"header\" => \"filtered\"}]}\n  end\n";
    for (own, inherited) in [("", ""), (definition, ""), ("", definition)] {
        let app = analyzed_app(
            "    answers = self.params.expect(answers: [[:header]])\n    render json: answers.map(&:to_h)",
            own,
            inherited,
        );
        let action = app.controllers.iter()
            .flat_map(|controller| controller.actions())
            .find(|action| action.name.as_str() == "create")
            .expect("create action");
        let ty = expect_type(&action.body).expect("expect call type");
        let expected = if own.is_empty() && inherited.is_empty() {
            Ty::Str
        } else {
            Ty::Array { elem: Ty::Hash { key: Ty::Str.into(), value: Ty::Str.into() }.into() }
        };
        assert_eq!(ty, expected);
    }
}

#[test]
fn unsupported_nested_filters_keep_the_source_error() {
    let errors = source_errors(
        "    answers = params.expect(answers: [[:header, { records: [[:name]] }]])\n    render json: answers.map(&:to_h)",
    );
    assert!(
        errors.iter().any(|error| error.contains("map")),
        "{errors:?}"
    );
}

#[test]
fn a_params_alias_does_not_gain_the_params_array_contract() {
    let errors = source_errors(
        "    p = params\n    answers = p.expect(answers: [[:header]])\n    render json: answers.map(&:to_h)",
    );
    assert!(
        errors.iter().any(|error| error.contains("map")),
        "{errors:?}"
    );
}

#[test]
fn an_ordinary_hash_does_not_gain_the_params_array_contract() {
    let errors = source_errors(
        "    answers = { value: \"scalar\" }.expect(answers: [[:header]])\n    render json: answers.map(&:to_h)",
    );
    assert!(
        errors.iter().any(|error| error.contains("map")),
        "{errors:?}"
    );
}
