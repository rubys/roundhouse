//! A nested child's required `belongs_to` where Rails infers no inverse
//! (activerecord 8.1.4, oracles `inverse.rb`, `cr_oracle.rb`). The owner's
//! `has_many` / `has_one` sets the child's owner when it has an inverse:
//! its `inverse_of:` names one, or Rails infers it, which neither side's
//! `foreign_key:` nor `inverse_of: false` allows. Without one, a child
//! built on a new owner has no owner when it's validated, so "… must
//! exist" fails the owner's save.
//! A saved owner's children have its key and pass.
//!
//! The nested check always took the child's owner as set, skipping its
//! "must exist", so such an owner saved where Rails rejects it.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
body = "A body long enough to validate."
a = Article.new(title: "t", body: body, comments_attributes: [{ commenter: "c", body: "one" }])
check("no inverse: the new owner's save fails", [a.save, a.errors, Comment.count], [false, ["Comments article must exist"], 0])
b = Article.create!(title: "u", body: body)
check("a saved owner's child has its key", [b.update(comments_attributes: [{ commenter: "c", body: "two" }]), Comment.where(article_id: b.id).count], [true, 1])
puts "nested inverse passed"
"##;

fn app(belongs_to: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments\n",
        )
        .edit("app/models/comment.rb", "  belongs_to :article\n", belongs_to)
}

#[test]
fn a_foreign_key_leaves_a_nested_child_without_its_owner_on_ruby() {
    let run = app("  belongs_to :article, class_name: \"Article\", foreign_key: \"article_id\"\n").run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested inverse passed"), "{}", run.stdout);
}

const INVERSE_SCRIPT: &str = r##"
a = Article.new(title: "t", body: "A body long enough to validate.", comments_attributes: [{ commenter: "c", body: "one" }])
raise "inverse_of: #{[a.save, a.errors].inspect}" unless a.save && Comment.count == 1
puts "inverse_of passed"
"##;

#[test]
fn an_inverse_of_keeps_the_owner_set_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy, inverse_of: :article\n  accepts_nested_attributes_for :comments\n",
        )
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article, foreign_key: \"article_id\", inverse_of: :comments\n")
        .run_ruby(INVERSE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("inverse_of passed"), "{}", run.stdout);
}

// The owner's reflection decides (review 4372094552, oracle
// `cr_oracle.rb`): `build` sets the inverse from the `has_many` /
// `has_one`, which has one when its `inverse_of:` names it, or when Rails
// infers it: neither side writes `foreign_key:` and neither says
// `inverse_of: false`. The child's `inverse_of:` name alone doesn't.
fn saves(macro_: &str, owner: &str, child: &str) -> (bool, String) {
    let (assoc, attrs, human) = match macro_ {
        "has_many" => ("comments", "[{ commenter: \"c\", body: \"one\" }]", "Comments"),
        _ => ("comment", "{ commenter: \"c\", body: \"one\" }", "Comment"),
    };
    let declaration = if owner.is_empty() { format!("  {macro_} :{assoc}\n") } else { format!("  {macro_} :{assoc}, {owner}\n") };
    let belongs_to = if child.is_empty() { "  belongs_to :article\n".to_string() } else { format!("  belongs_to :article, {child}\n") };
    let script = format!(
        "a = Article.new(title: \"t\", body: \"A body long enough to validate.\", {assoc}_attributes: {attrs})\nsaved = a.save\nraise \"errors: #{{a.errors.inspect}}\" unless saved || a.errors == [\"{human} article must exist\"]\nputs \"saved=#{{saved}}\"\n"
    );
    let mut app = emit_and_run::real_blog();
    app = if macro_ == "has_many" {
        app.edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", &format!("{declaration}  accepts_nested_attributes_for :comments\n"))
    } else {
        app.edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", &format!("  has_many :comments, dependent: :destroy\n{declaration}  accepts_nested_attributes_for :comment\n"))
    };
    let run = app.edit("app/models/comment.rb", "  belongs_to :article\n", &belongs_to).run_ruby(&script);
    run.assert_passes();
    (run.stdout.contains("saved=true"), run.stdout.clone())
}

#[test]
fn the_owners_reflection_decides_the_inverse_on_ruby() {
    let rows: &[(&str, &str, &str, &str, bool)] = &[
        ("A. inverse_of only on the child, foreign_key on the has_many", "has_many", "foreign_key: \"article_id\"", "inverse_of: :comments", false),
        ("B. inverse_of only on the owner, foreign_key on the has_many", "has_many", "foreign_key: \"article_id\", inverse_of: :article", "", true),
        ("B'. inverse_of only on the owner, foreign_key on both", "has_many", "foreign_key: \"article_id\", inverse_of: :article", "foreign_key: \"article_id\"", true),
        ("C. inverse_of on both, foreign_key on both", "has_many", "foreign_key: \"article_id\", inverse_of: :article", "foreign_key: \"article_id\", inverse_of: :comments", true),
        ("D. inverse_of: false on the owner, no foreign_key", "has_many", "inverse_of: false", "", false),
        ("E. inverse_of: false on the owner, foreign_key on the has_many", "has_many", "foreign_key: \"article_id\", inverse_of: false", "", false),
        ("F. foreign_key only on the belongs_to, inverse_of on the owner", "has_many", "inverse_of: :article", "foreign_key: \"article_id\"", true),
        ("G. foreign_key only on the belongs_to", "has_many", "", "foreign_key: \"article_id\"", false),
        ("H. inverse_of: false on the child", "has_many", "", "inverse_of: false", false),
        ("I. plain", "has_many", "", "", true),
        ("A1. has_one, inverse_of only on the child, foreign_key on the owner", "has_one", "foreign_key: \"article_id\"", "inverse_of: :comment", false),
        ("B1. has_one, inverse_of only on the owner, foreign_key on the owner", "has_one", "foreign_key: \"article_id\", inverse_of: :article", "", true),
        ("D1. has_one, inverse_of: false on the owner", "has_one", "inverse_of: false", "", false),
    ];
    let wrong: Vec<String> = rows
        .iter()
        .filter_map(|(label, macro_, owner, child, want)| {
            let (got, out) = saves(macro_, owner, child);
            (got != *want).then(|| format!("{label}: saved {got}, Rails {want} ({out})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}
