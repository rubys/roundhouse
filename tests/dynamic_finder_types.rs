use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn category() -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from("Category")),
        args: vec![].into(),
    }
}

#[test]
fn w088_review_extension_order_returns_match_singleton_precedence() {
    for (extensions, expected) in [
        ("extend FirstFinder, LastFinder", category()),
        ("extend FirstFinder\nextend LastFinder", Ty::Nil),
        ("extend LastFinder, FirstFinder", Ty::Nil),
        ("extend LastFinder\nextend FirstFinder", category()),
    ] {
        let model = format!("class Category < ActiveRecord::Base\n{extensions}\ndef self.lookup; find_by_name(\"found\"); end\nend\n");
        let types = action_types_with_models(&[
            ("first_finder", "module FirstFinder\ndef find_by(attrs); Category.new(id: 91, name: \"first owner\"); end\nend\n"),
            ("last_finder", "module LastFinder\ndef find_by(attrs); nil; end\nend\n"),
            ("category", &model),
        ], "def direct; Category.find_by_name(\"found\"); end\ndef ordinary; Category.find_by(name: \"found\"); end\ndef reflected; Category.send(:find_by_name, \"found\"); end\ndef reflected_string; Category.public_send(\"find_by_name\", \"found\"); end\ndef bare; Category.lookup; end\n");
        for (name, ty) in types { assert_eq!(ty, expected, "{extensions} {name}: inferred return must contain the native value"); }
    }
}

#[test]
fn w088_review_retained_hooks_keep_finder_returns_unknown() {
    for hook in ["find_by_name", "find_by_name!", "find_by", "find_by!", "method_missing", "public_send", "audit_label"] {
        for inclusion in ["direct", "nested", "parent"] {
            let external = format!("module ExternalFinder\ndef self.included(klass)\nklass.define_singleton_method(:{hook}) {{ |*args| \"hook owner\" }}\nend\nend\n");
            let carrier = "module FinderCarrier\ninclude ExternalFinder\ndef self.included(klass)\nklass.include ExternalFinder\nend\nend\n";
            let parent = format!("class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\n{}\nend\n", if inclusion == "parent" { "include ExternalFinder" } else { "" });
            let model = format!("class Category < ApplicationRecord\n{}\ndef self.lookup; find_by_name(\"found\"); end\nend\n", match inclusion { "direct" => "include ExternalFinder", "nested" => "include FinderCarrier", _ => "" });
            let types = action_types_with_models(&[("external_finder", &external), ("finder_carrier", carrier), ("application_record", &parent), ("category", &model)],
                "def direct; Category.find_by_name(\"found\"); end\ndef bare; Category.lookup; end\ndef bang; Category.find_by_name!(\"found\"); end\ndef ordinary; Category.find_by(name: \"found\"); end\ndef relation; Category.all.find_by_name(\"found\"); end\ndef reflected; Category.public_send(:find_by_name, \"found\"); end\n");
            for (name, ty) in types { assert!(ty.is_unknown(), "{hook} {inclusion} {name}: {ty:?}"); }
        }
    }
}

#[test]
fn w088_review_singleton_decoys_do_not_own_included_class_finders() {
    for (inclusion, definitions, expected) in [
        ("include ExternalFinder", "def self.find_by(attrs); \"decoy\"; end\ndef self.find_by!(attrs); \"decoy\"; end", nilable(category())),
        ("extend ExternalFinder", "def find_by(attrs); \"extended\"; end\ndef find_by!(attrs); \"extended\"; end", Ty::Str),
    ] {
        let external = format!("module ExternalFinder\n{definitions}\nend\n");
        let model = format!("class Category < ActiveRecord::Base\n{inclusion}\nend\n");
        let types = action_types_with_models(&[("external_finder", &external), ("category", &model)],
            "def direct; Category.find_by_name(\"found\"); end\ndef miss; Category.find_by_name(\"absent\"); end\ndef reflected; Category.send(:find_by_name, \"found\"); end\ndef bang; Category.find_by_name!(\"found\"); end\n");
        for name in ["direct", "miss", "reflected"] { assert_eq!(types[name], expected, "{inclusion} {name}"); }
        assert_eq!(types["bang"], if expected == Ty::Str { Ty::Str } else { category() }, "{inclusion} bang");
    }
}

fn nilable(ty: Ty) -> Ty {
    Ty::Union {
        variants: vec![ty, Ty::Nil].into(),
    }
}

#[test]
fn dynamic_finder_delegate_returns_reach_callers_instead_of_record_seeds() {
    for owner in ["own", "parent", "concern"] {
        for (delegate, value, expected) in [("find_by", "\"delegated\"", Ty::Str), ("find_by!", "\"bang delegated\"", Ty::Str), ("find_by!", "nil", Ty::Nil)] {
            let definition = format!("def self.{delegate}(attributes); {value}; end\n");
            let parent = format!("class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\n{}end\n", if owner == "parent" { &definition } else { "" });
            let concern = format!("module FinderHooks\nextend ActiveSupport::Concern\nclass_methods do\n{}end\nend\n", if owner == "concern" { definition.replace("def self.", "def ") } else { String::new() });
            let bang = if delegate == "find_by!" { "!" } else { "" };
            let category = format!(r#"class Category < ApplicationRecord
  {}
  def self.bare; find_by_name{bang}("found"); end
  def self.explicit; self.find_by_name{bang}("found"); end
  def self.raising; find_by_name!("found"); end
  def self.reflected; send(:find_by_name{bang}, "found"); end
end
"#, match owner { "own" => definition.as_str(), "concern" => "include FinderHooks", _ => "" });
            let types = action_types_with_models(&[("application_record", &parent), ("finder_hooks", &concern), ("category", &category)],
                "def bare; Category.bare; end\ndef explicit; Category.explicit; end\ndef raising; Category.raising; end\ndef reflected; Category.reflected; end\n");
            for name in ["bare", "explicit", "raising", "reflected"] {
                assert_eq!(types[name], expected, "{owner} {delegate} {name}");
            }
        }
    }
}

#[test]
fn dynamic_finder_source_delegates_with_unknown_returns_do_not_guess_records() {
    let types = action_types(
        "class Category < ActiveRecord::Base\n  def self.find_by(attributes); unknown_value; end\nend\n",
        "def direct; Category.find_by_name(\"found\"); end\ndef bang; Category.find_by_name!(\"found\"); end\n",
    );
    for (name, ty) in types { assert!(ty.is_unknown(), "{name}: {ty:?}"); }
}

#[test]
fn dynamic_finder_relation_fallback_does_not_override_source_dispatch() {
    for definition in [
        "def self.find_by_name(value); unknown_value; end",
        "def self.method_missing(name, *args); \"custom\"; end",
    ] {
        let types = action_types(
            &format!("class Category < ActiveRecord::Base\n{definition}\nend\n"),
            "def relation; Category.all.find_by_name(\"found\"); end\n",
        );
        assert!(types["relation"].is_unknown(), "{definition}: {:?}", types["relation"]);
    }
}

#[test]
fn finder_reflection_keeps_application_send_wrappers() {
    for alias in ["send", "public_send", "__send__"] {
        for finder in ["find_by_name", "find_by_name!", "find_by", "find_by!"] {
            let types = action_types(
                &format!("class Category < ActiveRecord::Base\ndef self.{alias}(name, *args); \"owned wrapper\"; end\nend\n"),
                &format!("def symbol; Category.{alias}(:{finder}, \"found\"); end\ndef string; Category.{alias}(\"{finder}\", \"found\"); end\n"),
            );
            assert_eq!(types["symbol"], Ty::Str, "{alias} {finder}");
            assert_eq!(types["string"], Ty::Str, "{alias} {finder}");
        }
    }
}

fn action_types(model: &str, actions: &str) -> HashMap<String, Ty> {
    action_types_with_models(&[("category", model)], actions)
}

fn action_types_with_models(models: &[(&str, &str)], actions: &str) -> HashMap<String, Ty> {
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :categories do |t|\n    t.string :name\n  end\nend\n".to_owned()),
        ("app/controllers/categories_controller.rb", format!("class CategoriesController < ActionController::Base\n{actions}\nend\n")),
    ];
    let mut tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, body)| (PathBuf::from(path), body.into_bytes()))
        .collect();
    for (stem, model) in models {
        tree.insert(
            PathBuf::from(format!("app/models/{stem}.rb")),
            model.as_bytes().to_vec(),
        );
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app.controllers[0]
        .actions()
        .map(|a| (a.name.as_str().to_owned(), a.body.ty.clone().unwrap()))
        .collect()
}

#[test]
fn class_dynamic_finders_keep_the_model_through_a_nullable_helper_and_ivar_guard() {
    let types = action_types(
        r#"class Category < ActiveRecord::Base
  def self.lookup(id)
    if id == 0
      nil
    else
      Category.find_by_id(id)
    end
  end
  def self.implicit_lookup(id)
    find_by_id(id)
  end
  def self.explicit_lookup(id)
    self.find_by_id(id)
  end
end
"#,
        r#"  def direct
    Category.find_by_id(1)
  end
  def absolute
    ::Category.find_by_id(1)
  end
  def bang
    Category.find_by_name!("one")
  end
  def reflected
    Category.send(:find_by_id, 1)
  end
  def copied
    klass = Category
    klass.find_by_id(1)
  end
  def helper
    @category = Category.lookup(1)
    raise "missing" if @category.nil?
    @category
  end
  def explicit_helper
    Category.explicit_lookup(1)
  end
  def implicit_helper
    Category.implicit_lookup(1)
  end
  def public_reflected
    Category.public_send(:find_by_id, 1)
  end
  def internal_reflected
    Category.__send__(:find_by_id, 1)
  end
"#,
    );
    for name in [
        "direct",
        "absolute",
        "reflected",
        "copied",
        "explicit_helper",
        "implicit_helper",
        "public_reflected",
        "internal_reflected",
    ] {
        assert_eq!(types[name], nilable(category()), "{name}");
    }
    assert_eq!(types["bang"], category());
    assert_eq!(types["helper"], category());
}

#[test]
fn dynamic_finders_do_not_guess_a_column_or_an_instance_method() {
    let types = action_types(
        "class Category < ActiveRecord::Base\n  def nickname\n    \"alias\"\n  end\nend\n",
        r#"  def missing_column
    Category.find_by_nickname("one")
  end
  def instance
    Category.new.find_by_id(1)
  end
  def missing_argument
    Category.find_by_id
  end
  def extra_argument
    Category.find_by_id(1, 2)
  end
  def block
    Category.find_by_id(1) { 7 }
  end
  def splatted_arguments
    Category.find_by_id(*[1, 2])
  end
  def computed_argument
    Category.find_by_nickname(params[:name])
  end
"#,
    );
    for (name, ty) in types {
        assert!(ty.is_unknown(), "{name}: {ty:?}");
    }
}

#[test]
fn application_dynamic_finder_definitions_win_on_each_side() {
    let types = action_types(
        r#"class Category < ActiveRecord::Base
  def self.find_by_id(id)
    "own class"
  end
  def find_by_name(name)
    7
  end
end
"#,
        r#"  def class_override
    Category.find_by_id(1)
  end
  def instance_override
    Category.new.find_by_name("one")
  end
  def class_ignores_instance_override
    Category.find_by_name("one")
  end
"#,
    );
    assert_eq!(types["class_override"], Ty::Str);
    assert_eq!(types["instance_override"], Ty::Int);
    assert_eq!(types["class_ignores_instance_override"], nilable(category()));
}

#[test]
fn an_inherited_application_finder_keeps_its_return() {
    let types = action_types_with_models(
        &[
            ("category", "class Category < ActiveRecord::Base\nend\n"),
            (
                "parent",
                r#"class Parent < ActiveRecord::Base
  self.abstract_class = true
  def self.find_by_id(id)
    "ancestor"
  end
end
"#,
            ),
            (
                "special_category",
                "class SpecialCategory < Parent\n  self.table_name = \"categories\"\nend\n",
            ),
        ],
        r#"  def inherited_override
    SpecialCategory.find_by_id(1)
  end
"#,
    );
    assert_eq!(types["inherited_override"], Ty::Str);
}

#[test]
fn a_plain_class_does_not_acquire_active_record_finders() {
    let types = action_types(
        "class Category\n  attr_reader :name\nend\n",
        "  def plain\n    Category.find_by_id(1)\n  end\n",
    );
    assert!(types["plain"].is_unknown(), "{:?}", types["plain"]);
}

#[test]
fn an_unresolved_application_finder_does_not_acquire_a_record_return() {
    let types = action_types(
        r#"class Category < ActiveRecord::Base
  def self.find_by_id(id)
    opaque(id)
  end
end
"#,
        "  def own_unknown\n    Category.find_by_id(1)\n  end\n",
    );
    assert!(
        types["own_unknown"].is_unknown(),
        "{:?}",
        types["own_unknown"]
    );
}

#[test]
fn an_application_method_missing_keeps_its_own_return() {
    let types = action_types(
        r#"class Category < ActiveRecord::Base
  def self.method_missing(name, *arguments)
    "custom"
  end
end
"#,
        "  def custom_missing\n    Category.find_by_id(1)\n  end\n",
    );
    assert_eq!(types["custom_missing"], Ty::Str);
}

#[test]
fn ordinary_finders_distinguish_subclass_instances_and_inherited_class_overrides() {
    let types = action_types_with_models(&[
        ("category", "class Category < ActiveRecord::Base\n  def self.lookup; find_by(name: \"one\"); end\nend\n"),
        ("special_category", "class SpecialCategory < Category\n  def self.find_by(value); :class_side; end\n  def find_by(value); :instance_side; end\nend\n"),
    ], "def instance; SpecialCategory.new.find_by(name: \"one\"); end\ndef inherited; SpecialCategory.lookup; end\n");
    assert_eq!(types["instance"], Ty::Sym);
    let inherited = &types["inherited"];
    assert!(matches!(inherited, Ty::Sym) || matches!(inherited, Ty::Union { variants } if variants.contains(&Ty::Sym)), "{inherited:?}");
}
