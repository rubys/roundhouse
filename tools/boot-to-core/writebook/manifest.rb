# frozen_string_literal: true

# Passed to the exact snapshot's bin/rh materialize; BootToCore is supplied there.
require "bundler/setup"
require_relative "provenance"
lane = ENV.fetch("WRITEBOOK_LANE")
if lane == "full"
  BootToCore.capture do
    require_relative "boot"
    Rails.application.eager_load!
    prepare_writebook
    generate_writebook_attributes(writebook_roots("enum"))
  end
  BootToCore.input(roots: writebook_roots("enum"))
else
  require_relative "boot"
  prepare_writebook
  roots = nil
  if ENV.fetch("WRITEBOOK_CAPTURE", "focused") == "focused"
    # Framework boot is unobserved, but the original app files/autoloads and
    # lazy schema accessors are observed, without repeating/replacing any DSL.
    BootToCore.capture do
      roots = writebook_roots(lane)
      generate_writebook_attributes(roots)
    end
  else
    roots = writebook_roots(lane)
    generate_writebook_attributes(roots)
  end
  BootToCore.input(roots: roots, signatures: lane == "qr" ? ["qr.rbs"] : [])
end
