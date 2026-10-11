require 'minitest/autorun'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'rbconfig'
require 'json'

class RhMaterializeTest < Minitest::Test
  ROOT = File.expand_path('..', __dir__)

  def setup
    @dir = Dir.mktmpdir('rh-materialize-')
  end

  def teardown
    FileUtils.remove_entry(@dir)
  end

  def invoke(*args)
    Open3.capture3(RbConfig.ruby, File.join(ROOT, 'bin/rh'), 'materialize', *args, chdir: '/')
  end

  def test_cli_guards_do_not_execute_manifest_or_overwrite_paths
    manifest = File.join(@dir, 'input.rb')
    marker = File.join(@dir, 'booted')
    File.write(manifest, "File.write(#{marker.inspect}, 'booted')\n")
    output = File.join(@dir, 'output')
    out, err, status = invoke('--help', manifest, '-o', output)
    assert status.success?, err
    assert_includes out, '--trust-boot'
    refute File.exist?(marker)
    _, _, status = invoke(manifest, '-o', output)
    refute status.success?
    refute File.exist?(marker)
    refute File.exist?(output)
    File.write(output, 'keep this')
    _, err, status = invoke('--trust-boot', manifest, '-o', output)
    refute status.success?
    assert_includes err, 'output already exists'
    assert_equal 'keep this', File.read(output)
    refute File.exist?(marker)
    File.delete(output)
    File.symlink(File.join(@dir, 'missing'), output)
    _, _, status = invoke('--trust-boot', manifest, '-o', output)
    refute status.success?
    assert File.symlink?(output)
    refute File.exist?(marker)
  end

  def test_unknown_generator_private_helpers_all_branches_and_effective_override
    manifest = File.join(@dir, 'input.rb')
    File.write(manifest, <<~RUBY)
      # frozen_string_literal: true
      invoker = 41
      local_scope = Module.new
      local_scope.module_eval('define_method(:read) { invoker }')
      raise 'instrumentation changed caller locals' unless Object.new.extend(local_scope).read == 41
      module ShadowedGenerator
        def answer(flag)
          eval('unsupported but shadowed')
        end
      end
      module Tail
        def layered(value)
          value * 3 + 5
        end
      end
      module Head
        include Tail
        def layered(value)
          super(value) - 11
        end
      end
      class UnexpectedName
        include ShadowedGenerator
        include Head
        def initialize
          @bias = 11
        end
      end
      module AnotherUnknownGenerator
        def self.install(owner)
          owner.module_eval("def answer(flag); if flag; self.first(6); else; second(-4); end; end\ndef first(value); value * 7 + @bias; end\ndef second(value); first(value) - 11; end")
        end
      end
      BootToCore.capture { AnotherUnknownGenerator.install(UnexpectedName) }
      UnexpectedName.send(:private, :first, :second)
      BootToCore.input(roots: { UnexpectedName => [:answer, :layered] })
    RUBY
    output = File.join(@dir, 'app')
    _, err, status = invoke('--trust-boot', manifest, '-o', output)
    assert status.success?, err
    core = File.join(output, 'lib/core.rb')
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', core, '-rjson', '-e', <<~RUBY)
      object = UnexpectedName.new
      puts JSON.generate([object.answer(true), object.answer(false), object.layered(7), object.layered(-4), object.respond_to?(:first), object.respond_to?(:second)])
    RUBY
    assert status.success?, err
    assert_equal [53, -28, 15, -18, false, false], JSON.parse(out)
    refute File.read(core).include?('AnotherUnknownGenerator')
    refute File.read(core).include?('eval(')
  end

  def test_opaque_boot_definitions_are_ignored_only_when_unreachable
    manifest = File.join(@dir, 'opaque.rb')
    File.write(manifest, <<~RUBY)
      class OpaqueProduct
        def good(value); value * 7 - 11; end
        def entry(flag); flag ? good(4) : native; end
      end
      BootToCore.capture do
        OpaqueProduct.define_method(:native, Kernel.instance_method(:puts))
        OpaqueProduct.module_eval('SIDE = 19; def mixed; 41; end; def from_mixed; SIDE; end')
      end
      raise 'boot did not finish' unless OpaqueProduct::SIDE == 19
      BootToCore.input(roots: { OpaqueProduct => [ENV.fetch('CUT').to_sym] })
    RUBY
    output = File.join(@dir, 'good')
    _, err, status = Open3.capture3({ 'CUT' => 'good' }, RbConfig.ruby,
      File.join(ROOT, 'bin/rh'), 'materialize', '--trust-boot', manifest, '-o', output)
    assert status.success?, err
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'),
      '-e', 'puts OpaqueProduct.new.good(-4)')
    assert status.success?, err
    assert_equal '-39', out.strip
    mixed = File.join(@dir, 'mixed')
    _, err, status = Open3.capture3({ 'CUT' => 'mixed' }, RbConfig.ruby,
      File.join(ROOT, 'bin/rh'), 'materialize', '--trust-boot', manifest, '-o', mixed)
    assert status.success?, err
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(mixed, 'lib/core.rb'),
      '-e', 'puts OpaqueProduct.new.mixed')
    assert status.success?, err
    assert_equal '41', out.strip
    %w[entry from_mixed].each do |cut|
      refused = File.join(@dir, cut)
      _, err, status = Open3.capture3({ 'CUT' => cut }, RbConfig.ruby,
        File.join(ROOT, 'bin/rh'), 'materialize', '--trust-boot', manifest, '-o', refused)
      refute status.success?, cut
      assert_includes err, 'BootToCore::Unsupported'
      refute_includes err, 'boot did not finish'
      refute File.exist?(refused)
    end
  end

  def test_observation_preserves_visibility_and_explicit_callable
    manifest = File.join(@dir, 'visibility.rb')
    File.write(manifest, <<~RUBY)
      inactive = Module.new do
        private
        define_method(:hidden) { 19 }
      end
      raise 'inactive observer changed visibility' unless inactive.private_instance_methods(false).include?(:hidden)
      BootToCore.capture do
        class VisibleProduct
          def self.method_added(name)
            if name == :helper
              raise 'callback saw public helper' unless private_instance_methods(false).include?(name)
            end
            super
          end
          def entry(value); helper(value); end
          private
          define_method(:helper) { |value| value * 3 + 5 }
        end
      end
      class ProcProduct; end
      explicit = proc { |value| value * 7 - 11 }
      BootToCore.capture do
        ProcProduct.define_method(:entry, explicit) { |value| 101 + value }
      end
      class ReentrantProcProduct
        def self.factory(bias)
          proc { |value| value * 7 + bias }
        end
        def self.method_added(name)
          if name == :entry && !@replaced
            @replaced = true
            define_method(:entry, factory(-29))
          end
          super
        end
      end
      BootToCore.capture { ReentrantProcProduct.define_method(:entry, ReentrantProcProduct.factory(13)) }
      raise 'original reentrant result wrong' unless ReentrantProcProduct.new.entry(-4) == -57
      BootToCore.input(roots: { VisibleProduct => [:entry], ProcProduct => [:entry], ReentrantProcProduct => [:entry] })
    RUBY
    output = File.join(@dir, 'visible')
    _, err, status = invoke('--trust-boot', manifest, '-o', output)
    assert status.success?, err
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'), '-rjson',
      '-e', 'puts JSON.generate([VisibleProduct.new.entry(-4), VisibleProduct.new.respond_to?(:helper), ProcProduct.new.entry(4), ReentrantProcProduct.new.entry(-4)])')
    assert status.success?, err
    assert_equal [-7, false, 17, -57], JSON.parse(out)
  end

  def test_alias_callback_replacement_preserves_the_final_body
    %w[before after].each do |order|
      manifest = File.join(@dir, "alias-#{order}.rb")
      File.write(manifest, <<~RUBY)
        class AliasProduct
          def source(value); value * 7 - 11; end
          def self.method_added(name)
            super if #{order.inspect} == 'before'
            if name == :entry && !@replaced
              @replaced = true
              define_method(:entry) { |value| value * 3 + 5 }
            end
            super if #{order.inspect} == 'after'
          end
        end
        BootToCore.capture { AliasProduct.alias_method(:entry, :source) }
        raise 'original alias result wrong' unless AliasProduct.new.entry(-4) == -7
        BootToCore.input(roots: { AliasProduct => [:entry] })
      RUBY
      output = File.join(@dir, "alias-#{order}")
      _, err, status = invoke('--trust-boot', manifest, '-o', output)
      assert status.success?, err
      out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'),
        '-e', 'puts AliasProduct.new.entry(-4)')
      assert status.success?, err
      assert_equal '-7', out.strip
    end
  end

  def test_inherited_source_initializer_preserves_stateful_cuts
    manifest = File.join(@dir, 'constructor.rb')
    File.write(manifest, <<~RUBY)
      class StateParent
        def initialize; @bias = 19; end
      end
      class StateChild < StateParent
        def stateful(value); @bias * 7 + value; end
        def pure(value); value * 3 - 11; end
      end
      raise 'original state wrong' unless StateChild.new.stateful(-4) == 129
      BootToCore.input(roots: { StateChild => [ENV.fetch('CUT').to_sym] })
    RUBY
    %w[stateful pure].each do |cut|
      output = File.join(@dir, cut)
      _, err, status = Open3.capture3({ 'CUT' => cut }, RbConfig.ruby,
        File.join(ROOT, 'bin/rh'), 'materialize', '--trust-boot', manifest, '-o', output)
      assert status.success?, err
      if cut == 'stateful'
        out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'),
          '-e', 'puts StateChild.new.stateful(-4)')
        assert status.success?, err
        assert_equal '129', out.strip
        report = JSON.parse(File.read(File.join(output, 'materialization.json')))
        assert_empty report.fetch('omitted_initializers')
      else
        out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'),
          '-e', 'puts StateChild.new.pure(-4)')
        assert status.success?, err
        assert_equal '-23', out.strip
        report = JSON.parse(File.read(File.join(output, 'materialization.json')))
        assert_equal [{ 'receiver' => 'StateChild', 'initializer_owner' => 'StateParent',
          'instance_variables' => [] }], report.fetch('omitted_initializers')
      end
    end
  end

  def test_namespaced_accessors_module_roots_and_original_parameter_forms
    manifest = File.join(@dir, 'breadth.rb')
    File.write(manifest, <<~RUBY)
      # frozen_string_literal: true
      module Products
        module Labels
          def label(value, prefix: 'λ')
            "\#{prefix}:\#{value}"
          end
        end
        class Parent
          attr_accessor :value
          def initialize(value: nil); @value = value; end
          private
          def weighted(values, offset)
            result = offset
            values.each { |entry| result += entry * 7 }
            result
          end
        end
        class Child < Parent
          def total(offset = 13, *values, factor: 3, &block)
            result = weighted(values, offset) * factor
            block.call(result)
          end
        end
      end
      bias = 19
      BootToCore.capture do
        Products::Child.define_method(:captured) do |values, offset: bias|
          result = offset
          values.each { |entry| result += entry * bias }
          result
        end
      end
      bias = 11
      BootToCore.input(roots: { Products::Child => [:value, :value=, :total, :captured],
        Products::Labels => [:label] })
    RUBY
    output = File.join(@dir, 'breadth')
    _, err, status = invoke('--trust-boot', manifest, '-o', output)
    assert status.success?, err
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'), '-rjson', '-e', <<~RUBY)
      left = Products::Child.new(value: 'east')
      right = Products::Child.new
      returned = left.public_send(:value=, 'west')
      checks = [returned, left.value, right.value,
        left.total(-5, 2, -4, factor: 7) { |v| v - 3 },
        left.total { |v| v + 5 }, left.captured([2, -4]),
        left.captured([-3, 7], offset: -13),
        Object.new.extend(Products::Labels).label('east'),
        Object.new.extend(Products::Labels).label('west', prefix: 'δ'),
        left.respond_to?(:weighted)]
      puts JSON.generate(checks)
    RUBY
    assert status.success?, err
    assert_equal ['west', 'west', nil, -136, 44, -11, 31, 'λ:east', 'δ:west', false], JSON.parse(out)
  end

  def test_generated_symbol_keys_and_deep_frozen_constant_collections
    manifest = File.join(@dir, 'policy.rb')
    File.write(manifest, <<~RUBY)
      # frozen_string_literal: true
      module Policies
        class Mask
          FLAGS = { viewer: 2, owner: 16 }.freeze
          SHARED = [7, -11].freeze
          TREE = { left: SHARED, right: SHARED }.freeze
          SHALLOW = { bad: [] }.freeze
          DEFAULT = Hash.new(19).freeze
          CYCLIC = []; CYCLIC << CYCLIC; CYCLIC.freeze
          def initialize(mask); @mask = mask; end
          def rules; FLAGS; end
          def tree; TREE; end
          def shallow; SHALLOW; end
          def default; DEFAULT; end
          def cyclic; CYCLIC; end
          FLAGS.each_key do |key|
            BootToCore.capture { define_method(key) { @mask.anybits?(FLAGS[key]) } }
          end
        end
      end
      BootToCore.input(roots: { Policies::Mask => ENV.fetch('CUT', 'viewer,owner,rules,tree').split(',').map(&:to_sym) })
    RUBY
    output = File.join(@dir, 'policy')
    _, err, status = invoke('--trust-boot', manifest, '-o', output)
    assert status.success?, err
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'), '-rjson', '-e', <<~RUBY)
      observations = [0, 1, 2, 16, 18].map do |mask|
        object = Policies::Mask.new(mask)
        [object.viewer, object.owner]
      end
      object = Policies::Mask.new(0)
      observations += [object.rules.frozen?, object.rules.equal?(object.rules),
        object.tree.frozen?, object.tree[:left].frozen?, object.tree[:left].equal?(object.tree[:right])]
      begin
        object.rules[:viewer] = 7
      rescue => error
        observations << error.class.name
      end
      puts JSON.generate(observations)
    RUBY
    assert status.success?, err
    assert_equal [[false, false], [false, false], [true, false], [false, true], [true, true],
      true, true, true, true, true, 'FrozenError'], JSON.parse(out)
    %w[shallow default cyclic].each do |cut|
      refused = File.join(@dir, cut)
      _, err, status = Open3.capture3({ 'CUT' => cut }, RbConfig.ruby,
        File.join(ROOT, 'bin/rh'), 'materialize', '--trust-boot', manifest, '-o', refused)
      refute status.success?, cut
      assert_includes err, 'BootToCore::Unsupported'
      refute File.exist?(refused)
    end
  end

  def test_scalar_constants_keep_original_lexical_scope_and_reject_autoload_or_mutable_values
    manifest = File.join(@dir, 'constants.rb')
    File.write(manifest, <<~RUBY)
      # frozen_string_literal: true
      module LexicalSource
        FACTOR = 7
        LABEL = 'λeast'
        EMPTY = nil
        DISABLED = false
        MUTABLE = []
        autoload :LAZY, #{File.join(@dir, 'must-not-load.rb').inspect}
        module Body
          def compute(value); FACTOR * value - 11; end
          def label; LABEL; end
          def flag(value); value ? EMPTY : DISABLED; end
          def mutable; MUTABLE; end
          def lazy; LAZY; end
          def kind; defined?(FACTOR); end
        end
      end
      class ConstantProduct
        FACTOR = 101
        LABEL = 'wrong destination'
      end
      BootToCore.capture do
        [:compute, :label, :flag, :mutable, :lazy, :kind].each do |name|
          ConstantProduct.define_method(name, LexicalSource::Body.instance_method(name))
        end
      end
      BootToCore.input(roots: { ConstantProduct => ENV.fetch('CUT', 'compute,label,flag').split(',').map(&:to_sym) })
    RUBY
    output = File.join(@dir, 'constants')
    _, err, status = invoke('--trust-boot', manifest, '-o', output)
    assert status.success?, err
    out, err, status = Open3.capture3(RbConfig.ruby, '-r', File.join(output, 'lib/core.rb'), '-rjson',
      '-e', 'x = ConstantProduct.new; puts JSON.generate([x.compute(4), x.compute(-4), x.label, x.flag(true), x.flag(false)])')
    assert status.success?, err
    assert_equal [17, -39, 'λeast', nil, false], JSON.parse(out)
    %w[mutable lazy kind].each do |cut|
      refused = File.join(@dir, cut)
      _, err, status = Open3.capture3({ 'CUT' => cut }, RbConfig.ruby,
        File.join(ROOT, 'bin/rh'), 'materialize', '--trust-boot', manifest, '-o', refused)
      refute status.success?, cut
      expected = { 'mutable' => 'constant MUTABLE', 'lazy' => 'constant LAZY', 'kind' => 'Prism::DefinedNode' }.fetch(cut)
      assert_includes err, expected
      refute File.exist?(refused)
    end
  end

  def test_unsupported_boundaries_fail_without_output
    {
      mutable: <<~RUBY,
        class MutableCapture; end
        state = {}
        BootToCore.capture { MutableCapture.define_method(:value) { state } }
        BootToCore.input(roots: { MutableCapture => [:value] })
      RUBY
      native: <<~RUBY,
        class NativeBody; end
        BootToCore.capture { NativeBody.define_method(:value, Kernel.instance_method(:puts)) }
        BootToCore.input(roots: { NativeBody => [:value] })
      RUBY
      uncaptured: <<~RUBY,
        class WarmBody; end
        WarmBody.module_eval('def value; 41; end')
        BootToCore.input(roots: { WarmBody => [:value] })
      RUBY
      override: <<~RUBY,
        class ChangedBody; end
        BootToCore.capture { ChangedBody.module_eval('def value; 3; end') }
        ChangedBody.module_eval('def value; 41; end')
        BootToCore.input(roots: { ChangedBody => [:value] })
      RUBY
      collector_failure: <<~RUBY,
        class HealthyBody
          def value; 41; end
        end
        NativeDefineMethodObserver.collector = ->(*) { raise 'collector bug' }
        Class.new.define_method(:irrelevant) { 19 }
        BootToCore.input(roots: { HealthyBody => [:value] })
      RUBY
      aliased_super: <<~RUBY
        module AncestorBody
          def value; 7; end
        end
        module AliasBody
          include AncestorBody
          def value; super + 11; end
          alias_method :other, :value
        end
        class AliasProduct
          include AliasBody
        end
        BootToCore.input(roots: { AliasProduct => [:other] })
      RUBY
    }.each do |name, source|
      manifest = File.join(@dir, "#{name}.rb")
      output = File.join(@dir, name.to_s)
      File.write(manifest, source)
      _, err, status = invoke('--trust-boot', manifest, '-o', output)
      refute status.success?, name.to_s
      assert_includes err, 'BootToCore::Unsupported'
      refute File.exist?(output)
    end
  end

  def test_real_rails_generator_and_callback_bodies_share_one_input
    output = File.join(@dir, 'app')
    _, err, status = invoke('--trust-boot', File.join(ROOT, 'tools/boot-to-core/rails_input.rb'), '-o', output)
    if err.include?('Gem::MissingSpec')
      skip 'requires ActiveModel/ActiveSupport 8.1.4; no gem is installed by this test'
    end
    assert status.success?, err
    contract = File.join(ROOT, 'tools/boot-to-core/rails_contract.rb')
    reference, err, status = Open3.capture3(RbConfig.ruby, contract, File.join(ROOT, 'tools/boot-to-core/rails_fixture.rb'))
    assert status.success?, err
    actual, err, status = Open3.capture3(RbConfig.ruby, contract, File.join(output, 'lib/core.rb'), '--core')
    assert status.success?, err
    assert_equal JSON.parse(reference), JSON.parse(actual)
    assert_equal 34, JSON.parse(actual).fetch('checks')
    assert File.file?(File.join(output, 'sig/input0.rbs'))
  end
end
