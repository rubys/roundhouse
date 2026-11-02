# The oracle for tests/broadcast_job.rs: the arguments turbo-rails 2.0.23
# (Rails 8.1.4) enqueues Turbo::Streams::ActionBroadcastJob with for a
# model's `broadcasts_to ->(_car) { "cars" }, inserts_by: :prepend`, on
# create and on update. Run in a Rails app with that model (Caboose's
# Car, with `name` and `cargo` columns) under the :test adapter, against
# a fresh database so the car's id is 1:
#
#   bin/rails runner tests/broadcast_job/oracle.rb > tests/broadcast_job/rails.json
require "json"

ActiveJob::Base.queue_adapter = :test
car = Car.create!(name: "X", cargo: "coal")
car.update!(name: "Y")
jobs = ActiveJob::Base.queue_adapter.enqueued_jobs.select { |j| j["job_class"] == "Turbo::Streams::ActionBroadcastJob" }
puts JSON.pretty_generate(
  "turbo-rails" => Gem.loaded_specs["turbo-rails"].version.to_s,
  "create" => jobs[0]["arguments"],
  "update" => jobs[1]["arguments"],
  "queue_name" => jobs[0]["queue_name"]
)
