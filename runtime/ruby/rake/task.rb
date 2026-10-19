# frozen_string_literal: true

# The task list `db:schema:load` and its prerequisites walk. A linked
# list, not a Hash: the runtime has no typed map from a task name to a
# task, and `invoke` only needs to find one by name.
module Rake
  class Task
    def self.define(name, prerequisites, kind)
      task = Task.new(name, prerequisites, kind)
      task.chain(@head)
      @head = task
      task
    end

    def self.[](name)
      find(name)
    end

    def self.task_defined?(name)
      !find(name).nil?
    end

    def self.find(name)
      current = @head
      while !current.nil?
        return current if current.name == name
        current = current.rest
      end
      nil
    end

    def initialize(name, prerequisites, kind)
      @name = name
      @prerequisites = prerequisites
      @kind = kind
      @invoked = false
      @rest = nil
    end

    def chain(rest)
      @rest = rest
      self
    end

    def rest
      @rest
    end

    def name
      @name
    end

    def reenable
      @invoked = false
      nil
    end

    def all_prerequisite_tasks
      out = []
      collect_prerequisites(out)
      out
    end

    # Walk prerequisite names. Indexing an Array of tasks would dispatch
    # `Task#[]` (the class lookup copied onto the instance) instead of
    # `Array#[]`.
    def collect_prerequisites(out)
      deps = @prerequisites
      i = 0
      while i < deps.length
        dep = deps[i].to_s
        i += 1
        pre = Rake::Task[dep]
        next if pre.nil?
        pre.collect_prerequisites(out)
        out << pre
      end
      nil
    end

    def invoke
      return nil if @invoked == true
      deps = @prerequisites
      i = 0
      while i < deps.length
        dep = deps[i].to_s
        i += 1
        pre = Rake::Task[dep]
        next if pre.nil?
        pre.invoke
      end
      @invoked = true
      ActiveRecord::Tasks::DatabaseTasks.dispatch(@kind)
      nil
    end
  end
end
