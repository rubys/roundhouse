use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect()
}

fn association_names(app: &roundhouse::App, model: &str) -> Vec<String> {
    app.models
        .iter()
        .find(|candidate| candidate.name.0.as_str() == model)
        .unwrap_or_else(|| panic!("missing model {model}"))
        .associations()
        .map(|association| association.name().as_str().to_string())
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "workspaces" do |t|; end
  create_table "receipts" do |t|; t.integer "workspace_id"; end
  create_table "memberships" do |t|; t.integer "workspace_id"; end
end
"#;

#[test]
fn nested_model_concerns_keep_qualified_ownership_and_do_not_leak() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/workspace.rb",
            r#"class Workspace < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { has_many :memberships }
  end
  include Associations
end
"#,
        ),
        (
            "app/models/receipt.rb",
            r#"class Receipt < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { belongs_to :workspace, optional: true }
  end
  include Associations
end
"#,
        ),
        (
            "app/models/membership.rb",
            "class Membership < ApplicationRecord\nend\n",
        ),
    ]))
    .expect("ingest nested model concerns");

    assert_eq!(association_names(&app, "Workspace"), ["memberships"]);
    assert_eq!(association_names(&app, "Receipt"), ["workspace"]);
}

#[test]
fn qualified_module_declaration_remains_the_control() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/workspace.rb",
            "class Workspace < ApplicationRecord\n  include Associations\nend\n",
        ),
        (
            "app/models/workspace/associations.rb",
            r#"module Workspace::Associations
  extend ActiveSupport::Concern
  included { has_many :memberships }
end
"#,
        ),
        (
            "app/models/membership.rb",
            "class Membership < ApplicationRecord\nend\n",
        ),
    ]))
    .expect("ingest qualified concern control");

    assert_eq!(association_names(&app, "Workspace"), ["memberships"]);
}

#[test]
fn support_root_models_register_nested_concern_items_too() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "lib/workspace.rb",
            r#"class Workspace < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { has_many :memberships }
  end
  include Associations
end
"#,
        ),
        (
            "lib/receipt.rb",
            r#"class Receipt < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { belongs_to :workspace, optional: true }
  end
  include Associations
end
"#,
        ),
        (
            "app/models/membership.rb",
            "class Membership < ApplicationRecord\nend\n",
        ),
    ]))
    .expect("ingest support-root model concern");

    assert_eq!(association_names(&app, "Workspace"), ["memberships"]);
    assert_eq!(association_names(&app, "Receipt"), ["workspace"]);
}

#[test]
fn nested_concern_enum_metadata_stays_with_its_includer() {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :workspaces do |t|; t.integer :status; end\n  create_table :receipts do |t|; end\nend\n",
        ),
        (
            "app/models/workspace.rb",
            "class Workspace < ApplicationRecord\n  module Status\n    extend ActiveSupport::Concern\n    included { enum :status, { pending: 0, archived: 3 } }\n  end\n  include Status\nend\n",
        ),
        (
            "app/models/receipt.rb",
            "class Receipt < ApplicationRecord\nend\n",
        ),
    ]))
    .expect("ingest nested enum metadata");
    let workspace = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Workspace")
        .unwrap();
    assert_eq!(
        workspace.enums[&roundhouse::Symbol::from("status")],
        [
            (
                "pending".to_string(),
                roundhouse::expr::Literal::Int { value: 0 }
            ),
            (
                "archived".to_string(),
                roundhouse::expr::Literal::Int { value: 3 }
            ),
        ]
    );
    let receipt = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Receipt")
        .unwrap();
    assert!(
        receipt.enums.is_empty(),
        "a different model must not inherit the enum"
    );
}

#[test]
fn model_ingest_failures_remain_errors() {
    let result = ingest_app_from_tree(tree(&[(
        "app/models/workspace.rb",
        r#"class Workspace < ApplicationRecord
  scope :broken, :not_a_lambda
  module Associations
    extend ActiveSupport::Concern
    included { has_many :memberships }
  end
end
"#,
    )]));

    assert!(
        result.is_err(),
        "a genuine model failure must not be hidden"
    );
}

#[test]
fn nested_library_failures_still_abort_strict_model_ingestion() {
    let result = ingest_app_from_tree(tree(&[(
        "app/models/workspace.rb",
        r#"class Workspace < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { has_many :memberships }
    def broken; `unsupported_command`; end
  end
end
"#,
    )]));
    assert!(
        result.is_err(),
        "the original nested library error must survive"
    );
}

#[test]
fn collection_does_not_admit_unregistered_sibling_modules() {
    let app = ingest_app_from_tree(tree(&[(
        "app/models/workspace.rb",
        r#"class Workspace < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { has_many :memberships }
  end
end
module Unregistered
  extend ActiveSupport::Concern
  included { belongs_to :workspace }
end
"#,
    )]))
    .unwrap();
    assert!(
        app.concern_model_items
            .contains_key(&roundhouse::ClassId("Workspace::Associations".into()))
    );
    assert!(
        !app.concern_model_items
            .contains_key(&roundhouse::ClassId("Unregistered".into()))
    );
}

#[test]
#[ignore = "requires native activerecord/activesupport 8.1.4; default emitted tests do not"]
fn native_nested_concerns_preserve_four_ownership_assertions() {
    let output = std::process::Command::new("ruby").arg("-e").arg(r#"
gem 'activerecord', '8.1.4'
gem 'activesupport', '8.1.4'
require 'active_record'
class ApplicationRecord < ActiveRecord::Base; self.abstract_class = true; end
class Workspace < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { has_many :memberships }
  end
  include Associations
end
class Receipt < ApplicationRecord
  module Associations
    extend ActiveSupport::Concern
    included { belongs_to :workspace, optional: true }
  end
  include Associations
end
raise 'has_many lost' unless Workspace.reflect_on_association(:memberships).macro == :has_many
raise 'belongs_to lost' unless Receipt.reflect_on_association(:workspace).macro == :belongs_to
raise 'foreign concern leaked' unless Workspace.reflect_on_association(:workspace).nil?
raise 'foreign concern leaked' unless Receipt.reflect_on_association(:memberships).nil?
puts "PASS: four native ownership assertions; Ruby #{RUBY_VERSION}, AR #{ActiveRecord::VERSION::STRING}, AS #{ActiveSupport::VERSION::STRING}"
"#).output().expect("native Ruby with pinned gems");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PASS: four native ownership assertions")
    );
}

#[test]
fn nested_concern_associations_run_in_emitted_ruby() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            r#"  module Associations
    extend ActiveSupport::Concern
    included { has_many :comments, dependent: :destroy }
  end
  include Associations"#,
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article",
            r#"  module Associations
    extend ActiveSupport::Concern
    included { belongs_to :article }
  end
  include Associations"#,
        )
        .run_ruby(
            r#"article = Article.create!(title: "Nested concern", body: "Long enough body")
comment = Comment.create!(article_id: article.id, commenter: "Compiler", body: "Works")
raise "has_many missing" unless article.comments.map(&:id) == [comment.id]
raise "belongs_to missing" unless comment.article.id == article.id
raise "article concern leaked" if Article.new.respond_to?(:article)
raise "comment concern leaked" if Comment.new.respond_to?(:comments)
"#,
        )
        .assert_passes();
}
