//! Declaration lookup for association build and find.
//!
//! The controller rewrite and the association seed both read
//! `AssocRegistry`. These tests pin that shared answer: the later
//! declaration, a typed owner that does not declare the name, and a
//! name that is not a direct `has_many` on every declarer.

/// The association registry over models ingested from `sources`, as
/// the controller lowering builds it.
fn registry(schema: &str, sources: &[(&str, &str)]) -> roundhouse::lower::scope_chain::AssocRegistry {
    let schema = roundhouse::ingest::ingest_schema(schema.as_bytes(), "db/schema.rb").expect("ingest schema");
    let models: Vec<_> = sources
        .iter()
        .map(|(src, path)| {
            roundhouse::ingest::ingest_model(src.as_bytes(), path, &schema, &Default::default())
                .expect("ingest model")
                .expect("model recognized")
        })
        .collect();
    roundhouse::lower::scope_chain::build_assoc_registry(&models)
}

/// An owner expression: `"@story"` is an ivar of no analyzed type,
/// `"@parent: Article"` one analyzed as Article, and `"@owner: Tag |
/// Article"` one analyzed as that union.
fn owner(written: &str) -> roundhouse::expr::Expr {
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::ty::Ty;

    let (name, ty) = match written.split_once(": ") {
        Some((name, ty)) => (name, Some(ty)),
        None => (written, None),
    };
    let mut owner = Expr::new(
        roundhouse::span::Span::synthetic(),
        ExprNode::Ivar { name: Symbol::from(name.trim_start_matches('@')) },
    );
    owner.ty = ty.map(|ty| {
        let mut arms: Vec<Ty> =
            ty.split(" | ").map(|c| Ty::Class { id: ClassId(Symbol::from(c)), args: vec![].into() }).collect();
        if arms.len() == 1 { arms.remove(0) } else { Ty::Union { variants: arms.into() } }
    });
    owner
}

/// What `<owner>.<assoc>.<call>` builds or finds through.
fn resolve(
    reg: &roundhouse::lower::scope_chain::AssocRegistry,
    written: &str,
    assoc: &str,
    call: &str,
) -> roundhouse::lower::scope_chain::AssocBuildTarget {
    use roundhouse::ident::Symbol;
    reg.build_target(&owner(written), &Symbol::from(assoc), &Symbol::from(call))
}

/// What `<owner>.comments.<call>` builds or finds through.
fn comments(
    reg: &roundhouse::lower::scope_chain::AssocRegistry,
    written: &str,
    call: &str,
) -> roundhouse::lower::scope_chain::AssocBuildTarget {
    resolve(reg, written, "comments", call)
}

/// A `Comment` built or found under `foreign_key`.
fn comment_by(foreign_key: &str) -> roundhouse::lower::scope_chain::AssocBuildTarget {
    use roundhouse::ident::{ClassId, Symbol};
    roundhouse::lower::scope_chain::AssocBuildTarget::Declared {
        target: ClassId(Symbol::from("Comment")),
        foreign_key: Symbol::from(foreign_key),
    }
}

use roundhouse::lower::scope_chain::AssocBuildTarget::{Declined, Undeclared};

/// The parent's class picks between declarations, whatever its name. A
/// subclass finds the declaration it inherits.
#[test]
fn the_declaration_is_resolved_by_the_parent_class_not_the_ivar_name() {
    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "type"
  end
  create_table "posts", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "post_id"
  end
end
"#,
        &[
            ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
            ("class Post < ApplicationRecord\n  has_many :comments\nend\n", "app/models/post.rb"),
            ("class Story < Article\nend\n", "app/models/story.rb"),
        ],
    );
    assert_eq!(comments(&reg, "@post: Article", "build"), comment_by("article_id"));
    assert_eq!(comments(&reg, "@article: Post", "build"), comment_by("post_id"));
    assert_eq!(comments(&reg, "@parent: Story", "find"), comment_by("article_id"));
    assert_eq!(comments(&reg, "@parent: Story | Article", "build"), comment_by("article_id"));
    assert_eq!(comments(&reg, "@parent: Article | Post", "build"), Declined);
    assert_eq!(comments(&reg, "@parent", "build"), Declined);
}

/// A parent of no known model resolves by its name when the name is a
/// model that (itself or through a superclass) declares the association,
/// as the association seed resolves it, so a typed-params build and a
/// kwargs build on the same parent agree. A name that is no model, or a
/// model without the association, leaves only the declarers' agreement.
#[test]
fn an_untyped_parent_resolves_by_its_name_like_the_seed() {
    use roundhouse::ident::Symbol;
    use roundhouse::lower::scope_chain::assoc_read_target;

    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "type"
  end
  create_table "users", force: :cascade do |t|
  end
  create_table "tags", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "user_id"
  end
end
"#,
        &[
            ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
            ("class Story < Article\nend\n", "app/models/story.rb"),
            ("class User < ApplicationRecord\n  has_many :comments\nend\n", "app/models/user.rb"),
            ("class Tag < ApplicationRecord\nend\n", "app/models/tag.rb"),
        ],
    );
    let seed = |written: &str| {
        assoc_read_target(&owner(written), &Symbol::from("comments"), &reg)
            .map(|(model, _, fk)| (model.map(|m| m.0.as_str().to_string()), fk.as_str().to_string()))
    };
    assert_eq!(comments(&reg, "@user", "build"), comment_by("user_id"));
    assert_eq!(seed("@user"), Some((Some("User".to_string()), "user_id".to_string())));
    assert_eq!(comments(&reg, "@story", "build"), comment_by("article_id"));
    assert_eq!(seed("@story"), Some((Some("Article".to_string()), "article_id".to_string())));
    for written in ["@parent", "@tag"] {
        assert_eq!(comments(&reg, written, "build"), Declined, "{written}");
        assert_eq!(seed(written), None, "{written}");
    }
}

/// A parent typed as a model whose chain does not declare the name has
/// no such association, though another model declares it: `@tag.comments`
/// is not built under `article_id`. Only a parent of no known model
/// takes the declaration every declarer agrees on.
#[test]
fn a_model_without_the_declaration_does_not_take_another_models() {
    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "tags", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
  end
end
"#,
        &[
            ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
            ("class Tag < ApplicationRecord\nend\n", "app/models/tag.rb"),
        ],
    );
    assert_eq!(comments(&reg, "@tag: Tag", "build"), Declined);
    assert_eq!(comments(&reg, "@tag: Tag", "find"), Declined);
    assert_eq!(comments(&reg, "@owner: Tag | Article", "build"), Declined);
    for written in ["@parent", "@parent: Current", "@tag"] {
        assert_eq!(comments(&reg, written, "build"), comment_by("article_id"), "{written}");
    }
}

/// A name no model declares (an association from a `with_options` block
/// or a custom macro, which the registry does not see) is `Undeclared`,
/// for the rewrite's name-derived form, even on a parent typed as a
/// model. One another model declares stays declined for a model that
/// does not (above).
#[test]
fn a_name_no_model_declares_is_undeclared_on_any_parent() {
    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
  end
end
"#,
        &[("class Article < ApplicationRecord\nend\n", "app/models/article.rb")],
    );
    for written in ["@article: Article", "@article", "@owner: Article | Article"] {
        for call in ["build", "find", "find_by"] {
            assert_eq!(comments(&reg, written, call), Undeclared, "{written}.comments.{call}");
        }
    }
}

/// A union arm whose chain declares no `comments` has no such reader,
/// so a parent named after the arm that declares it is that arm, for
/// the rewrite and the seed alike (campfire's `@room` typed `Membership
/// | Room`). The name does not choose between arms declaring it
/// differently, nor name a model outside the union.
#[test]
fn a_union_typed_parent_named_after_its_declaring_arm_reads_that_arm() {
    use roundhouse::ident::Symbol;
    use roundhouse::lower::scope_chain::assoc_read_target;

    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "posts", force: :cascade do |t|
  end
  create_table "tags", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "post_id"
  end
end
"#,
        &[
            ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
            ("class Post < ApplicationRecord\n  has_many :comments\nend\n", "app/models/post.rb"),
            ("class Tag < ApplicationRecord\nend\n", "app/models/tag.rb"),
        ],
    );
    let seed = |written: &str| {
        assoc_read_target(&owner(written), &Symbol::from("comments"), &reg)
            .map(|(model, _, fk)| (model.map(|m| m.0.as_str().to_string()), fk.as_str().to_string()))
    };
    assert_eq!(comments(&reg, "@article: Tag | Article", "build"), comment_by("article_id"));
    assert_eq!(comments(&reg, "@article: Tag | Article", "find"), comment_by("article_id"));
    assert_eq!(seed("@article: Tag | Article"), Some((Some("Article".to_string()), "article_id".to_string())));
    for written in ["@tag: Tag | Article", "@post: Tag | Article", "@article: Article | Post"] {
        assert_eq!(comments(&reg, written, "build"), Declined, "{written}");
        assert_eq!(seed(written), None, "{written}");
    }
}

/// A model declaring `comments` twice keeps the later declaration, for
/// a typed parent and as one declarer among the rest for an untyped one.
#[test]
fn a_redeclared_name_resolves_to_the_later_declaration() {
    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "legacy_article_id"
  end
end
"#,
        &[(
            "class Article < ApplicationRecord\n  has_many :comments\n  has_many :comments, foreign_key: :legacy_article_id\nend\n",
            "app/models/article.rb",
        )],
    );
    for written in ["@parent: Article", "@parent"] {
        assert_eq!(comments(&reg, written, "build"), comment_by("legacy_article_id"), "{written}");
    }
}

/// One model's direct `has_many :comments` and another's `through:` (or
/// `has_one`) of the same name do not agree. The rewrite declines an
/// untyped parent, and the seed must decline with it: otherwise
/// `@parent.comments.find` becomes `Comment` filtered by `article_id`.
/// A parent typed as the model that declares the direct `has_many` still
/// reads that declaration.
#[test]
fn a_non_direct_declaration_keeps_the_seed_off_the_direct_key() {
    use roundhouse::ident::Symbol;
    use roundhouse::lower::scope_chain::assoc_read_target;

    let through = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "users", force: :cascade do |t|
  end
  create_table "posts", force: :cascade do |t|
    t.integer "user_id"
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "post_id"
  end
end
"#,
        &[
            ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
            ("class Post < ApplicationRecord\n  belongs_to :user\nend\n", "app/models/post.rb"),
            (
                "class User < ApplicationRecord\n  has_many :posts\n  has_many :comments, through: :posts\nend\n",
                "app/models/user.rb",
            ),
        ],
    );
    let has_one = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "users", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "user_id"
  end
end
"#,
        &[
            ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
            (
                "class User < ApplicationRecord\n  has_one :comments, class_name: \"Comment\"\nend\n",
                "app/models/user.rb",
            ),
        ],
    );
    let seed = |reg: &roundhouse::lower::scope_chain::AssocRegistry, written: &str, assoc: &str| {
        assoc_read_target(&owner(written), &Symbol::from(assoc), reg)
            .map(|(model, _, fk)| (model.map(|m| m.0.as_str().to_string()), fk.as_str().to_string()))
    };
    assert_eq!(comments(&through, "@parent", "find"), Declined);
    assert_eq!(seed(&through, "@parent", "comments"), None);
    assert_eq!(comments(&through, "@article: Article", "find"), comment_by("article_id"));
    assert_eq!(
        seed(&through, "@article: Article", "comments"),
        Some((Some("Article".to_string()), "article_id".to_string()))
    );
    assert_eq!(comments(&has_one, "@parent", "find"), Declined);
    assert_eq!(seed(&has_one, "@parent", "comments"), None);
}

/// An extension's method answers the call by its name in Rails' place,
/// so that call is declined; the others stay on the declaration.
#[test]
fn an_extension_declines_only_the_call_it_defines() {
    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
  end
end
"#,
        &[(
            "class Article < ApplicationRecord\n  has_many :comments do\n    def find(id)\n      where(id: id).first\n    end\n  end\nend\n",
            "app/models/article.rb",
        )],
    );
    for written in ["@parent: Article", "@parent"] {
        assert_eq!(comments(&reg, written, "find"), Declined, "{written}");
        assert_eq!(comments(&reg, written, "build"), comment_by("article_id"), "{written}");
    }
}

/// A scope is judged per call. Ordering decides which match `find_by`
/// takes, so it declines `find_by` alone; what `build` makes and the row
/// `find(id)` names do not depend on it. Eager loading, `references` and
/// `readonly` change none of them; a `where` changes all of them.
#[test]
fn a_scope_declines_only_the_calls_it_changes() {
    let reg = registry(
        r#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.string "commenter"
    t.datetime "created_at"
  end
end
"#,
        &[(
            "class Article < ApplicationRecord\n  has_many :ordered, -> { order(created_at: :desc) }, class_name: \"Comment\"\n  has_many :reordered, -> { includes(:article).reorder(:id) }, class_name: \"Comment\"\n  has_many :locked, -> { readonly }, class_name: \"Comment\"\n  has_many :referenced, -> { includes(:article).references(:article) }, class_name: \"Comment\"\n  has_many :filtered, -> { where(commenter: \"amy\") }, class_name: \"Comment\"\nend\n",
            "app/models/article.rb",
        )],
    );
    let builds_and_finds = ["build", "new", "create", "create!", "find"];
    for assoc in ["ordered", "reordered"] {
        for call in builds_and_finds {
            assert_eq!(resolve(&reg, "@article: Article", assoc, call), comment_by("article_id"), "{assoc}.{call}");
        }
        assert_eq!(resolve(&reg, "@article: Article", assoc, "find_by"), Declined, "{assoc}.find_by");
    }
    for assoc in ["locked", "referenced"] {
        for call in builds_and_finds.iter().chain(&["find_by"]) {
            assert_eq!(resolve(&reg, "@article: Article", assoc, call), comment_by("article_id"), "{assoc}.{call}");
        }
    }
    for call in builds_and_finds.iter().chain(&["find_by"]) {
        assert_eq!(resolve(&reg, "@article: Article", "filtered", call), Declined, "filtered.{call}");
    }
}
