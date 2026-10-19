//! A concern that lives in the includer's namespace, included by its
//! relative name, and the ivars it writes.
//!
//! Shopify's customer-authentication controllers keep their mixins
//! beside the controllers:
//!
//! ```ruby
//! module Auth
//!   module Concerns
//!     module ValidateRedirectUri
//!       def validate_redirect_uri!
//!         @redirect_uri = T.let(params[:redirect_uri], T.nilable(String))
//!       end
//!     end
//!   end
//!
//!   class SessionsController < SectionController
//!     include Concerns::ValidateRedirectUri   # Auth::Concerns::…
//!   end
//! end
//! ```
//!
//! Ruby resolves `Concerns::ValidateRedirectUri` through the lexical
//! nesting (`Auth::SessionsController`, then `Auth`, then top level).
//! The include was kept as written, matched no ingested module, and
//! every ivar the concern assigns read `@redirect_uri has no known
//! type` in the controller.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

fn ivar_errors(files: &[(&str, &str)]) -> Vec<String> {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains("ivar_unresolved"))
        .collect()
}

const APPLICATION: (&str, &str) = (
    "app/controllers/application_controller.rb",
    "class ApplicationController < ActionController::Base\nend\n",
);
const ROUTES: (&str, &str) = (
    "config/routes.rb",
    "Rails.application.routes.draw do\n  get 'new' => 'auth/sessions#new'\nend\n",
);
const SECTION: (&str, &str) = (
    "app/controllers/auth/section_controller.rb",
    "module Auth\n  class SectionController < ApplicationController\n  end\nend\n",
);

const VALIDATE: (&str, &str) = (
    "app/controllers/auth/concerns/validate_redirect_uri.rb",
    r#"
module Auth
  module Concerns
    module ValidateRedirectUri
      extend T::Helpers
      requires_ancestor { SectionController }

      def validate_redirect_uri!
        @redirect_uri = T.let(params[:redirect_uri], T.nilable(String))
      end
    end
  end
end
"#,
);

const REQUIRE_CLIENT: (&str, &str) = (
    "app/controllers/auth/concerns/require_client.rb",
    r#"
module Auth
  module Concerns
    module RequireClient
      #: (untyped client_id) -> void
      def set_client_id!(client_id)
        @client_id = client_id #: String?
      end
    end
  end
end
"#,
);

#[test]
fn ivars_written_by_a_relatively_included_concern_are_typed() {
    let o = ivar_errors(&[
        APPLICATION,
        ROUTES,
        SECTION,
        VALIDATE,
        REQUIRE_CLIENT,
        (
            "app/controllers/auth/sessions_controller.rb",
            r#"
module Auth
  class SessionsController < SectionController
    include Concerns::ValidateRedirectUri
    include Concerns::RequireClient

    before_action :validate_redirect_uri!, only: [:new]

    def new
      redirect_to(@redirect_uri)
      render plain: @client_id
    end
  end
end
"#,
        ),
    ]);
    assert!(o.is_empty(), "{o:#?}");
}

/// A module's own `include` resolves the same way: `include Concerns::…`
/// from a sibling namespace's module is `Auth::Concerns::…`, so the
/// ivars reach the controller through the module chain.
#[test]
fn a_concern_including_a_sibling_concern_relatively_carries_its_ivars() {
    let o = ivar_errors(&[
        APPLICATION,
        ROUTES,
        SECTION,
        VALIDATE,
        (
            "app/controllers/auth/concerns/bundle.rb",
            "module Auth\n  module Concerns\n    module Bundle\n      include ValidateRedirectUri\n\n      def bundled; end\n    end\n  end\nend\n",
        ),
        (
            "app/controllers/auth/sessions_controller.rb",
            r#"
module Auth
  class SessionsController < SectionController
    include Concerns::Bundle

    def new
      validate_redirect_uri!
      redirect_to(@redirect_uri)
    end
  end
end
"#,
        ),
    ]);
    assert!(o.is_empty(), "{o:#?}");
}

/// A module the controller never includes does not seed ivars, and a
/// module reached only through another included module does.
#[test]
fn transitive_concern_ivars_are_seeded_and_an_unincluded_module_is_not() {
    let o = ivar_errors(&[
        APPLICATION,
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get 'show' => 'things#show'\nend\n",
        ),
        (
            "app/controllers/concerns/extra.rb",
            "module Extra\n  def set_extra\n    @from_extra = 'e'\n  end\nend\n",
        ),
        (
            "app/controllers/concerns/bundle.rb",
            "module Bundle\n  include Extra\n  def set_bundle\n    @from_bundle = 'b'\n  end\nend\n",
        ),
        (
            "app/controllers/concerns/orphan.rb",
            "module Orphan\n  def set_orphan\n    @from_orphan = 'o'\n  end\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            r#"
class ThingsController < ApplicationController
  include Bundle

  def show
    set_extra
    render plain: @from_extra
    render plain: @from_orphan
  end
end
"#,
        ),
    ]);
    assert!(
        !o.iter().any(|d| d.contains("from_extra")),
        "included concern chain should type @from_extra; got {o:#?}"
    );
    assert!(
        o.iter().any(|d| d.contains("from_orphan")),
        "an unincluded module should not seed @from_orphan; got {o:#?}"
    );
}
