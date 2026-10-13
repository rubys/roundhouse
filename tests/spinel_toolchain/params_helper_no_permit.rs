//! The Spinel side of rubys/roundhouse#810: a private `*_params` helper
//! with no recognized permit list used to be pinned to
//! `Hash[Symbol, untyped]` in its `.rbs` sidecar by its name alone, and
//! `spin build` refused the mismatch between the guard filter's real
//! (`bool`-shaped) return and that pinned Hash pointer. The CRuby-level
//! sidecar check lives in `tests/params_helper_no_permit_untyped.rs`.
//!
//! This is the Spinel compile+run half, on (close to) the issue's own
//! fixture. It has to go through `spin build` (`native_http::build`),
//! not a lone `spinel contract.rb -o contract` compile: a single-file
//! compile treeshakes differently and did not reproduce the project-
//! wide `-Wint-conversion` hard error `spin build` hits — confirmed by
//! hand against the issue's exact repro before writing this test.

use super::{emit_and_run, native_http};
use roundhouse::project::BuildTarget;

fn widgets() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        // `touch_request` lives on the ANCESTOR (ApplicationController),
        // not on WidgetsController itself. A second `before_action`
        // targeting it is needed so the inliner keeps `check_kind_params`
        // as a method of its own instead of inlining it into `show` (see
        // `own_filter_inlining_is_ordered`), which is how the sidecar's
        // wrong pin on it becomes observable at all.
        .edit(
            "app/controllers/application_controller.rb",
            "  stale_when_importmap_changes\nend\n",
            "  stale_when_importmap_changes\n\n  private\n\n  def touch_request\n    @touched = true\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  resources :widgets, only: [:index, :show]\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  KINDS = %w[small large].freeze

  before_action :check_kind_params, only: :show
  before_action :touch_request

  def index
    head :ok
  end

  def show
    head :ok
  end

  private

  def check_kind_params
    @kind = params.require(:kind)
    head(:not_found) unless KINDS.include?(@kind)
  end
end
"#,
        )
}

/// Before the fix, `native_http::build` panics at `spin build blog`
/// with the issue's own C error: "returning 'sp_bool' {aka 'bool'} from
/// a function with return type 'sp_SymPolyHash *' makes pointer from
/// integer without a cast". After the fix, the helper's sidecar is
/// `untyped` and the project builds; a `kind` in the allow list reaches
/// `show` (200) and one outside it is refused by the guard (404).
#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn a_no_permit_guard_filter_builds_and_runs_on_spinel() {
    let (tree, errors) = widgets().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));

    native_http::build(&tree);
    let server = native_http::Server::start(&tree);

    let allowed = server.get("/widgets/1?kind=small");
    assert_eq!(allowed.status, 200, "allowed kind: {} {}", allowed.status, allowed.body);

    let refused = server.get("/widgets/1?kind=bogus");
    assert_eq!(refused.status, 404, "disallowed kind: {} {}", refused.status, refused.body);
}
