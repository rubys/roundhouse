//! Native HTTP coverage for Rails-shaped JSON request parameters. The
//! expected JSON tree follows Rails 8.1.4's ActionDispatch parser with
//! its default `perform_deep_munge = true` setting, which removes nil
//! array members recursively while retaining nil Hash values.

use super::{emit_and_run, native_http};

/// Builds a real-blog probe route; `wrapping` controls ParamsWrapper.
fn probe_app(wrapping: bool) -> emit_and_run::Overlay {
    let app = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "\n  private\n",
            "\n  def request_probe\n    render plain: JSON.generate({ \"body\" => request.request_parameters, \"params\" => params }), content_type: \"application/json\"\n  end\n\n  private\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  post \"/__json_params/:id\", to: \"articles#request_probe\"\n",
        );
    if wrapping {
        app
    } else {
        app.edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  wrap_parameters false\n",
        )
    }
}

/// Checks the native response and decodes its JSON, including server logs
/// in diagnostics when status or response decoding fails.
fn decoded(response: &native_http::Response, log: &str) -> serde_json::Value {
    assert_eq!(response.status, 200, "{}\n{log}", response.body);
    serde_json::from_str(&response.body)
        .unwrap_or_else(|error| panic!("invalid JSON response: {error}: {}\n{log}", response.body))
}

/// Rails' JSON request parser preserves JSON values in
/// `request.request_parameters`; with the default deep-munge setting,
/// nil Hash values survive while nil array members are dropped. Query
/// keys merge over the body, then route captures merge over both. The
/// body-only request hash stays independent of those query/path merges.
/// Rack::MethodOverride reads urlencoded/multipart form bodies, not
/// `application/json`, so JSON `_method` remains a body parameter and
/// the POST route still runs.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn json_body_values_and_source_precedence_match_rails_natively() {
    const JSON: &str = "application/json";
    let (tree, errors) = probe_app(false).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");

    let body = r#"{"title":"body-title","body":"body-content","collision":"body","id":"body","body_only":"kept","_method":"DELETE","integer":17,"decimal":2.5,"enabled":true,"disabled":false,"nullable":null,"empty_object":{},"empty_array":[],"nested":{"object":{"empty":{}},"array":[1,false,null,{"value":2.5},[],{}]},"array":["x",2,true,null,{},[]]}"#;
    let response = server.post(
        "/__json_params/path?collision=query&id=query&query_only=query",
        JSON,
        body,
    );
    let result = decoded(&response, &server.log());
    let body_params = &result["body"];
    assert_eq!(body_params["collision"], "body");
    assert_eq!(body_params["id"], "body");
    assert_eq!(body_params["_method"], "DELETE");
    assert_eq!(body_params["title"], "body-title");
    assert_eq!(body_params["body"], "body-content");
    assert_eq!(body_params["empty_array"], serde_json::json!([]));
    assert_eq!(body_params["integer"], 17);
    assert_eq!(body_params["decimal"], 2.5);
    assert_eq!(body_params["enabled"], true);
    assert_eq!(body_params["disabled"], false);
    assert!(
        body_params.get("nullable").is_some(),
        "JSON null key was dropped: {body_params}"
    );
    assert!(body_params["nullable"].is_null());
    assert_eq!(body_params["empty_object"], serde_json::json!({}));
    assert_eq!(
        body_params["nested"],
        serde_json::json!({"object": {"empty": {}}, "array": [1, false, {"value": 2.5}, [], {}]})
    );
    assert_eq!(
        body_params["array"],
        serde_json::json!(["x", 2, true, {}, []])
    );
    assert_eq!(body_params["body_only"], "kept");
    assert!(body_params.get("query_only").is_none());
    assert!(body_params.get("controller").is_none());

    let merged = &result["params"];
    assert_eq!(merged["collision"], "query");
    assert_eq!(merged["id"], "path");
    assert_eq!(merged["query_only"], "query");
    assert_eq!(merged["body_only"], "kept");
    for key in [
        "integer",
        "decimal",
        "enabled",
        "disabled",
        "nullable",
        "empty_object",
        "empty_array",
        "nested",
        "array",
    ] {
        assert_eq!(merged[key], body_params[key], "{key} changed during merge");
    }
    assert!(
        merged.get("nullable").is_some(),
        "JSON null key was dropped: {merged}"
    );
    assert!(merged["nullable"].is_null());

    // ActionDispatch wraps a non-Hash JSON root under `_json`.
    let top_level = server.post("/__json_params/root", JSON, r#"[1,false,null,{"x":[]},{}]"#);
    let top_level = decoded(&top_level, &server.log());
    assert_eq!(
        top_level["body"]["_json"],
        serde_json::json!([1, false, {"x": []}, {}])
    );

    // URL-encoded fields remain strings, with the existing form/query/path
    // precedence and body-only request_parameters behavior.
    let form = server.post(
        "/__json_params/path?collision=query&id=query&query_only=query",
        "application/x-www-form-urlencoded",
        "collision=form&id=form&form_only=form",
    );
    let form = decoded(&form, &server.log());
    assert_eq!(form["body"]["collision"], "form");
    assert_eq!(form["body"]["id"], "form");
    assert!(form["body"].get("query_only").is_none());
    assert_eq!(form["params"]["collision"], "query");
    assert_eq!(form["params"]["id"], "path");
    assert_eq!(form["params"]["query_only"], "query");

    // Multipart text fields remain strings and use the same merge rules.
    let boundary = "json-request-boundary";
    let multipart_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"collision\"\r\n\r\nform-data\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"id\"\r\n\r\nform-data\r\n--{boundary}--\r\n"
    );
    let multipart = server.post(
        "/__json_params/path?collision=query&id=query&query_only=query",
        &format!("multipart/form-data; boundary={boundary}"),
        &multipart_body,
    );
    let multipart = decoded(&multipart, &server.log());
    assert_eq!(multipart["body"]["collision"], "form-data");
    assert_eq!(multipart["body"]["id"], "form-data");
    assert!(multipart["body"].get("query_only").is_none());
    assert_eq!(multipart["params"]["collision"], "query");
    assert_eq!(multipart["params"]["id"], "path");
    assert_eq!(multipart["params"]["query_only"], "query");
}

/// ParamsWrapper's controller-params projection reads the body-only
/// source: the wrapped article contains body-sourced model fields, not
/// colliding query or path values. This does not assert that Roundhouse
/// mutates `request.request_parameters` when wrapping; Rails does, while
/// the existing `Params.wrap` runtime currently only updates controller
/// params, a separate fidelity gap from JSON value parsing.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn params_wrapper_uses_body_values_when_query_and_path_collide() {
    let (tree, errors) = probe_app(true).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");

    let response = server.post(
        "/__json_params/path?title=query-title&body=query-body&id=query-id&query_only=query-only",
        "application/json",
        r#"{"title":"body-title","body":"body-content","id":"body-id"}"#,
    );
    let result = decoded(&response, &server.log());
    assert_eq!(
        result["params"]["article"],
        serde_json::json!({
            "title": "body-title",
            "body": "body-content",
            "id": "body-id"
        })
    );
    assert_eq!(result["params"]["title"], "query-title");
    assert_eq!(result["params"]["body"], "query-body");
    assert_eq!(result["params"]["id"], "path");
    assert_eq!(result["params"]["query_only"], "query-only");
    assert_eq!(result["body"]["title"], "body-title");
    assert_eq!(result["body"]["body"], "body-content");
    assert_eq!(result["body"]["id"], "body-id");
    assert!(result["body"].get("query_only").is_none());
}
