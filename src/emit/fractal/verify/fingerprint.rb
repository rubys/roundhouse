# frozen_string_literal: true

# Write what Ruby can observe of the modules a build compiled, as JSON, for
# `fractal verify` to compare between the app and the app with its overlay.
#
#   ruby fingerprint.rb OUTPUT ROOT [--require PATH]... [--file PATH]... [--all]
#
# It requires each --require path, then each compiled --file, both relative
# to ROOT. It then describes every named module that has a method defined in
# a compiled file (every named module with --all): its ancestors, constants
# and instance variable names, and for each of its own methods the owner,
# visibility, parameters, arity, source location and a digest of its
# bytecode with column and node positions removed. A method defined in a
# compiled file also carries the normalised listing, so a difference can be
# shown. Output goes to OUTPUT, not stdout, which the app may write to.

require "json"
require "digest"

output, root, *options = ARGV
abort("usage: fingerprint.rb OUTPUT ROOT [--require PATH]... [--file PATH]... [--all]") unless output && root

root = File.realpath(root)
requires = []
files = []
all = false
until options.empty?
  case (flag = options.shift)
  when "--require" then requires << options.shift
  when "--file" then files << options.shift
  when "--all" then all = true
  else abort("fingerprint.rb: unknown option #{flag}")
  end
end

Dir.chdir(root)
requires.each { |path| require File.expand_path(path, root) }
compiled = files.map { |file| File.realpath(file, root) }
compiled.each { |path| require path }

module Fingerprint
  # Ruby's ISEQ array format: the positions that hold where code was
  # written rather than what it does.
  ISEQ_MAGIC = "YARVInstructionSequence/SimpleDataFormat"
  ISEQ_FIELDS = 14
  POSITION_KEYS = %i[node_id code_location node_ids].freeze
  # Memory addresses differ between processes: in a builtin's C function
  # pointer, and in the inspect output of an object the bytecode embeds.
  ADDRESS = /0x\h{6,}/

  module_function

  def setup(root, compiled)
    @root = root
    @compiled = compiled
  end

  def relative(path)
    return path unless path.is_a?(String) && path.start_with?("#{@root}/")

    path.delete_prefix("#{@root}/")
  end

  MODULE_METHODS = %i[
    name singleton_class? ancestors constants instance_method
    public_instance_methods protected_instance_methods private_instance_methods
  ].to_h { |name| [name, Module.instance_method(name)] }.merge(attached_object: Class.instance_method(:attached_object)).freeze
  KERNEL_METHODS = %i[singleton_class instance_variables is_a?].to_h { |name| [name, Kernel.instance_method(name)] }.freeze

  # Call Module's or Kernel's own `name` on `namespace`: an app module may
  # define a method of the same name for its own purposes.
  def reflect(namespace, name, *arguments)
    (MODULE_METHODS[name] || KERNEL_METHODS.fetch(name)).bind_call(namespace, *arguments)
  end

  # A module named inside an anonymous one (`#<Class:0x…>::Matcher`) has a
  # name that changes with every process, so it is treated as anonymous.
  def name_of(namespace)
    name = reflect(namespace, :name)
    name unless name&.match?(ADDRESS)
  end

  def label_of(namespace)
    return name_of(namespace) || "#<anonymous>" unless reflect(namespace, :singleton_class?)

    attached = reflect(namespace, :attached_object)
    reflect(attached, :is_a?, Module) ? "#<Class:#{label_of(attached)}>" : "#<singleton>"
  end

  # The ISEQ array with column and node positions dropped, the path made
  # relative to the app, and the absolute path removed: code compiled from a
  # string (`module_eval`) has none, code loaded from a file does, and
  # nothing a delegated method does reads it. Line numbers stay, because a
  # rewrite keeps them.
  def normalize(value)
    return value unless value.is_a?(Array)
    return value.map { |element| normalize(element) } unless value[0] == ISEQ_MAGIC

    unless value.size == ISEQ_FIELDS
      abort("fingerprint.rb: this Ruby's ISEQ format has #{value.size} fields, not #{ISEQ_FIELDS}; update fingerprint.rb")
    end
    magic, major, minor, format, misc, label, path, _absolute_path, first_line, type, locals, parameters, catch_table, body = value
    [
      magic, major, minor, format, misc.except(*POSITION_KEYS), label, relative(path), first_line, type, locals, parameters,
      normalize(catch_table), normalize(body),
    ]
  end

  # Constant paths the bytecode looks up relative to its lexical scope
  # (`Foo::Bar`, not `::Foo::Bar`). The scope is not part of the bytecode,
  # so identical bytecode compiled in different scopes can resolve them
  # differently.
  def relative_constants(value, found = [])
    return found unless value.is_a?(Array)

    if value[0].is_a?(Symbol) && value[0].to_s.include?("getconstant") && value[1].is_a?(Array)
      found << value[1].join("::") unless value[1].first == :""
    else
      value.each { |element| relative_constants(element, found) }
    end
    found
  end

  LABEL = /\Alabel_\d+\z/

  # The bytecode of a method and every block, rescue and ensure clause in
  # it, split by the line each instruction belongs to, for a pass that
  # changes the bytecode of the lines it rewrites by design: `fractal verify`
  # then checks that every other line compiles as it did. Labels are named
  # by the line they are placed on, not by offset or by their order in the
  # sequence, so neither an instruction more on one line nor a branch more
  # (a rewrite that writes a conditional) renames the jump targets on
  # another. Within its own line a label also gets its order among that
  # line's labels; seen from another line, or from the header and catch
  # table, it is only "a label on line n", since a rewrite may add labels
  # before it on its own line. Returns line => digest, and a digest of the
  # rest of the shape (each sequence's header and catch table), which
  # leaves out the stack depth a rewritten line may change.
  def by_line(value)
    lines = Hash.new { |hash, line| hash[line] = [] }
    shape = []
    pending = [value]
    until pending.empty?
      sequence = pending.shift
      _magic, _major, _minor, _format, misc, label, path, _absolute_path, first_line, type, locals, parameters, catch_table, body = sequence
      placed = {}
      placed_on = Hash.new(0)
      line = first_line
      body.each do |entry|
        case entry
        when Integer then line = entry
        when Symbol
          if entry.match?(LABEL) && !placed.key?(entry)
            placed[entry] = [line, placed_on[line]]
            placed_on[line] += 1
          end
        end
      end
      # `from` is the line the operand is on, or nil for the shape.
      rename = lambda do |operand, from|
        if operand.is_a?(Symbol) && operand.match?(LABEL)
          on, order = placed[operand]
          if on.nil? then :label_unplaced
          elsif on == from then :"label_#{on}_#{order}"
          else :"label_#{on}"
          end
        elsif operand.is_a?(Array) && operand[0] == ISEQ_MAGIC
          pending << operand
          :child_sequence
        else
          normalize(operand)
        end
      end
      line = first_line
      body.each do |entry|
        case entry
        when Integer then line = entry
        when Array then lines[line] << entry.map { |operand| rename.(operand, line) }
        else lines[line] << rename.(entry, line)
        end
      end
      # Optional positional parameters name the labels their defaults jump
      # to, so they are renamed with the body's labels too.
      parameters = parameters.merge(opt: parameters[:opt].map { |operand| rename.(operand, nil) }) if parameters.is_a?(Hash) && parameters[:opt]
      shape << [label, relative(path), first_line, type, locals, parameters, misc.except(*POSITION_KEYS, :stack_max), catch_table.map { |row| row.map { |operand| rename.(operand, nil) } }]
    end
    digest = ->(object) { Digest::SHA256.hexdigest(object.inspect.gsub(/func_ptr: \d+/, "func_ptr: _").gsub(ADDRESS, "0x_")) }
    [lines.to_h { |line, instructions| [line.to_s, digest.(instructions)] }, digest.(shape)]
  end

  # `namespace`'s own definition of `name`, past anything prepended to it.
  def own_method(namespace, name)
    method = reflect(namespace, :instance_method, name)
    method = method.super_method while method && !method.owner.equal?(namespace)
    method
  end

  def describe(method, visibility)
    file, line = method.source_location
    entry = {
      "owner" => label_of(method.owner),
      "visibility" => visibility.to_s,
      "parameters" => method.parameters.inspect,
      "arity" => method.arity,
      "original_name" => method.original_name.to_s,
      "source_location" => file && "#{relative(file)}:#{line}",
    }
    iseq = RubyVM::InstructionSequence.of(method)
    if iseq
      raw = iseq.to_a
      listing = normalize(raw).inspect.gsub(/func_ptr: \d+/, "func_ptr: _").gsub(ADDRESS, "0x_")
      entry["bytecode"] = Digest::SHA256.hexdigest(listing)
      entry["evaluated"] = iseq.absolute_path.nil?
      entry["relative_constants"] = relative_constants(raw).uniq.sort
      if file && @compiled.include?(File.expand_path(file))
        entry["listing"] = listing
        entry["lines"], entry["shape"] = by_line(raw)
        entry["last_line"] = raw[4][:code_location]&.[](2)
      end
    else
      # A C function, an attribute reader or writer, or another method
      # without bytecode.
      entry["bytecode"] = "none"
    end
    entry
  end

  def own_methods(namespace)
    %i[public protected private].to_h { |visibility| [visibility, reflect(namespace, :"#{visibility}_instance_methods", false)] }
  end

  def compiled_here?(namespace)
    [namespace, reflect(namespace, :singleton_class)].any? do |owner|
      own_methods(owner).values.flatten.any? do |name|
        file, = own_method(owner, name)&.source_location
        file && @compiled.include?(File.expand_path(file))
      end
    end
  end

  def module_entry(namespace)
    {
      "kind" => reflect(namespace, :is_a?, Class) ? "class" : "module",
      "ancestors" => reflect(namespace, :ancestors).map { |ancestor| label_of(ancestor) },
      "singleton_ancestors" => reflect(reflect(namespace, :singleton_class), :ancestors).map { |ancestor| label_of(ancestor) },
      "constants" => reflect(namespace, :constants, false).map(&:to_s).sort,
      "instance_variables" => reflect(namespace, :instance_variables).map(&:to_s).sort,
    }
  end

  def run(all:)
    modules = {}
    methods = {}
    anonymous = 0
    ObjectSpace.each_object(Module) do |namespace|
      next if reflect(namespace, :singleton_class?)

      name = name_of(namespace)
      unless name
        anonymous += 1 if compiled_here?(namespace)
        next
      end
      next unless all || compiled_here?(namespace)

      modules[name] = module_entry(namespace)
      { "#" => namespace, "." => reflect(namespace, :singleton_class) }.each do |separator, owner|
        own_methods(owner).each do |visibility, names|
          names.each do |method_name|
            method = own_method(owner, method_name)
            methods["#{name}#{separator}#{method_name}"] = describe(method, visibility) if method
          end
        end
      end
    end
    { "ruby" => RUBY_DESCRIPTION, "anonymous_modules" => anonymous, "modules" => modules.sort.to_h, "methods" => methods.sort.to_h }
  end
end

Fingerprint.setup(root, compiled)
File.write(output, JSON.generate(Fingerprint.run(all: all)))
