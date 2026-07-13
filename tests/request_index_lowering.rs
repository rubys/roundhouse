//! `request[<key>]` → `params[<key>]` grounding
//! (`lower::apply_request_index_lowering`).
//!
//! Rails' `Request#[]` is pure params delegation (rack:
//! `params[key.to_s]`; the CRuby overlay Request is the same
//! two-liner), so the indexed read rewrites to the params machinery —
//! typed on every target — while every other `request.<member>` stays
//! untouched (they belong to the pending two-layer Request split).
//! Surfaced by lobsters' RSS-token filter: `request[:format] == "rss"`
//! typed unknown on the spinel tree (no Request object there) and AOT
//! refused the equality.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_request_index_lowering;

fn controller_body_debug(src: &str) -> String {
    let files: Vec<(&str, &str)> = vec![("app/controllers/home_controller.rb", src)];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    apply_request_index_lowering(&mut app);
    let controller = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "HomeController")
        .expect("HomeController ingested");
    controller
        .body
        .iter()
        .filter_map(|item| match item {
            roundhouse::dialect::ControllerBodyItem::Action { action, .. } => {
                Some(format!("{:?}", action.body))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn request_index_rewrites_to_params_and_other_members_survive() {
    let body = controller_body_debug(
        r#"class HomeController < ApplicationController
  def index
    if request[:format] == "rss"
      @rss = true
    end
    @ua = request.env["HTTP_USER_AGENT"]
  end
end
"#,
    );
    // The indexed read now goes through params, key untouched.
    assert!(
        body.contains("params"),
        "request[:format] must ground to a params read:\n{body}"
    );
    assert!(
        body.contains("format"),
        "the key must survive the receiver swap:\n{body}"
    );
    // Non-indexed request members are NOT this pass's business.
    assert!(
        body.contains("request"),
        "request.env must stay verbatim (pending Request split):\n{body}"
    );
    assert!(
        body.contains("env"),
        "request.env must stay verbatim (pending Request split):\n{body}"
    );
}
