# frozen_string_literal: true
require "mkmf"
abort "this experiment requires MRI" unless RUBY_ENGINE == "ruby"
create_makefile("native_define_method_observer")
