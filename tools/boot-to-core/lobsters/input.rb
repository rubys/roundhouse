# frozen_string_literal: true

app = File.realpath(ENV.fetch("LOBSTERS_APP"))
lane = ENV.fetch("LOBSTERS_LANE")
booted = false
at_exit do
  warn "LOBSTERS_OBSERVATION=" + JSON.generate(booted: booted,
    opaque_definitions: BootToCore.records.values.count { |record| record.reason },
    observer_failures: NativeDefineMethodObserver.failures.map { |owner, name, error|
      { owner: owner.name, method: name, error: error.class.name, message: error.message } })
end

# Observe the actual first app/gem declarations. Never replay a DSL or replace
# storage. The full-capture lane also observes framework boot, for comparison.
if lane == "full_capture"
  BootToCore.capture { require File.join(app, "config/environment") }
else
  require File.join(app, "config/boot")
  trace = TracePoint.new(:class, :end) do |event|
    if event.path.start_with?(app + "/app/models/")
      BootToCore.active = event.event == :class
    end
  end
  begin
    trace.enable { require File.join(app, "config/environment") }
  ensure
    BootToCore.active = false
  end
end
booted = true

roots = case lane
when "search", "full_capture"
  { Search => %i[q q= order order= page page= per_page per_page= total_results total_results= what what= page_count persisted?] }
when "short_id"
  { ShortId => %i[klass klass= generation_attempts generation_attempts=] }
when "namespace"
  { ShortId::CandidateId => %i[id id= to_s] }
when "pagination"
  { ApplicationHelper => [:page_numbers_for_pagination] }
when "paginator"
  { StoriesPaginator => %i[per_page per_page=] }
when "inherited"
  { TimeSeries => [:get_x_labels] }
when "delegate"
  { ModNote => [:username] }
when "dynamic"
  { Search => [:to_url_params] }
else
  raise "unknown lane #{lane}"
end
warn "LOBSTERS_ROOTS=" + JSON.generate(roots.flat_map { |owner, names|
  names.map { |name|
    method = owner.instance_method(name)
    { receiver: owner.name, method: name, definition_owner: method.owner.name,
      source: method.source_location, parameters: method.parameters }
  }
})
signature = { "full_capture" => "search", "dynamic" => "search" }.fetch(lane, lane)
path = File.join(__dir__, "#{signature}.rbs")
BootToCore.input(roots: roots, signatures: File.file?(path) ? [path] : [])
