//! One synthetic contract shared by the interpreted and native output lanes.

pub const CONCERN: &str = r#"
module WindowSettings
  extend ActiveSupport::Concern
  class_methods do
    def configure_window(**opts)
      @window_options = opts
    end
    def window_options
      @window_options || {}
    end
  end
end
"#;

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write("app/controllers/concerns/window_settings.rb", CONCERN)
        .write(
            "app/controllers/concerns/wrapped_settings.rb",
            r#"
module WrappedSettings
  extend ActiveSupport::Concern
  include WindowSettings
  def marker
    :wrapper
  end
end
"#,
        )
        .write(
            "app/controllers/month_controller.rb",
            r#"
class MonthController < ApplicationController
  include WrappedSettings
  configure_window mode: :month, days: 3
  def snapshot
    self.class.window_options
  end
  def replace_window
    self.class.configure_window opts: "runtime", days: -9
    self.class.window_options
  end
end
"#,
        )
        .write(
            "app/controllers/year_controller.rb",
            r#"
class YearController < ApplicationController
  include WindowSettings
  configure_window mode: :year, days: 0
end
"#,
        )
        .write(
            "app/controllers/child_controller.rb",
            "class ChildController < MonthController\nend\n",
        )
        .write(
            "app/controllers/sibling_controller.rb",
            "class SiblingController < MonthController\nend\n",
        )
}

/// No nonempty write or value-bearing call site can supply a guessed type.
pub fn empty_overlay() -> super::emit_and_run::Overlay {
    overlay()
        .write("app/controllers/month_controller.rb", "class MonthController < ApplicationController\n include WindowSettings\n configure_window\nend\n")
        .write("app/controllers/year_controller.rb", "class YearController < ApplicationController\nend\n")
}

pub const EMPTY_ASSERTIONS: &str = r#"
require_relative "app/controllers/month_controller"
require_relative "app/controllers/child_controller"
raise "empty boot value" unless MonthController.window_options == {}
raise "empty boot identity" unless MonthController.window_options.equal?(MonthController.window_options)
raise "empty inherited value" unless ChildController.window_options == {}
raise "empty unset allocation" if ChildController.window_options.equal?(ChildController.window_options)
ChildController.configure_window
raise "empty runtime value" unless ChildController.window_options == {}
raise "empty runtime identity" unless ChildController.window_options.equal?(ChildController.window_options)
raise "empty receiver isolation" if ChildController.window_options.equal?(MonthController.window_options)
puts "finite class configuration contract passed"
"#;

pub const ASSERTIONS: &str = r#"
require_relative "app/controllers/month_controller"
require_relative "app/controllers/year_controller"
require_relative "app/controllers/child_controller"
require_relative "app/controllers/sibling_controller"
raise "month" unless MonthController.window_options == {mode: :month, days: 3}
raise "year" unless YearController.window_options == {mode: :year, days: 0}
raise "consumer" unless MonthController.new.snapshot == {mode: :month, days: 3}
raise "wrapper instance method" unless MonthController.new.marker == :wrapper
raise "identity" unless MonthController.window_options.equal?(MonthController.window_options)
raise "inherited class ivar" unless ChildController.window_options == {}
raise "unset identity" if ChildController.window_options.equal?(ChildController.window_options)
ChildController.window_options[:leak] = 7
raise "default allocation" unless ChildController.window_options == {}
ChildController.configure_window mode: :year, days: 0
raise "child override" unless ChildController.window_options == {mode: :year, days: 0}
raise "parent leak" unless MonthController.window_options == {mode: :month, days: 3}
raise "sibling leak" unless SiblingController.window_options == {}
old = ChildController.window_options
ChildController.configure_window mode: :week, days: -2
raise "replace identity" if old.equal?(ChildController.window_options)
raise "replace value" unless ChildController.window_options == {mode: :week, days: -2}
raise "previous hash mutated" unless old == {mode: :year, days: 0}
ChildController.configure_window
raise "empty override" unless ChildController.window_options == {}
raise "configured empty identity" unless ChildController.window_options.equal?(ChildController.window_options)
begin
  ChildController.configure_window({mode: :invalid})
  raise "positional hash accepted"
rescue ArgumentError
end
raise "runtime write type or keyword-rest name" unless MonthController.new.replace_window == {opts: "runtime", days: -9}
raise "runtime sibling leak" unless YearController.window_options == {mode: :year, days: 0}
puts "finite class configuration contract passed"
"#;
