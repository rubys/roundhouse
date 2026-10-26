# frozen_string_literal: true

# Replaced per app when `lib/tasks/*.rake` prepends onto
# `ActiveRecord::Tasks::DatabaseTasks`. The stub keeps a tree with no
# such file booting: `load_tasks` still calls `install!`, and a task
# kind this runtime does not know calls `run`.
module AppRakeTasks
  def self.install!
    nil
  end

  def self.run(_name)
    nil
  end
end
