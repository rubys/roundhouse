//! A hop a LOWERING synthesizes must carry its own type.
//!
//! Three passes rewrite one call into a CHAIN, and each of them typed
//! only the outermost link. The hop underneath was built with a bare
//! `Expr::new` — `ty: None` — and no analyzer ever runs again to fill
//! it in, so `analyze::diagnose`'s rule ("receiver typed, result not")
//! fired on a send the pass had just written itself:
//!
//! * `lower::exists_conditions` — `Ban.exists?(ip_address: ip)` becomes
//!   `Ban.where(…).exists?`. The outer `exists?` carried `Bool`; the
//!   `where` under it carried nothing, and the ledger read `no known
//!   method where on Class(Ban)`, naming the one method every model has.
//! * `lower::config_reader` — `Rails.configuration.x.vapid.public_key`
//!   anchors on a synthesized `Rails.application` hop, which the stdlib
//!   registry types `Untyped` when the source writes it out longhand.
//! * `lower::including` — `xs.including(y)` becomes `xs.to_a + [y]`,
//!   and `to_a` is identity on the Array receiver it was handed.
//!
//! All three are the same rule as `lower::params_merge`'s `.to_attrs`
//! and `lower::attached`'s synthesized reader: a name the pipeline
//! writes must be a name the analyzer's world contains.
//!
//! The gates run the LOWERING and then diagnose, because that is the
//! only order in which the defect exists — analyze alone never sees
//! these nodes.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::{
    apply_config_reader_lowering, apply_exists_conditions_lowering, apply_including_lowering,
};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :bans do |t|\n    t.string :ip_address\n  end\n  \
    create_table :users do |t|\n    t.string :name\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :users\nend\n";

/// Ingest → analyze → the named lowerings → diagnose, which is the
/// order the emit runs them in and the only one in which a synthesized
/// hop can be reported at all.
fn errors(tree: Vec<(&str, &str)>) -> Vec<String> {
    let tree = tree
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    apply_including_lowering(&mut app);
    apply_exists_conditions_lowering(&mut app);
    apply_config_reader_lowering(&mut app);
    diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error"))
        .collect()
}

#[test]
fn exists_conditions_types_the_where_it_synthesizes() {
    let found = errors(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        (
            "app/models/ban.rb",
            "class Ban < ApplicationRecord\n  \
               def self.banned?(ip_address)\n    \
                 exists?(ip_address: ip_address)\n  \
               end\nend\n",
        ),
    ]);
    let offenders: Vec<&String> =
        found.iter().filter(|d| d.contains("`where`")).collect();
    assert!(
        offenders.is_empty(),
        "the `where` hop is the pass's own work; it must carry a Relation type: {offenders:?}"
    );
}

#[test]
fn config_reader_types_the_application_hop_it_synthesizes() {
    let found = errors(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        (
            // The lift only runs when `config/application.rb` exists —
            // it is the file the Application reopen is built from, and
            // the initializers are read beside it.
            "config/application.rb",
            "module Sample\n  class Application < Rails::Application\n  end\nend\n",
        ),
        (
            "config/initializers/vapid.rb",
            "Rails.application.configure do\n  \
               config.x.vapid.public_key = \"pk\"\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
               def vapid_key\n    \
                 Rails.configuration.x.vapid.public_key\n  \
               end\nend\n",
        ),
    ]);
    let offenders: Vec<&String> =
        found.iter().filter(|d| d.contains("`application`")).collect();
    assert!(
        offenders.is_empty(),
        "`Rails.application` is Untyped in the registry; the synthesized hop must say so too: {offenders:?}"
    );
}

#[test]
fn including_types_the_to_a_it_synthesizes() {
    let found = errors(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
               def peer_ids\n    \
                 ids = [ 1, 2 ]\n    \
                 ids.including(id)\n  \
               end\nend\n",
        ),
    ]);
    let offenders: Vec<&String> = found.iter().filter(|d| d.contains("`to_a`")).collect();
    assert!(
        offenders.is_empty(),
        "`to_a` on an Array receiver is identity; the synthesized hop must carry it: {offenders:?}"
    );
}

/// The other half of the `including` rule, and the reason the stamp is
/// conditional: a receiver with no arm that can answer `to_a` is one
/// the pass cannot claim `to_a` for, and stamping an Array over it
/// would launder a receiver the emit genuinely cannot lower into a
/// clean ledger entry.
///
/// The fixture used to be campfire's
/// `params.fetch(:user_ids, []).including(…)`. That shape is no longer
/// an example of the rule: `fetch` now reads the `[]` default, so the
/// receiver is `Array[…] | Str` and the Array arm answers — see
/// `tests/params_fetch_array_default.rs`. A bare `params[:user_ids]`
/// still types `Str | Nil`, which is what "no arm can answer" looks
/// like, so the invariant keeps a fixture that actually exercises it.
#[test]
fn including_leaves_an_unknown_receiver_on_the_ledger() {
    let found = errors(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        (
            "app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  \
               def index\n    \
                 @ids = params[:user_ids].including(1)\n  \
               end\nend\n",
        ),
    ]);
    assert!(
        found.iter().any(|d| d.contains("`to_a`")),
        "an untypable receiver must stay reported, not be laundered: {found:?}"
    );
}
