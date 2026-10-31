//! PostgreSQL: class macro class eval string concern, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ACTS_AS_LIST: &str = r####"# frozen_string_literal: true

module ActsAsList
  extend ActiveSupport::Concern


  # This +acts_as+ extension provides the capabilities for sorting and reordering a number of objects in a list.
  # The class that has this specified needs to have a +position+ column defined as an integer on
  # the mapped database table.
  #
  # Todo list example:
  #
  #   class TodoList < ApplicationRecord
  #     has_many :todo_items, -> { order "position" }
  #   end
  #
  #   class TodoItem < ApplicationRecord
  #     include ActsAsList
  #
  #     belongs_to :todo_list
  #     acts_as_list :scope => :todo_list
  #   end
  #
  #   todo_list.first.move_to_bottom
  #   todo_list.last.move_higher
  module ClassMethods
    include Kernel

    # Configuration options are:
    #
    # * +column+ - specifies the column name to use for keeping the position integer (default: +position+)
    # * +scope+ - restricts what is to be considered a list. Given a symbol, it'll attach <tt>_id</tt>
    #   (if it hasn't already been added) and use that as the foreign key restriction. It's also possible
    #   to give it an entire string that is interpolated if you need a tighter scope than just a foreign key.
    #   Example: <tt>acts_as_list :scope => 'todo_list_id = #{todo_list_id} AND completed = 0'</tt>

    #: (?column: String, ?use_position_if_set: bool, ?add_to_list_bottom: bool, ?scope: Symbol?, ?limit: Integer?) -> void
    def acts_as_list(column: "position", use_position_if_set: false, add_to_list_bottom: true, scope: nil, limit: nil)
      model_class = self #: as singleton(ApplicationRecord)

      scope_condition_method = if scope
        %(
          def acts_as_list_scope_condition
            { #{scope}: #{scope} }
          end
        )
      end

      class_eval(<<-EOV)
        private

        def acts_as_list_class
          ::#{model_class.name}
        end

        def position_column
          '#{column}'
        end

        def position_column_limit
          #{limit.present? ? limit : "nil"}
        end

        #{scope_condition_method}

        before_create :add_to_list_bottom, :if => Proc.new { |list|
          if use_position_if_set && list[position_column].present? && list[position_column] > 0
            false
          else
            add_to_list_bottom || list[position_column] == 0 || list[position_column].nil?
          end
        }

      EOV
    end
  end

  # All the methods needed for <tt>acts_as_list</tt>. Each method works
  # by assuming the object to be the item in the list, so <tt>chapter.move_lower</tt> would move that chapter
  # lower in the list of all chapters. Likewise, <tt>chapter.first?</tt> would return +true+ if that chapter is
  # the first in the list of all chapters.
  def add_to_list_bottom
    self[position_column] = bottom_position_in_list.to_i + 1

    if position_column_limit && position_column_limit < self[position_column]
      self[position_column] = position_column_limit
    end
  end

  private

  # Overwrite this method to define the scope of the list changes
  def acts_as_list_scope_condition
  end

  # Returns the bottom position number in the list.
  #   bottom_position_in_list    # => 2
  def bottom_position_in_list
    item = bottom_item
    item ? item.send(position_column) : 0
  end

  # Returns the bottom item
  def bottom_item
    acts_as_list_class.where(acts_as_list_scope_condition).order(position_column => :desc).first
  end
end
"####;

const OPTION: &str = r#"class Option < ApplicationRecord
  include ActsAsList

  acts_as_list scope: :product_id, limit: 127

  def move_to_end
    add_to_list_bottom
  end
end
"#;

#[test]
fn a_model_class_body_macro_that_class_evals_a_string_is_called() {
    roundhouse::ingest::survey::activate();
    let (emitted, _errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_class_macro_eval\n")
        .write("db/structure.sql", "CREATE TABLE options (\n    id bigserial NOT NULL,\n    position integer,\n    product_id bigint,\n    CONSTRAINT options_pkey PRIMARY KEY (id)\n);\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/acts_as_list.rb", ACTS_AS_LIST)
        .write("app/models/option.rb", OPTION)
        .emit(BuildTarget::Ruby);
    let option = std::fs::read_to_string(emitted.join("app/models/option.rb")).expect("app/models/option.rb");
    assert!(
        option.lines().any(|l| l.trim_start().starts_with("acts_as_list(") || l.trim_start().starts_with("acts_as_list ")),
        "the class body calls acts_as_list:\n{option}"
    );
}
