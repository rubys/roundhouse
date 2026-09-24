# CachePageJob façade — lobsters warms its full-page cache by replaying a
# request through `ActionDispatch::Integration::Session` inside a job and
# writing the body where `caches_page` would. Neither half is modeled:
# `caches_page` is dropped by the controller lowering (page caching is a
# deployment concern the emitted server does not have), and an
# integration session is a test harness no AOT runtime carries. So the
# scaffold base ships this stand-in at the same emit path, leaving the
# require graph untouched; the CRuby tree — where the source runs as
# written — restores the verbatim emit (see
# emit::ruby::library::restore_extras_facades). Same raise-loudly
# contract as the Sponge façade: the class-side job API is REAL (the
# same shape the job lowering synthesizes, so PrefillPageCacheJob's
# enqueue still compiles and runs), and `perform` raises — which the
# in-process queue reports and moves past.
require_relative "../../runtime/gem_facades"
require_relative "application_job"

class CachePageJob < ApplicationJob
  def perform(path)
    GemFacade.fail!("CachePageJob#perform")
    nil
  end

  def self.perform_later(path)
    ActiveJob.record_performed("CachePageJob")
    if !(ActiveJob.enqueue_only)
      if ActiveJob.drain_registered
        ActiveJob.enqueue(-> { new.perform(path)
        nil })
      else
        new.perform(path)
      end
    end
    nil
  end

  def self.perform_now(path)
    new.perform(path)
  end

  def self.set(options)
    self
  end
end
