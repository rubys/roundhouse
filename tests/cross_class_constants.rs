//! A validation that names another class's or module's constant:
//!
//! ```ruby
//! class RuleTranslation < ApplicationRecord
//!   validates :text, length: { maximum: Rule::TEXT_SIZE_LIMIT }
//! end
//! ```
//!
//! Ruby resolves `Rule` lexically from the reference site, then reads
//! `TEXT_SIZE_LIMIT` from that owner. The validation folded only the
//! class body's own constants, so a qualified one stayed the reported
//! gap. It now reads the owner's constants, folded by the same rules as
//! the owner's own body (one plain top-level write, no mutation there),
//! when no file the constant prepass reads writes, mutates, or lets a
//! variable hold that name, and the owner is declared in one body.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::ValidationRule;
use roundhouse::expr::Literal;
use roundhouse::ingest::{ingest_app_from_tree, survey};

const SCHEMA: &str = "ActiveRecord::Schema.define do
  create_table \"widgets\", force: :cascade do |t|
    t.string \"text\"
    t.string \"name\"
    t.string \"category\"
    t.string \"kind\"
    t.string \"mode\"
    t.string \"code\"
    t.string \"shade\"
    t.string \"email\"
    t.string \"label\"
    t.string \"note\"
    t.string \"slug\"
  end
  create_table \"fasp_subscriptions\", force: :cascade do |t|
    t.string \"category\"
  end
  create_table \"fasp_backfill_requests\", force: :cascade do |t|
    t.string \"category\"
  end
end
";

fn ingest(files: &[(&str, &str)]) -> (roundhouse::App, Vec<String>) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    for (path, source) in files {
        tree.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = survey::drain().into_iter().map(|g| g.to_string()).collect();
    (app, gaps)
}

fn rules<'a>(app: &'a roundhouse::App, model: &str, attr: &str) -> Vec<&'a ValidationRule> {
    let model = app.models.iter().find(|m| m.name.0.as_str() == model).unwrap_or_else(|| panic!("{model}"));
    model
        .body
        .iter()
        .filter_map(|item| match item {
            roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
                Some(&validation.rules)
            }
            _ => None,
        })
        .flatten()
        .collect()
}

const RULE: &str = "class Rule < ApplicationRecord
  self.table_name = \"widgets\"
  TEXT_SIZE_LIMIT = 300
  MAX_SCORE = (2**31) - 1
end
";

const TAG: &str = "class Tag < ApplicationRecord
  self.table_name = \"widgets\"
  NAME_PAT = \"[a-z]+\"
  NAME_RE = /\\A#{NAME_PAT}\\z/i
end
";

const FASP: &str = "module Fasp
  DATA_CATEGORIES = %w(account content).freeze
end
";

#[test]
fn another_owners_constants_fold_into_a_validation() {
    let (app, gaps) = ingest(&[
        ("app/models/rule.rb", RULE),
        ("app/models/tag.rb", TAG),
        ("app/models/fasp.rb", FASP),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord
  validates :text, length: { maximum: Rule::TEXT_SIZE_LIMIT }
  validates :mode, numericality: { less_than_or_equal_to: Rule::MAX_SCORE, greater_than: -Rule::MAX_SCORE }
  validates :name, format: { with: Tag::NAME_RE }
  validates :category, inclusion: Fasp::DATA_CATEGORIES
  validates :kind, inclusion: { in: ::Fasp::DATA_CATEGORIES }
end
",
        ),
        (
            "app/models/fasp/subscription.rb",
            "class Fasp::Subscription < ApplicationRecord
  validates :category, inclusion: Fasp::DATA_CATEGORIES
end
",
        ),
        (
            "app/models/fasp/backfill_request.rb",
            "module Fasp
  class BackfillRequest < ApplicationRecord
    validates :category, inclusion: { in: DATA_CATEGORIES }
  end
end
",
        ),
    ]);
    assert!(matches!(rules(&app, "Widget", "text").as_slice(), [ValidationRule::Length { max: Some(300), .. }]), "{gaps:?}");
    // An owner's Integer arithmetic (#1161) is a numericality bound too.
    assert!(
        matches!(rules(&app, "Widget", "mode").as_slice(), [ValidationRule::Numericality { lte: Some(lte), gt: Some(gt), .. }]
            if *lte == 2147483647.0 && *gt == -2147483647.0),
        "{:?} {gaps:?}",
        rules(&app, "Widget", "mode")
    );
    assert!(
        matches!(rules(&app, "Widget", "name").as_slice(), [ValidationRule::Format { pattern, flags, .. }]
            if pattern == r"\A[a-z]+\z" && flags == "i"),
        "{:?} {gaps:?}",
        rules(&app, "Widget", "name")
    );
    let categories = |model: &str, attr: &str| match rules(&app, model, attr).as_slice() {
        [ValidationRule::Inclusion { values, .. }] => values.clone(),
        other => panic!("{model}.{attr}: {other:?} {gaps:?}"),
    };
    let want = vec![Literal::Str { value: "account".into() }, Literal::Str { value: "content".into() }];
    assert_eq!(categories("Widget", "category"), want);
    assert_eq!(categories("Widget", "kind"), want);
    assert_eq!(categories("Fasp::Subscription", "category"), want);
    // A bare name inside `module Fasp` was already Ruby's lexical lookup
    // through the module; it reads the module's constant the same way.
    assert_eq!(categories("Fasp::BackfillRequest", "category"), want);
    assert!(!gaps.iter().any(|g| g.contains("widget.rb") || g.contains("fasp/")), "{gaps:?}");
}

/// Each case where Ruby may read something other than the folded value
/// stays the reported gap.
#[test]
fn a_constant_ruby_may_read_differently_stays_a_gap() {
    let (app, gaps) = ingest(&[
        ("app/models/rule.rb", RULE),
        (
            "app/models/kinds.rb",
            "module Kinds
  ALL = %w(a b)
  MODES = %w(x y)
  SHADES = %w(light dark)
  CODES = %w(c d)
  COMPUTED = %w(a b).map(&:upcase)
end
",
        ),
        // A method elsewhere appends to one list; a variable holds
        // another, which it may mutate later out of sight.
        (
            "app/models/kind_registry.rb",
            "class KindRegistry
  def self.register(kind)
    Kinds::ALL << kind
  end

  def self.modes
    list = Kinds::MODES
    list
  end
end
",
        ),
        // Two bodies of one owner write the same name: which value Ruby
        // sees depends on load order.
        ("app/models/palette.rb", "module Palette\n  SHADES = %w(red blue)\nend\n"),
        ("app/models/palette_ext.rb", "module Palette\n  SHADES = %w(green)\nend\n"),
        // An app body reopening a standard-library namespace is not the
        // whole owner: the library writes it too.
        ("lib/uri_mail_to.rb", "module URI\n  module MailTo\n    LOCAL_RE = /x/\n  end\nend\n"),
        // `Rule` here is `Widget::Rule`, which has no `TEXT_SIZE_LIMIT`:
        // Rails raises NameError, so nothing can fold.
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord
  module Rule
  end

  validates :text, length: { maximum: Rule::TEXT_SIZE_LIMIT }
  validates :kind, inclusion: Kinds::ALL
  validates :mode, inclusion: Kinds::MODES
  validates :code, inclusion: { in: Kinds::COMPUTED }
  validates :shade, inclusion: Palette::SHADES
  validates :label, inclusion: Kinds::MISSING
  validates :name, inclusion: Kinds::SHADES
  validates :slug, format: { with: URI::MailTo::LOCAL_RE }
  validates :email, format: { with: Markup::LINK_RE }
end
",
        ),
    ]);
    for attr in ["text", "kind", "mode", "code", "shade", "label", "email", "slug"] {
        assert!(rules(&app, "Widget", attr).is_empty(), "{attr}: {:?}", rules(&app, "Widget", attr));
        assert!(gaps.iter().any(|g| g.contains("widget.rb") && g.contains(&format!(":{attr},"))), "{attr}: {gaps:?}");
    }
    // The control: the same owner's untouched constant still folds, so
    // each refusal above is that constant's own.
    assert!(matches!(rules(&app, "Widget", "name").as_slice(), [ValidationRule::Inclusion { .. }]), "{gaps:?}");
}

/// `const_set` on the owner from a method anywhere replaces constants
/// Ruby reads later, so the owner's constants do not fold.
#[test]
fn reflection_on_the_owner_keeps_its_constants_a_gap() {
    let (app, gaps) = ingest(&[
        ("app/models/rule.rb", RULE),
        (
            "app/models/rule_loader.rb",
            "class RuleLoader\n  def self.load(limit)\n    Rule.send(:remove_const, :TEXT_SIZE_LIMIT)\n    Rule.const_set(:TEXT_SIZE_LIMIT, limit)\n  end\nend\n",
        ),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :text, length: { maximum: Rule::TEXT_SIZE_LIMIT }\nend\n"),
    ]);
    assert!(rules(&app, "Widget", "text").is_empty(), "{:?}", rules(&app, "Widget", "text"));
    assert!(gaps.iter().any(|g| g.contains("widget.rb") && g.contains(":text,")), "{gaps:?}");
}

/// `Regexp.union` of literal Strings (or a folded list of them) is the
/// escaped alternation Ruby builds; with no String it matches nothing, as
/// Ruby's `/(?!)/` does, spelled `[^\s\S]`, which every target's engine
/// compiles (RE2 on go and Rust's regex crate reject a lookahead), which
/// matches nothing.
#[test]
fn regexp_union_folds_to_an_escaped_alternation() {
    let (app, gaps) = ingest(&[(
        "app/models/widget.rb",
        "class Widget < ApplicationRecord
  SLUGS = %w(a.b c+d).freeze
  SLUG_RE = Regexp.union(SLUGS)
  NONE_RE = Regexp.union
  EMPTY_RE = Regexp.union([])
  validates :slug, format: { with: SLUG_RE }
  validates :code, format: { with: Regexp.union(\"x\", \"y|z\") }
  validates :name, format: { with: NONE_RE }
  validates :text, format: { with: EMPTY_RE }
end
",
    )]);
    let pattern = |attr: &str| match rules(&app, "Widget", attr).as_slice() {
        [ValidationRule::Format { pattern, flags, .. }] if flags.is_empty() => pattern.clone(),
        other => panic!("{attr}: {other:?} {gaps:?}"),
    };
    assert_eq!(pattern("slug"), r"a\.b|c\+d");
    assert_eq!(pattern("code"), r"x|y\|z");
    assert_eq!(pattern("name"), r"[^\s\S]");
    assert_eq!(pattern("text"), r"[^\s\S]");
    // The strict targets compile what the fold hands them: Rust's own
    // regex crate stands for rust's emit here (go's RE2 rejects the same
    // lookaround).
    for attr in ["slug", "code", "name", "text"] {
        let pattern = pattern(attr);
        let re = regex::Regex::new(&pattern).unwrap_or_else(|e| panic!("{attr}: {pattern}: {e}"));
        if attr == "name" || attr == "text" {
            assert!(!re.is_match("") && !re.is_match("a") && !re.is_match("\n"), "{pattern} matches nothing");
        }
    }
}

/// An app constant named `Regexp` may be what a body's `Regexp.union`
/// resolves to, so no union folds then.
#[test]
fn an_app_regexp_constant_keeps_regexp_union_a_gap() {
    let (app, gaps) = ingest(&[
        ("app/models/kinds.rb", "module Kinds\n  SLUGS = Regexp.union(%w(a b))\n  module Regexp\n  end\nend\n"),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord
  SLUG_RE = Regexp.union(%w(a b))
  validates :slug, format: { with: SLUG_RE }
  validates :code, format: { with: Regexp.union(\"x\") }
  validates :name, format: { with: Kinds::SLUGS }
end
",
        ),
    ]);
    for attr in ["slug", "code", "name"] {
        assert!(rules(&app, "Widget", attr).is_empty(), "{attr}: {:?}", rules(&app, "Widget", attr));
        assert!(gaps.iter().any(|g| g.contains("widget.rb") && g.contains(&format!(":{attr},"))), "{attr}: {gaps:?}");
    }
}

/// `const_get` returns the owner's constant itself, so a mutation through
/// it (`Kinds.const_get(:ALL) << "c"`) reaches the folded value, as does a
/// `constantize` of the owner's name. `Object.const_get` (or a
/// receiverless one outside any body) can reach any owner, so nothing
/// folds then; `Object.const_set(:Name, …)` writes only `Name`, and an
/// unrelated `Object.const_set(:Other, …)` keeps every owner folding. A
/// dynamic `name.constantize` is not followed.
#[test]
fn a_dynamic_constant_lookup_keeps_the_owner_a_gap() {
    let kinds = "module Kinds\n  ALL = %w(a b)\nend\n";
    let model = "class Widget < ApplicationRecord\n  validates :kind, inclusion: Kinds::ALL\n  validates :text, length: { maximum: Rule::TEXT_SIZE_LIMIT }\nend\n";
    for (case, source, rule_folds) in [
        ("const_get", "class Registry\n  def self.add(kind)\n    Kinds.const_get(:ALL) << kind\n  end\nend\n", true),
        ("constantize", "class Registry\n  def self.add(kind)\n    \"Kinds::ALL\".constantize << kind\n  end\nend\n", true),
        ("Object.const_get", "class Registry\n  def self.add(name, kind)\n    Object.const_get(name) << kind\n  end\nend\n", false),
        ("top-level const_get", "const_get(:Kinds)::ALL << \"c\"\n", false),
        // `Object.const_set(:Name, …)` writes the one top-level `Name`.
        ("Object.const_set(:Kinds)", "class Registry\n  def self.swap(list)\n    Object.const_set(:Kinds, list)\n  end\nend\n", true),
    ] {
        let (app, gaps) = ingest(&[
            ("app/models/rule.rb", RULE),
            ("app/models/kinds.rb", kinds),
            ("app/models/registry.rb", source),
            ("app/models/widget.rb", model),
        ]);
        assert!(rules(&app, "Widget", "kind").is_empty(), "{case}: {:?}", rules(&app, "Widget", "kind"));
        assert!(gaps.iter().any(|g| g.contains("widget.rb") && g.contains(":kind,")), "{case}: {gaps:?}");
        assert_eq!(!rules(&app, "Widget", "text").is_empty(), rule_folds, "{case}: Rule's constant {gaps:?}");
    }
}

#[test]
fn an_unrelated_top_level_const_set_keeps_owners_folding() {
    let (app, gaps) = ingest(&[
        ("app/models/rule.rb", RULE),
        ("lib/tasks_helper.rb", "class TasksHelper\n  def self.define(klass)\n    Object.const_set(:StatusToBeDeleted, klass)\n  end\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :text, length: { maximum: Rule::TEXT_SIZE_LIMIT }\nend\n"),
    ]);
    assert!(matches!(rules(&app, "Widget", "text").as_slice(), [ValidationRule::Length { max: Some(300), .. }]), "{gaps:?}");
}

/// Parentheses around the constant are only Ruby grouping:
/// `inclusion: (Fasp::DATA_CATEGORIES)`, `in: (MODES)` and
/// `with: (Tag::NAME_RE)` read the same value as without them. The list
/// and regex lookups stopped at the parentheses node, so each stayed the
/// reported gap. The numericality and length bounds already looked
/// through them, as these pins show.
#[test]
fn a_parenthesized_constant_folds_like_a_bare_one() {
    let (app, gaps) = ingest(&[
        ("app/models/rule.rb", RULE),
        ("app/models/tag.rb", TAG),
        ("app/models/fasp.rb", FASP),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord
  MODES = %w(x y).freeze
  validates :category, inclusion: (Fasp::DATA_CATEGORIES)
  validates :kind, inclusion: { in: (Fasp::DATA_CATEGORIES) }
  validates :mode, inclusion: { in: (MODES) }
  validates :name, format: { with: (Tag::NAME_RE) }
  validates :text, length: { maximum: (Rule::TEXT_SIZE_LIMIT) }
  validates :code, numericality: { less_than: (Rule::TEXT_SIZE_LIMIT) }
end
",
        ),
    ]);
    for attr in ["category", "kind", "mode", "name", "text", "code"] {
        assert_eq!(rules(&app, "Widget", attr).len(), 1, "{attr}: {:?} {gaps:?}", rules(&app, "Widget", attr));
    }
    assert!(!gaps.iter().any(|g| g.contains("widget.rb")), "{gaps:?}");
    assert!(matches!(rules(&app, "Widget", "name").as_slice(), [ValidationRule::Format { pattern, .. }] if pattern == r"\A[a-z]+\z"));
}
