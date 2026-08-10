//! A concern's class-side filter macro, run at compile time
//! (`ingest::app::expand_class_body_macros`).
//!
//! `allow_unauthenticated_access` is a method the Authentication concern
//! exports through `class_methods do`, whose body is filter DSL. Ingest
//! binds the call's arguments to the macro's parameters, substitutes,
//! and recognizes the result as Filters.
//!
//! The BARE call — no arguments at all — is the case these tests exist
//! for. Its `**options` parameter has nothing to bind to, and an
//! unbound parameter left the substituted body holding a free variable
//! that `filters_from_macro_body` could not recognize, so the whole
//! macro was dropped. Dropping is the safe direction by design
//! (all-or-nothing: half-expanding an auth macro fails OPEN), but the
//! cost was real — campfire's FirstRunsController kept
//! `require_authentication`, so `/first_run` redirected to
//! `/session/new`, which redirects back to `/first_run`.
//!
//! An unsupplied `**options` means `{}` in Ruby, which makes the skip
//! UNSCOPED — off every action, not off none.

use roundhouse::dialect::{ControllerBodyItem, FilterKind};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

const AUTHENTICATION: &str = r#"
module Authentication
  extend ActiveSupport::Concern

  included do
    before_action :require_authentication
  end

  class_methods do
    def allow_unauthenticated_access(**options)
      skip_before_action :require_authentication, **options
    end
  end

  private
    def require_authentication
      redirect_to "/session/new"
    end
end
"#;

const APPLICATION_CONTROLLER: &str = r#"
class ApplicationController < ActionController::Base
  include Authentication
end
"#;

fn app_with(controller_src: &str) -> App {
    let tree = vec![
        (
            std::path::PathBuf::from("app/controllers/concerns/authentication.rb"),
            AUTHENTICATION.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/controllers/application_controller.rb"),
            APPLICATION_CONTROLLER.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/controllers/things_controller.rb"),
            controller_src.as_bytes().to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

/// Every filter on ThingsController, as `(kind, target, only, except)`.
fn filters(app: &App) -> Vec<(FilterKind, String, Vec<String>, Vec<String>)> {
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("ThingsController ingested");
    c.body
        .iter()
        .filter_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some((
                filter.kind.clone(),
                filter.target.as_str().to_string(),
                filter.only.iter().map(|s| s.as_str().to_string()).collect(),
                filter.except.iter().map(|s| s.as_str().to_string()).collect(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn bare_macro_call_expands_to_an_unscoped_skip() {
    let app = app_with(
        r#"
class ThingsController < ApplicationController
  allow_unauthenticated_access

  def show
  end
end
"#,
    );
    let skips: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .collect();
    assert_eq!(skips.len(), 1, "bare macro should expand to one skip: {skips:?}");
    let (_, target, only, except) = &skips[0];
    assert_eq!(target, "require_authentication");
    assert!(
        only.is_empty() && except.is_empty(),
        "an unsupplied **options means `{{}}` — the skip is UNSCOPED, \
         so it comes off every action: {only:?} / {except:?}"
    );
}

#[test]
fn scoped_macro_call_still_narrows_the_skip() {
    // The shape that already worked, kept honest: a supplied `only:` must
    // NOT be widened to an unscoped skip by the new binding.
    let app = app_with(
        r#"
class ThingsController < ApplicationController
  allow_unauthenticated_access only: %i[new create]

  def new
  end

  def show
  end
end
"#,
    );
    let skips: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .collect();
    assert_eq!(skips.len(), 1, "expected one skip: {skips:?}");
    let (_, target, only, _) = &skips[0];
    assert_eq!(target, "require_authentication");
    assert_eq!(
        only,
        &vec!["new".to_string(), "create".to_string()],
        "a scoped skip must stay scoped — widening it would sign users \
         out of authentication on every action"
    );
}

#[test]
fn a_controller_without_the_macro_keeps_the_filter() {
    let app = app_with(
        r#"
class ThingsController < ApplicationController
  def show
  end
end
"#,
    );
    let skips: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .collect();
    assert!(skips.is_empty(), "no macro call means no skip: {skips:?}");
}
