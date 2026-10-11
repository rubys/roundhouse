# frozen_string_literal: true

# Research prototype, not a compiler admission path. Only run trusted fixtures.
require "prism"
require "native_define_method_observer"

module BootToCore
  class Unsupported < StandardError; end
  Record = Data.define(:owner, :name, :node, :binding, :origin, :definition, :reason, :scopes)
  Cell = Struct.new(:binding, :name, :written)

  class << self
    attr_accessor :active
    attr_reader :input_spec

    def capture
      previous = active
      self.active = true
      yield
    ensure
      self.active = previous
    end

    def input(roots:, signatures: [])
      raise Unsupported, "only one input declaration per process" if @input_spec
      @input_spec = { roots: roots, signatures: signatures }
    end

    def records
      @records ||= {}
    end

    def events
      @events ||= []
    end

    def remember(record)
      (@history ||= []) << record
      (@definitions ||= {})[record.definition] = record
      records[[record.owner, record.name]] = record
    end

    def source_record(method)
      definition = method.is_a?(Method) ? method.unbind : method
      captured = (@definitions || {})[definition] || (@history || []).reverse.find do |record|
        # MRI's included-module binding context also changes unbound equality.
        # Normalize the saved definition to the SAME receiver before comparing,
        # without calling the body or conflating two Proc closure environments.
        next false unless method.is_a?(Method) && record.definition.owner == method.owner
        record.definition.bind(method.receiver).unbind == definition
      end
      return captured if captured
      inherited_body = definition.owner.ancestors.drop(1).any? do |ancestor|
        next false unless own_methods(ancestor).include?(definition.name)
        inherited = ancestor.instance_method(definition.name)
        inherited.source_location == definition.source_location && inherited.original_name == definition.original_name
      end
      if inherited_body
        raise Unsupported, "inherited visibility/copy wrapper requires captured provenance"
      end
      path, line = definition.source_location
      raise Unsupported, "#{definition.name}: no captured or readable definition" unless path && File.file?(path)
      candidates = []
      scopes_by_node = {}
      walk_lexical(source_tree(path)) do |node, scopes|
        if node.is_a?(Prism::DefNode) && node.receiver.nil? && node.name == definition.original_name && node.location.start_line == line
          candidates << node
          scopes_by_node[node] = scopes
        end
        # MRI accessors have a source location but no DefNode. Recover only
        # literal, ordinary accessor declarations, not arbitrary native bodies.
        if node.is_a?(Prism::CallNode) && %i[attr_reader attr_writer attr_accessor].include?(node.name) && node.location.start_line == line
          args = node.arguments&.arguments || []
          next unless args.all? { |arg| arg.is_a?(Prism::SymbolNode) }
          args.each do |arg|
            name = arg.unescaped
            next unless name.match?(/\A[a-zA-Z_]\w*\z/)
            bodies = []
            bodies << "def #{name}; @#{name}; end" unless node.name == :attr_writer
            bodies << "def #{name}=(value); @#{name} = value; end" unless node.name == :attr_reader
            bodies.each do |body|
              candidate = parse(body).statements.body.first
              candidates << candidate if candidate.name == definition.original_name
            end
          end
        end
      end
      raise Unsupported, "#{definition.name}: ambiguous or missing source definition at #{path}:#{line}" unless candidates.length == 1
      scopes = scopes_by_node[candidates.first]
      scopes = nil unless scopes&.first.equal?(definition.owner)
      Record.new(definition.owner, definition.name, candidates.first, nil, "source", definition, nil, scopes)
    rescue Unsupported => error
      Record.new(definition.owner, definition.name, nil, nil, "opaque_source", definition, error.message, nil)
    end

    # Reconstruct lexical declarations only under the manifest's no-namespace-
    # rebinding precondition (definition creation through export). This is not
    # original CREF capture. Qualified declarations add only the named module.
    def walk_lexical(node, scopes = [], &block)
      return unless node
      if node.is_a?(Prism::ClassNode) || node.is_a?(Prism::ModuleNode)
        path = node.constant_path.location.slice
        parts = path.delete_prefix("::").split("::")
        bases = path.start_with?("::") ? [Object] : scopes + [Object]
        base = bases.find { |scope| scope.const_defined?(parts.first, false) && !scope.autoload?(parts.first, false) }
        parts.each do |name|
          unless base.is_a?(Module) && base.const_defined?(name, false) && !base.autoload?(name, false)
            base = nil
            break
          end
          base = base.const_get(name, false)
        end
        scopes = base.is_a?(Module) ? [base] + scopes : []
      end
      block.call(node, scopes)
      node.compact_child_nodes.each { |child| walk_lexical(child, scopes, &block) }
    end

    def walk(node, &block)
      return unless node
      block.call(node)
      node.compact_child_nodes.each { |child| walk(child, &block) }
    end

    def parse(source)
      result = Prism.parse(source)
      raise Unsupported, result.errors.map(&:message).join("; ") unless result.success?
      result.value
    end

    # Manifest source files must remain unchanged throughout this process.
    def source_tree(path)
      (@source_trees ||= {})[path] ||= parse(File.read(path))
    end

    def record_eval(owner, source, path, first_line)
      # Admission failures describe the observed definitions; they must not
      # interrupt the original eval or unrelated Rails boot operations.
      statements = parse(source).statements.body
      definitions = statements.select { |node| node.is_a?(Prism::DefNode) && node.receiver.nil? }
      events << { kind: "string_eval", owner_id: owner.object_id, names: definitions.map(&:name) }
      definitions.each do |node|
        (@eval_definitions ||= {})[[owner, node.name]] ||= []
        @eval_definitions[[owner, node.name]] << [node, path, first_line + node.location.start_line - 1]
      end
      unless definitions.length == statements.length
        raise Unsupported, "string eval also contains non-instance-method statements (not exported)"
      end
    rescue Unsupported => error
      events << { kind: "opaque_eval", owner_id: owner.object_id, reason: error.message }
      (@opaque_evals ||= []) << [owner, path, first_line, first_line + source.lines.length, error.message]
    end

    def record_added(owner, name)
      definition = owner.instance_method(name)
      (@additions ||= []) << [owner, name, definition]
      (@installations ||= {})[[owner, name, definition]] = true
      pending = (@eval_definitions || {})[[owner, name]] || []
      matches = pending.select { |_, path, line| definition.source_location == [path, line] }
      if matches.length == 1
        pending.delete(matches.first)
        remember(Record.new(owner, name, matches.first[0], nil, "string_eval", definition, nil, nil))
      elsif matches.length > 1
        remember(Record.new(owner, name, nil, nil, "opaque_eval", definition, "ambiguous same-site eval definitions", nil))
      elsif (opaque = (@opaque_evals || []).reverse.find do |entry|
        location = definition.source_location
        entry[0] == owner && location && location[0] == entry[1] && (entry[2]..entry[3]).cover?(location[1])
      end)
        remember(Record.new(owner, name, nil, nil, "opaque_eval", definition, opaque[4], nil))
      end
    end

    def record_proc(owner, name, block, definition = owner.instance_method(name))
      path, line = block.source_location
      raise Unsupported, "Proc has no readable source" unless path && File.file?(path)
      candidates = []
      scopes_by_node = {}
      location = RubyVM::InstructionSequence.of(block)&.to_a&.dig(4, :code_location)
      walk_lexical(source_tree(path)) do |node, scopes|
        if node.is_a?(Prism::BlockNode) && node.location.start_line == line
          span = node.location
          next if location && location != [span.start_line, span.start_column, span.end_line, span.end_column]
          candidates << node
          scopes_by_node[node] = scopes
        end
      end
      raise Unsupported, "ambiguous or missing block source at #{path}:#{line}" unless candidates.length == 1
      remember(Record.new(owner, name.to_sym, candidates.first, block.binding, "define_method", definition, nil, scopes_by_node[candidates.first]))
    rescue Unsupported => error
      remember(Record.new(owner, name.to_sym, nil, nil, "opaque_proc", definition, error.message, nil))
    end

    def record_copy(owner, name, source, target = owner.instance_method(name))
      # Source identity is checked BEFORE installation. Ruby changes owner and
      # equality when copying an UnboundMethod; the new definition is separate.
      events << { kind: "method_copy", source_owner_id: source.definition.owner.object_id,
                  target_owner_id: target.owner.object_id, name: name,
                  original_name: target.original_name, same_definition: target == source.definition }
      remember(Record.new(owner, name.to_sym, source.node, source.binding, "method_copy", target, source.reason, source.scopes))
    end

    def additions
      @additions ||= []
    end

    def recorded_definition?(definition)
      (@definitions || {}).key?(definition)
    end

    def record_defined(owner, name, callable)
      definition = owner.instance_method(name)
      # Reentrant callbacks can install a different method before the outer
      # native call returns. Never overwrite a known final definition with
      # the outer call's stale callable/binding.
      return if recorded_definition?(definition)
      installed = (@installations || {}).key?([owner, name, definition])
      code = RubyVM::InstructionSequence.of(definition)&.to_a
      callable_code = RubyVM::InstructionSequence.of(callable)&.to_a
      unless installed && code == callable_code
        remember(Record.new(owner, name, nil, nil, "opaque_installation", definition,
          "no matching installation snapshot; callback replacement or unobserved method_added", nil))
        return
      end
      if callable.is_a?(Proc)
        record_proc(owner, name, callable, definition)
      elsif callable.is_a?(Method) || callable.is_a?(UnboundMethod)
        record_copy(owner, name, source_record(callable), definition)
      end
    rescue NameError
      events << { kind: "removed_during_installation", owner_id: owner.object_id, name: name }
    end

    def admitted(record)
      raise Unsupported, "#{record.owner.name || '(anonymous module)'}##{record.name}: #{record.reason}" if record.reason
      record
    end

    # Reify immutable collection nodes once, preserving sharing and freezing.
    # Mutable descendants, cycles and special Hash defaults are not snapshots.
    def immutable_literal(value, stack = [])
      if [NilClass, TrueClass, FalseClass, Integer, Symbol].include?(value.class) ||
          (value.instance_of?(String) && value.frozen?)
        return value.inspect
      end
      unless value.frozen? && (value.instance_of?(Array) || value.instance_of?(Hash))
        raise Unsupported, "#{value.class} is not an immutable scalar/collection"
      end
      if value.instance_of?(Hash) && (value.default_proc || !value.default.nil? || value.compare_by_identity?)
        raise Unsupported, "Hash default/identity semantics are not admitted"
      end
      raise Unsupported, "cyclic immutable collection" if stack.any? { |ancestor| ancestor.equal?(value) }
      return "BootState::VALUE#{@immutable_values.fetch(value)[0]}" if @immutable_values.key?(value)
      stack = stack + [value]
      literal = if value.instance_of?(Array)
        "[#{value.map { |entry| immutable_literal(entry, stack) }.join(', ')}]"
      else
        "{#{value.map { |key, entry| "#{immutable_literal(key, stack)} => #{immutable_literal(entry, stack)}" }.join(', ')}}"
      end
      index = @immutable_values.length
      @immutable_values[value] = [index, "#{literal}.freeze"]
      "BootState::VALUE#{index}"
    end

    # Binding object identity is NOT lexical-slot identity. Probe aliases by a
    # reversible write, in this single-threaded, trusted research process only.
    def cell_for(binding, name, cells)
      cells.each_with_index do |cell, index|
        next unless cell.name == name
        previous = cell.binding.local_variable_get(name)
        marker = Object.new
        begin
          cell.binding.local_variable_set(name, marker)
          return index if binding.local_variable_get(name).equal?(marker)
        ensure
          cell.binding.local_variable_set(name, previous)
        end
      end
      value = binding.local_variable_get(name)
      immutable_literal(value)
      cells << Cell.new(binding, name, false)
      cells.length - 1
    end

    def parameters(record)
      node = record.node.parameters
      if node.is_a?(Prism::BlockParametersNode)
        raise Unsupported, "block-local parameter declarations are not admitted" unless node.locals.empty?
        node = node.parameters
      end
      return nil unless node
      unless node.is_a?(Prism::ParametersNode)
        raise Unsupported, "implicit block parameters are not admitted"
      end
      node
    end

    def rewrite(node, record, cells, depth = 0)
      if node.is_a?(Prism::ConstantReadNode)
        scope = record.scopes&.find { |entry| entry.const_defined?(node.name, false) }
        raise Unsupported, "constant #{node.name}: no proven direct lexical binding" unless scope
        raise Unsupported, "constant #{node.name}: autoload is not admitted" if scope.autoload?(node.name, false)
        value = scope.const_get(node.name, false)
        begin
          literal = immutable_literal(value)
        rescue Unsupported => error
          raise Unsupported, "constant #{node.name}: #{error.message}"
        end
        # Namespace paths must never have been rebound since definition creation;
        # scalar bindings must be sealed after boot. No original CREF is captured.
        kind = value.instance_of?(Array) || value.instance_of?(Hash) ? "immutable_constant" : "scalar_constant"
        events << { kind: kind, lexical_owner: scope.name, name: node.name, type: value.class.name }
        return "(#{literal})"
      end
      forbidden = [Prism::DefNode, Prism::ClassNode,
                   Prism::ModuleNode, Prism::BreakNode, Prism::NextNode,
                   Prism::DefinedNode, Prism::ConstantPathNode, Prism::ConstantWriteNode, Prism::ConstantPathWriteNode,
                   Prism::ClassVariableReadNode, Prism::ClassVariableWriteNode,
                   Prism::GlobalVariableReadNode, Prism::GlobalVariableWriteNode]
      raise Unsupported, "body contains #{node.class}" if forbidden.any? { |kind| node.is_a?(kind) }
      if node.is_a?(Prism::CallNode) && %i[eval class_eval module_eval define_method send require load binding const_set remove_const autoload __method__ __callee__ equal? object_id].include?(node.name)
        raise Unsupported, "late dynamic operation #{node.name}"
      end
      if node.respond_to?(:depth) && node.respond_to?(:name) && node.depth > depth
        raise Unsupported, "outer local without a captured binding" unless record.binding
        index = cell_for(record.binding, node.name, cells)
        cell = "BootState::CELL#{index}"
        case node
        when Prism::LocalVariableReadNode
          return "#{cell}.read"
        when Prism::LocalVariableWriteNode
          cells[index].written = true
          return "#{cell}.write(#{rewrite(node.value, record, cells, depth)})"
        when Prism::LocalVariableOperatorWriteNode
          cells[index].written = true
          return "#{cell}.write(#{cell}.read #{node.binary_operator} (#{rewrite(node.value, record, cells, depth)}))"
        else
          raise Unsupported, "unsupported captured-local operation #{node.class}"
        end
      end
      # Prism offsets are bytes, including when text contains non-ASCII Ruby.
      text = node.location.slice.b.dup
      depth += 1 if node.is_a?(Prism::BlockNode) || node.is_a?(Prism::LambdaNode)
      node.compact_child_nodes.sort_by { |child| -child.location.start_offset }.each do |child|
        offset = child.location.start_offset - node.location.start_offset
        text[offset, child.location.length] = rewrite(child, record, cells, depth).b
      end
      text.force_encoding(Encoding::UTF_8)
    end

    # A declared instance-method cut, not a trace-derived whole-program graph.
    # Resolve effective definitions, then follow every direct self-call in every
    # branch. Shadowed methods are needed only when a reachable super uses them.
    def select_roots(roots)
      selected = roots.keys.to_h { |owner| [owner, []] }
      state = roots.keys.to_h { |owner| [owner, []] }
      @omitted_initializers = []
      pending = roots.flat_map do |owner, names|
        names.map do |name|
          unless owner.public_instance_methods.include?(name.to_sym)
            raise Unsupported, "root #{owner.name}##{name} is not public"
          end
          [owner, owner.instance_method(name)]
        end
      end
      roots.each_key do |owner|
        next unless owner.is_a?(Class)
        initializer = owner.instance_method(:initialize)
        chain = owner.ancestors.take_while { |ancestor| ancestor != owner.superclass }
        if chain.include?(initializer.owner)
          pending << [owner, initializer]
        elsif initializer.owner != BasicObject
          @omitted_initializers << { receiver: owner.name, initializer_owner: initializer.owner.name,
            instance_variables: state.fetch(owner) }
        end
      end
      visited = {}
      loop do
        if pending.empty?
          boundary = @omitted_initializers.find { |entry| !entry.fetch(:instance_variables).empty? }
          break unless boundary
          @omitted_initializers.delete(boundary)
          receiver = roots.keys.find { |owner| owner.name == boundary.fetch(:receiver) }
          pending << [receiver, receiver.instance_method(:initialize)]
        end
        receiver, method = pending.shift
        key = [receiver, method.owner, method.name]
        next if visited[key]
        visited[key] = true
        chain = receiver.ancestors.take_while { |ancestor| ancestor != Object }
        unless chain.include?(method.owner)
          raise Unsupported, "#{receiver.name}##{method.name}: inherited runtime outside the declared cut"
        end
        selected[method.owner] ||= []
        selected[method.owner] |= [method.name]
        record = admitted(source_record(method))
        walk(record.node) do |node|
          if node.class.name.start_with?("Prism::InstanceVariable") && node.respond_to?(:name)
            state.fetch(receiver) << node.name unless state.fetch(receiver).include?(node.name)
          end
          if node.is_a?(Prism::CallNode) && (node.receiver.nil? || node.receiver.is_a?(Prism::SelfNode))
            pending << [receiver, receiver.instance_method(node.name)]
          elsif node.is_a?(Prism::SuperNode) || node.is_a?(Prism::ForwardingSuperNode)
            if record.node.is_a?(Prism::DefNode) && record.node.name != method.name
              raise Unsupported, "aliased super is not preserved by the current Roundhouse Ruby emitter"
            end
            parent = method.super_method
            raise Unsupported, "missing super for #{receiver.name}##{method.name}" unless parent
            # The projected class still inherits Object's native default init.
            pending << [receiver, parent] unless parent.owner == BasicObject && parent.name == :initialize
          end
        end
      end
      selected
    rescue NameError => error
      raise Unsupported, "unresolved self-call or undef barrier: #{error.message}"
    end

    def export(owners, slices: nil, roots: nil)
      cells = []
      @immutable_values = {}.compare_by_identity
      inventory = []
      selected = {}
      owners.each do |owner|
        unless owner.is_a?(Module) && owner.name&.match?(/\A[A-Z]\w*(?:::[A-Z]\w*)*\z/) &&
            (roots || (owner.is_a?(Class) && owner.superclass == Object))
          raise Unsupported, "only named classes/modules (Object subclasses unless an explicit root cut) are admitted"
        end
        raise Unsupported, "prepend is not admitted" unless owner.ancestors.first == owner
        unless slices || roots || (owner.singleton_methods(false).empty? && owner.instance_variables.empty? && owner.constants(false).empty?)
          raise Unsupported, "class-side state or methods are not exported"
        end
        next if roots
        chain = owner.ancestors.take_while { |ancestor| ancestor != Object }
        names = slices ? slices.fetch(owner).map(&:to_sym) : chain.flat_map { |ancestor| own_methods(ancestor) }.uniq
        chain.each do |ancestor|
          selected[ancestor] ||= []
          selected[ancestor] |= own_methods(ancestor) & names
        end
        names.each do |name|
          raise Unsupported, "missing slice method #{owner.name}##{name}" unless chain.any? { |ancestor| own_methods(ancestor).include?(name) }
          begin
            owner.instance_method(name)
          rescue NameError
            raise Unsupported, "undef barrier for #{owner.name}##{name} is not exported"
          end
        end
      end
      selected = select_roots(roots) if roots
      selected.delete_if { |owner, names| names.empty? && !owners.include?(owner) }
      labels = selected.keys.each_with_index.to_h { |owner, index| [owner, owners.include?(owner) ? owner.name : "BootOwner#{index}"] }
      # Keep separate module owners and their projected ancestor order. Bodies
      # are NEVER moved into the class, where super would skip the next module.
      classes = selected.keys.sort_by { |owner| owner.ancestors.length }.map do |owner|
        raise Unsupported, "prepend is not admitted" unless owner.ancestors.first == owner
        aliases = []
        methods = selected.fetch(owner).sort.map do |name|
          record = admitted(source_record(owner.instance_method(name)))
          unless owner.instance_method(name) == record.definition
            raise Unsupported, "#{labels.fetch(owner)}##{name} changed after its captured definition"
          end
          params = parameters(record)
          args = params ? rewrite(params, record, cells) : ""
          raise Unsupported, "empty method body" unless record.node.body
          visibility = owner.private_instance_methods(false).include?(name) ? :private : owner.protected_instance_methods(false).include?(name) ? :protected : :public
          inventory << { owner: labels.fetch(owner), original_owner: owner.name, method: name, origin: record.origin, parameters: owner.instance_method(name).parameters, visibility: visibility }
          has_super = false
          walk(record.node.body) { |node| has_super ||= node.is_a?(Prism::SuperNode) || node.is_a?(Prism::ForwardingSuperNode) }
          if has_super && record.node.is_a?(Prism::DefNode) && record.node.name != name
            original = record.node.name
            same_body = selected.fetch(owner).include?(original) && owner.instance_method(original) == record.definition
            raise Unsupported, "renamed super requires its unchanged original in the slice" unless same_body
            aliases << "  alias_method #{name.inspect}, #{original.inspect}"
            next nil
          end
          body = rewrite(record.node.body, record, cells)
          "  def #{name}(#{args})\n    #{body}\n  end"
        end.compact
        includes = owner.ancestors.drop(1).take_while { |ancestor| ancestor != Object }.select { |ancestor| selected.key?(ancestor) }.reverse.map { |ancestor| "  include #{labels.fetch(ancestor)}" }
        visibility = %i[private protected].filter_map do |kind|
          names = selected.fetch(owner) & owner.public_send("#{kind}_instance_methods", false)
          "  #{kind} #{names.map(&:inspect).join(', ')}" unless names.empty?
        end
        kind = owners.include?(owner) && owner.is_a?(Class) ? "class" : "module"
        "#{kind} #{labels.fetch(owner)}\n#{(includes + methods + aliases + visibility).join("\n")}\nend"
      end
      namespaces = {}
      owners.each do |owner|
        parts = owner.name.split("::")
        namespace = Object
        parts[0...-1].each_with_index do |name, index|
          namespace = namespace.const_get(name, false)
          path = parts.take(index + 1).join("::")
          namespaces[path] ||= "#{namespace.is_a?(Class) ? 'class' : 'module'} #{path}\nend"
        end
      end
      declarations = cells.each_with_index.map do |cell, index|
        value = cell.binding.local_variable_get(cell.name)
        writer = cell.written ? "\n  def write(value)\n    @value = value\n  end" : ""
        "class BootCell#{index}\n  def initialize\n    @value = #{immutable_literal(value)}\n  end\n  def read\n    @value\n  end#{writer}\nend"
      end
      values = @immutable_values.values.map { |index, literal| "  VALUE#{index} = #{literal}\n" }.join
      state = "class BootState\n#{values}" + cells.each_index.map { |index| "  CELL#{index} = BootCell#{index}.new\n" }.join + "end"
      [(["# frozen_string_literal: true"] + declarations + [state] + namespaces.values + classes).join("\n\n") + "\n", inventory, cells.length]
    end

    def own_methods(owner)
      owner.public_instance_methods(false) | owner.protected_instance_methods(false) | owner.private_instance_methods(false)
    end

    def omitted_initializers
      @omitted_initializers || []
    end
  end

  module Hooks
    def method_added(name)
      BootToCore.record_added(self, name) if BootToCore.active
      super
    end

    def alias_method(name, original)
      source = BootToCore.source_record(instance_method(original)) if BootToCore.active
      before = BootToCore.additions.length
      result = super
      if source
        definition = BootToCore.additions.drop(before).find { |owner, method, _| owner == self && method == name.to_sym }&.last
        if definition && !BootToCore.recorded_definition?(definition)
          if definition == source.definition
            BootToCore.record_copy(self, name, source, definition)
          else
            BootToCore.remember(Record.new(self, name.to_sym, nil, nil, "opaque_alias", definition,
              "alias installation identity differs from its saved source", nil))
          end
        end
      end
      result
    end
  end
end

Module.prepend(BootToCore::Hooks)
NativeDefineMethodObserver.collector = ->(owner, name, callable) do
  BootToCore.record_defined(owner, name, callable) if BootToCore.active
end

# Do not wrap class_eval/module_eval: an extra Ruby frame changes which locals
# their string bodies can see (FFI's invoker is a real-world counterexample).
# Observe native compilation instead, then save the installed definition via
# method_added. Never request event.binding here: MRI 3.4.8 can crash when an
# eval creates a Proc after that binding has been materialized.
BootToCore::EVAL_TRACE = TracePoint.new(:script_compiled) do |event|
  if BootToCore.active && event.eval_script && event.self.is_a?(Module)
    iseq = event.instruction_sequence
    BootToCore.record_eval(event.self, event.eval_script, iseq.path, iseq.first_lineno)
  end
end
BootToCore::EVAL_TRACE.enable
