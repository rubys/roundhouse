//! graphql-ruby object types (`ingest::graphql_ruby`, `analyze::graphql`).
//!
//! howtographql's `check` used to report nothing under `app/graphql`:
//! library class bodies are outside `diagnose`, and a type's `object`
//! had no type. The ingest pass gives each field the method graphql-ruby
//! would resolve it through, so inference carries the record class from
//! the schema's root down, and `check` reports a field with nothing
//! behind it and a `null: false` field that can be nil.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::dialect::GraphqlResolution;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name, null: false\n    t.string :nickname\n  end\n  \
    create_table :posts do |t|\n    t.string :title, null: false\n    \
    t.integer :user_id, null: false\n    t.integer :editor_id\n    t.integer :board_id, null: false\n  end\n  \
    create_table :boards do |t|\n    t.string :name, null: false\n  end\n  \
    add_foreign_key :posts, :users\n  add_foreign_key :posts, :users, column: :editor_id\nend\n";

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  post '/graphql', to: 'graphql#execute'\nend\n",
    ),
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    ("app/models/user.rb", "class User < ApplicationRecord\n  has_many :posts\nend\n"),
    (
        "app/models/post.rb",
        "class Post < ApplicationRecord\n  belongs_to :user\n  \
         belongs_to :editor, class_name: \"User\", optional: true\n  belongs_to :board\nend\n",
    ),
    ("app/models/board.rb", "class Board < ApplicationRecord\nend\n"),
    (
        "app/controllers/graphql_controller.rb",
        "class GraphqlController < ActionController::Base\n  def execute\n    \
         render json: AppSchema.execute(params[:query])\n  end\nend\n",
    ),
    (
        "app/graphql/app_schema.rb",
        "class AppSchema < GraphQL::Schema\n  query Types::QueryType\nend\n",
    ),
    (
        "app/graphql/types/base_object.rb",
        "module Types\n  class BaseObject < GraphQL::Schema::Object\n  end\nend\n",
    ),
    (
        "app/graphql/types/query_type.rb",
        "module Types\n  class QueryType < BaseObject\n    \
         field :posts, [PostType], null: false\n\n    def posts\n      Post.all\n    end\n  end\nend\n",
    ),
];

fn post_type(fields: &str) -> String {
    format!("module Types\n  class PostType < BaseObject\n{fields}  end\nend\n")
}

const USER_TYPE: &str = "module Types\n  class UserType < BaseObject\n    \
    field :name, String, null: false\n    field :posts, [PostType], null: false\n  end\nend\n";

fn analyzed(extra: &[(&str, &str)]) -> App {
    let mut files: HashMap<&str, &str> = BASE.iter().copied().collect();
    files.insert("app/graphql/types/user_type.rb", USER_TYPE);
    files.extend(extra.iter().copied());
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    // As `check` does: analyze, no lowering.
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Diagnostics anchored in `app/graphql`, as `(code, message)`.
fn graphql_diagnostics(app: &App) -> Vec<(String, String)> {
    diagnose(app)
        .into_iter()
        .filter(|d| in_graphql(app, d))
        .map(|d| (d.code().to_owned(), d.message.clone()))
        .collect()
}

fn in_graphql(app: &App, d: &Diagnostic) -> bool {
    let index = d.span.file.0 as usize;
    index > 0 && app.sources[index - 1].path.contains("app/graphql/")
}

fn nullable_fields(app: &App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::GraphqlNullableField { field, .. } => {
                assert_eq!(d.severity, Severity::Warning, "{d}");
                Some(field.as_str().to_owned())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_field_backed_by_a_not_null_column_or_association_is_clean() {
    let post = post_type(
        "    field :title, String, null: false\n    \
         field :author, UserType, null: false, method: :user\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// The record class reaches each type from the root, cycles included:
/// `PostType` wraps a `Post`, `UserType` a `User`.
#[test]
fn the_object_type_flows_from_the_root() {
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let object = |class: &str| {
        let lc = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == class)
            .unwrap();
        let m = lc
            .methods
            .iter()
            .find(|m| m.name.as_str() == "object")
            .unwrap();
        format!("{:?}", m.signature)
    };
    assert!(
        object("Types::PostType").contains("\"Post\""),
        "{}",
        object("Types::PostType")
    );
    assert!(
        object("Types::UserType").contains("\"User\""),
        "{}",
        object("Types::UserType")
    );
}

#[test]
fn a_non_null_field_on_a_nullable_column_warns() {
    let user = "module Types\n  class UserType < BaseObject\n    \
        field :nickname, String, null: false\n    field :posts, [PostType], null: false\n  end\nend\n";
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", user),
    ]);
    assert_eq!(nullable_fields(&app), ["nickname"]);
    // Declared nullable: nothing to report.
    let user = user.replace(
        "null: false\n    field :posts",
        "null: true\n    field :posts",
    );
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", &user),
    ]);
    assert!(nullable_fields(&app).is_empty());
}

/// A `belongs_to` is non-nil for a stored row only when the database
/// says so: NOT NULL and a foreign key. `editor` is optional and
/// nullable; `board` is NOT NULL with no foreign key.
#[test]
fn a_belongs_to_is_proven_only_by_not_null_and_a_foreign_key() {
    let post = post_type(
        "    field :author, UserType, null: false, method: :user\n    \
         field :editor, UserType, null: false\n    field :board_name, String, null: false\n\n    \
         def board_name\n      object.board&.name\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(nullable_fields(&app), ["editor", "board_name"]);
}

/// graphql-ruby's "Failed to implement": neither the type nor the
/// object has the method. Reported at the `field` call.
#[test]
fn a_field_with_nothing_behind_it_is_a_dispatch_failure() {
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "send_dispatch_failed");
    assert!(found[0].1.contains("`headline` on Post"), "{found:?}");
}

/// The bodies of the type's own methods are checked like a controller's.
#[test]
fn a_type_method_body_is_checked() {
    let post = post_type(
        "    field :score, Integer, null: false\n\n    def score\n      object.title + 1\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let codes: Vec<String> = graphql_diagnostics(&app)
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(codes, ["incompatible_binop"]);
}

/// search_object's `scope { … }` is what a resolver class answers.
#[test]
fn a_search_object_resolver_carries_its_scope() {
    let query = "module Types\n  class QueryType < BaseObject\n    \
        field :posts, resolver: Resolvers::PostsSearch\n  end\nend\n";
    let resolver = "module Resolvers\n  class PostsSearch < GraphQL::Schema::Resolver\n    \
        scope { Post.all }\n    type [Types::PostType]\n  end\nend\n";
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/types/query_type.rb", query),
        ("app/graphql/resolvers/posts_search.rb", resolver),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("`headline` on Post"), "{found:?}");
}

/// No `argument` declared: graphql-ruby calls the method with no
/// keywords, so an optional one takes its default. `length` is nil
/// here, and `length[:x]` raises on every request for the field.
#[test]
fn an_undeclared_optional_parameter_takes_its_default() {
    let post = post_type(
        "    field :excerpt, String, null: false\n\n    \
         def excerpt(length: nil)\n      length[:x]\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("`[]` on nil"), "{found:?}");
}

/// A type nothing constructs has no known object, so nothing is claimed.
#[test]
fn an_unreachable_type_reports_nothing() {
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let orphan = "module Types\n  class OrphanType < BaseObject\n    \
        field :anything, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/orphan_type.rb", orphan),
    ]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// The synthesized methods are for the analyzer: lowering removes them.
#[test]
fn lowering_removes_the_synthesized_methods() {
    let post = post_type(
        "    field :title, String, null: false\n\n    def shout\n      object.title.upcase\n    end\n",
    );
    let mut app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    roundhouse::session::analyze_and_lower(&mut app);
    let lc = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Types::PostType")
        .unwrap();
    let names: Vec<&str> = lc.methods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["shout"]);
}

/// A method an included app module defines is the type's own.
#[test]
fn a_method_from_an_included_app_module_resolves_the_field() {
    let concern = "module PostFields\n  def headline\n    object.title.upcase\n  end\nend\n";
    let post = "module Types\n  class PostType < BaseObject\n    include PostFields\n\n    \
        field :headline, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/post_fields.rb", concern),
        ("app/graphql/types/post_type.rb", post),
    ]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// A module computed at load time (`include Resolvers.for(:post)`), or
/// one the app does not define, may hold the method: a field with none
/// in sight is skipped, not reported as missing on the record.
#[test]
fn an_include_out_of_sight_makes_unanswered_fields_skipped() {
    for include in [
        "include Resolvers.for(:post)",
        "include SomeGem::PostFields",
    ] {
        let post = format!(
            "module Types\n  class PostType < BaseObject\n    {include}\n\n    \
             field :headline, String, null: false\n    field :title, String, null: false\n  end\nend\n"
        );
        let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
        assert!(
            graphql_diagnostics(&app).is_empty(),
            "{include}: {:?}",
            graphql_diagnostics(&app)
        );
        let post_type = app
            .graphql_types
            .iter()
            .find(|t| t.class.0.as_str() == "Types::PostType");
        let resolution = |name: &str| {
            post_type
                .unwrap()
                .fields
                .iter()
                .find(|f| f.name.as_str() == name)
                .unwrap()
                .resolution
                .clone()
        };
        assert!(
            matches!(resolution("headline"), GraphqlResolution::Skipped { .. }),
            "{include}"
        );
        assert!(
            matches!(resolution("title"), GraphqlResolution::Skipped { .. }),
            "{include}"
        );
    }
}

/// The summary `check` prints: a clean run's denominator, and the
/// reasons the rest was not followed.
#[test]
fn coverage_counts_checked_unreached_arguments_and_skipped() {
    let post = post_type(
        "    field :title, String, null: false\n    \
         field :excerpt, String, null: false\n\n    def excerpt(length: nil)\n      object.title\n    end\n    \
         field :tags, String, null: false, hash_key: :tags\n",
    );
    let orphan = "module Types\n  class OrphanType < BaseObject\n    \
        field :anything, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/orphan_type.rb", orphan),
    ]);
    let coverage = roundhouse::analyze::graphql::coverage(&app).expect("graphql coverage");
    // Checked: QueryType.posts, PostType.title and .excerpt (called as
    // graphql-ruby would, with no keywords). Nothing here leads to
    // UserType (no author field) or OrphanType: their 3 are unreached.
    assert_eq!(
        coverage.summary(),
        "graphql: 4 object type(s), 7 field(s): 3 checked, 3 on types nothing reaches, \
         0 take arguments, 1 skipped (hash lookup 1)"
    );
}

fn codes(app: &App) -> Vec<String> {
    graphql_diagnostics(app)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

/// graphql-ruby passes a field's `argument`s as keywords: the method
/// is called with them, typed as declared, and its body is checked.
#[test]
fn field_arguments_type_the_method_parameters() {
    let post = post_type(
        "    field :excerpt, String, null: false do\n      argument :length, Integer\n    end\n\n    \
         def excerpt(length:)\n      object.title[0, length] + length\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(
        codes(&app),
        ["incompatible_binop"],
        "{:?}",
        graphql_diagnostics(&app)
    );
    let coverage = roundhouse::analyze::graphql::coverage(&app).unwrap();
    assert_eq!(coverage.arguments, 0, "{}", coverage.summary());
}

/// An optional keyword (`length: nil`) is flattened at ingest; its slot
/// is filled in order with the declared argument.
#[test]
fn an_optional_argument_fills_a_flattened_keyword() {
    let post = post_type(
        "    field :excerpt, String, null: false do\n      argument :length, Integer, required: false\n      \
         argument :suffix, String, required: true\n    end\n\n    \
         def excerpt(length: nil, suffix: \"...\")\n      suffix + 1\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(
        codes(&app),
        ["incompatible_binop"],
        "{:?}",
        graphql_diagnostics(&app)
    );
    assert!(
        graphql_diagnostics(&app)[0].1.contains("String + Integer"),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// A parameter no argument fills (graphql-ruby would raise) is not
/// modeled: recorded, not called, not checked.
#[test]
fn a_parameter_no_argument_fills_is_recorded_not_guessed() {
    let post = post_type(
        "    field :excerpt, String, null: false\n\n    def excerpt(length:)\n      length[:x]\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
    let coverage = roundhouse::analyze::graphql::coverage(&app).unwrap();
    assert_eq!(coverage.arguments, 1, "{}", coverage.summary());
}

/// An input object argument is an instance of its class, read by
/// method or by key; an enum argument is its value's name, a String.
#[test]
fn input_object_and_enum_arguments() {
    let input = "module Types\n  class PostFilter < GraphQL::Schema::InputObject\n    \
        argument :query, String\n  end\nend\n";
    let order = "module Types\n  class PostOrder < GraphQL::Schema::Enum\n    \
        value \"NEWEST\"\n    value \"OLDEST\"\n  end\nend\n";
    let query = "module Types\n  class QueryType < BaseObject\n    \
        field :posts, [PostType], null: false do\n      argument :filter, PostFilter\n      \
        argument :order, PostOrder\n    end\n\n    \
        def posts(filter:, order:)\n      filter[:query] + 1\n      filter.query + 2\n      order + 3\n      Post.all\n    end\n  end\nend\n";
    let post = post_type("    field :title, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/types/post_filter.rb", input),
        ("app/graphql/types/post_order.rb", order),
        ("app/graphql/types/query_type.rb", query),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(
        found
            .iter()
            .all(|(c, m)| c == "incompatible_binop" && m.contains("String + Integer")),
        "{found:?}"
    );
}

/// A mutation's class-body `argument`s are its `resolve` keywords; the
/// record it returns types the payload type.
#[test]
fn a_mutation_resolves_with_its_arguments() {
    let schema = "class AppSchema < GraphQL::Schema\n  query Types::QueryType\n  \
        mutation Types::MutationType\nend\n";
    let mutation_type = "module Types\n  class MutationType < BaseObject\n    \
        field :create_post, mutation: Mutations::CreatePost\n  end\nend\n";
    let create = "module Mutations\n  class CreatePost < GraphQL::Schema::Mutation\n    \
        argument :title, String\n    type Types::PostType\n\n    \
        def resolve(title:)\n      title + 1\n      Post.create!(title: title)\n    end\n  end\nend\n";
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/app_schema.rb", schema),
        ("app/graphql/types/mutation_type.rb", mutation_type),
        ("app/graphql/mutations/create_post.rb", create),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    // `title` arrives typed as declared, and the payload type is
    // reached through the mutation's return.
    assert!(
        found
            .iter()
            .any(|(c, m)| c == "incompatible_binop" && m.contains("String + Integer")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|(_, m)| m.contains("`headline` on Post")),
        "{found:?}"
    );
}

/// The argument and reader signatures are for the analyzer: lowering
/// removes them with the synthesized methods.
#[test]
fn lowering_removes_the_argument_signatures() {
    let post = post_type(
        "    field :excerpt, String, null: false do\n      argument :length, Integer\n    end\n\n    \
         def excerpt(length:)\n      object.title\n    end\n",
    );
    let mut app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(!app.graphql_signatures.is_empty());
    roundhouse::session::analyze_and_lower(&mut app);
    let leftover: Vec<_> = app
        .rbs_signatures
        .values()
        .flat_map(|t| t.keys())
        .filter(|k| k.as_str().starts_with("__gql_"))
        .collect();
    assert!(leftover.is_empty(), "{leftover:?}");
}
