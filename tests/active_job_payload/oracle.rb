#!/usr/bin/env ruby
# The oracle for tests/active_job_payload.rs: real ActiveJob's answer for
# the argument values driver.rb writes, and for `job.serialize`.
#
#   ruby tests/active_job_payload/oracle.rb > tests/active_job_payload/rails.json
#
# Needs activejob 8.1.3 (the version Phase 1 matches; Lobsters pins
# 8.1.3.1, Caboose 8.1.4) and globalid installed. The values below are
# the ones driver.rb writes, in the same order; keep the two in step.
gem "activejob", "8.1.3"
gem "activesupport", "8.1.3"
require "active_job"
require "cgi"
require "global_id"
require "json"
require "active_support/core_ext/time"

GlobalID.app = "campfire"
# A Rails app's default `config.time_zone`, which `job.serialize` writes
# as `timezone`.
Time.zone = "UTC"

class Room
  include GlobalID::Identification
  attr_reader :id
  def initialize(id) = @id = id
end

module Rooms
  class Open < Room; end
end

class PushMessageJob < ActiveJob::Base
  def perform(*args); end
end

t = Time.at(1_791_548_096, 123_456_789, :nsec).utc
values = [
  nil, true, false,
  42, -7, 2**40,
  1.5, 0.1, -2.5e-08, 1.0e+20,
  "plain", "quote\" backslash\\ nl\n tab\t ctrl\u0001 <&> é ✓  ", "",
  :open,
  Room.new(7), Rooms::Open.new(8),
  t, Time.at(1_791_548_096, 5, :nsec).utc,
  [1, 2, 3], ["a", "b"], [], [true, false], [:x, :y]
]

job = PushMessageJob.new(*values)
puts JSON.pretty_generate(
  "activejob" => ActiveJob.version.to_s,
  "arguments" => ActiveJob::Arguments.serialize(values),
  "job" => job.serialize
)
