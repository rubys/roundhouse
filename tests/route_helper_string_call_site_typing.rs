//! A route segment's type comes from the CALL SITE when the name-based
//! rule would guess wrong.
//!
//! `param_ty` reads the segment's NAME and answers Int for `id` /
//! `<x>_id`. That is a claim about the value, and a name cannot support
//! it: campfire writes `resources :qr_code` — no `QrCode` model behind
//! it anywhere — and calls `qr_code_path(Base64.urlsafe_encode64(url))`.
//! The generated signature said `(Integer id)` against a String, and on
//! a strict target that is not a warning: spinel refuses the build with
//! "a seed is trusted, so the emitted code would reinterpret the value
//! rather than convert it."
//!
//! Answering it by NAME the other way — no model behind the route ⇒
//! String — was tried and is wrong in both directions: a route's name
//! flattens what a model's name nests (`Rooms::OpensController` ⇒
//! `rooms_open`, model `Rooms::Open`), and an `id` routinely belongs to
//! a model the route never names (campfire's `account_bot_path` carries
//! a User id). Both misses retype a segment whose call sites pass
//! `record.id`, breaking the emit in the direction the change was meant
//! to fix. Measured: 20 signatures moved, 19 of them wrong.
//!
//! So the evidence is the call site, read off the analyzer's type. A
//! helper nobody calls keeps the name-based default — which is what the
//! second half of this test pins.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_routes_to_library_functions;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :articles\n  \
    resources :qr_code, only: :show\nend\n";

// One helper module, two calls: the qr code's `:id` is a Base64 blob,
// the article's is a primary key.
const HELPER: &str = r#"
module LinkHelper
  def zoom_qr_code(url)
    token = Base64.urlsafe_encode64(url)
    qr_code_path(token)
  end

  def article_link(article)
    article_path(article.id)
  end
end
"#;

fn helper_params(name: &str) -> Vec<roundhouse::ty::Param> {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/helpers/link_helper.rb", HELPER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    // The survey reads `Expr::ty`, which only the analyzer populates —
    // lowering the routes off a raw ingest would silently find nothing
    // and the test would pass for the wrong reason.
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let sig = lower_routes_to_library_functions(&app)
        .into_iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| panic!("helper {name} not generated"))
        .signature
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("helper signature is not Ty::Fn") };
    params
}

#[test]
fn string_call_site_retypes_an_id_segment() {
    let params = helper_params("qr_code_path");
    assert_eq!(params[0].name.as_str(), "id");
    assert!(
        matches!(params[0].ty, Ty::Str),
        "an `id` segment a call site fills with a String must type Str, got {:?}",
        params[0].ty
    );
}

#[test]
fn an_id_call_site_keeps_the_integer_default() {
    let params = helper_params("article_path");
    assert_eq!(params[0].name.as_str(), "id");
    assert!(
        matches!(params[0].ty, Ty::Int),
        "a model-backed `id` segment must stay Int, got {:?}",
        params[0].ty
    );
}
