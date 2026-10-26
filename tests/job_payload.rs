//! ActiveJob payloads (`lower::job_payload`): the codec plan each job
//! gets from its call sites, the enqueue arm the Ruby emitter rewrites,
//! and the `JobRegistry` text `project.rs` writes.
//!
//! The plan comes from `perform_later` CALL SITES, not `perform`'s own
//! signature, which is untyped for every corpus job (nothing calls
//! `perform` directly before the wrappers exist). A job whose arguments
//! all have a codec gets a payload; any other keeps its Proc and a
//! `job-closure-fallback` ledger line. The hold arm (the `:test`
//! adapter) and the inline arm are not touched.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::App;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::job_payload::{Codec, JobPlan, registry_source};

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :rooms do |t|
    t.string :name
    t.string :type
  end
  create_table :messages do |t|
    t.integer :room_id
    t.string :body
  end
  create_table :users do |t|
    t.string :name
  end
end
"#;

fn app() -> App {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  has_many :messages

  def receive(message)
    push_later(message)
  end

  def push_later(message)
    PushJob.perform_later(self, message)
  end
end
"#,
        ),
        ("app/models/rooms/open.rb", "class Rooms::Open < Room\nend\n"),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  belongs_to :room
  after_create_commit -> { room.receive(self) }

  def notify
    NotifyJob.perform_later(self)
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  def self.later
    NullableJob.perform_later(User.find_by(id: 1))
    PrimitivesJob.perform_later(1, "x", 1.5, true, :a, [1, 2])
    QueueJob.perform_later
  end
end
"#,
        ),
        (
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\nend\n",
        ),
        (
            "app/jobs/push_job.rb",
            "class PushJob < ApplicationJob\n  def perform(room, message)\n    nil\n  end\nend\n",
        ),
        (
            "app/jobs/notify_job.rb",
            "class NotifyJob < ApplicationJob\n  def perform(*messages)\n    nil\n  end\nend\n",
        ),
        (
            "app/jobs/nullable_job.rb",
            "class NullableJob < ApplicationJob\n  def perform(user)\n    nil\n  end\nend\n",
        ),
        (
            "app/jobs/primitives_job.rb",
            "class PrimitivesJob < ApplicationJob\n  def perform(n, s, f, b, sym, ids)\n    nil\n  end\nend\n",
        ),
        (
            "app/jobs/queue_job.rb",
            "class QueueJob < ApplicationJob\n  queue_as :mailers\n  discard_on ActiveJob::DeserializationError\n\n  def perform\n    nil\n  end\nend\n",
        ),
        (
            "app/jobs/untyped_job.rb",
            "class UntypedJob < ApplicationJob\n  def perform(*args)\n    nil\n  end\nend\n",
        ),
    ]))
    .expect("ingest job app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn plan<'a>(app: &'a App, job: &str) -> &'a JobPlan {
    app.job_plans
        .iter()
        .find(|p| p.job == job)
        .unwrap_or_else(|| panic!("no plan for {job}: {:#?}", app.job_plans))
}

fn emitted(app: &App, suffix: &str) -> String {
    let files = ruby::emit_library(app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("no file ending in {suffix}"))
}

fn room_record() -> Codec {
    Codec::Record { model: "Room".to_string(), accepts: vec!["Room".to_string(), "Rooms::Open".to_string()] }
}

#[test]
fn codecs_come_from_the_call_sites() {
    let app = app();

    // `message` reaches `perform_later` through `Room#receive`, whose
    // only caller is Message's block-form `after_create_commit`. That
    // callback has to count as a call site for the type to arrive.
    let push = plan(&app, "PushJob");
    assert!(push.is_payload(), "{push:#?}");
    assert_eq!(push.params[0].codec, room_record(), "the STI base, accepting its subclass");
    assert_eq!(
        push.params[1].codec,
        Codec::Record { model: "Message".to_string(), accepts: vec!["Message".to_string()] }
    );

    let notify = plan(&app, "NotifyJob");
    assert!(notify.params[0].rest, "{notify:#?}");
    assert!(matches!(&notify.params[0].codec, Codec::Record { model, .. } if model == "Message"));

    let nullable = plan(&app, "NullableJob");
    assert!(
        matches!(&nullable.params[0].codec, Codec::NullableRecord { model, .. } if model == "User"),
        "{nullable:#?}"
    );

    let prims = plan(&app, "PrimitivesJob");
    let codecs: Vec<&Codec> = prims.params.iter().map(|p| &p.codec).collect();
    assert_eq!(
        codecs,
        vec![
            &Codec::Int,
            &Codec::Str,
            &Codec::Float,
            &Codec::Bool,
            &Codec::Sym,
            &Codec::Array(Box::new(Codec::Int)),
        ],
        "{prims:#?}"
    );
}

#[test]
fn queue_as_and_discard_on_reach_the_plan() {
    let app = app();
    let queue = plan(&app, "QueueJob");
    assert!(queue.is_payload() && queue.params.is_empty(), "{queue:#?}");
    assert_eq!(queue.queue, "mailers");
    assert!(queue.discards_deserialization_error);
    let push = plan(&app, "PushJob");
    assert_eq!(push.queue, "default");
    assert!(!push.discards_deserialization_error);
}

#[test]
fn a_job_without_typed_arguments_keeps_its_proc() {
    let app = app();
    let untyped = plan(&app, "UntypedJob");
    let fallback = untyped.fallback.as_ref().expect("UntypedJob has no call site");
    assert!(fallback.reason.contains("no call site"), "{fallback:?}");

    let src = emitted(&app, "untyped_job.rb");
    assert!(src.contains("ActiveJob.enqueue(-> {"), "the Proc stays:\n{src}");
    assert!(!src.contains("enqueue_payload"), "{src}");
}

#[test]
fn only_the_enqueue_arm_changes() {
    let app = app();
    let src = emitted(&app, "push_job.rb");
    assert!(
        src.contains("ActiveJob.enqueue_payload(JobRegistry.payload_push_job(room, message))"),
        "the enqueue arm writes a payload:\n{src}"
    );
    assert!(!src.contains("ActiveJob.enqueue(-> {"), "no Proc on the drain path:\n{src}");
    // The `:test` adapter's arm and the inline arm are as before.
    assert!(src.contains(r#"ActiveJob.hold("PushJob", -> {"#), "{src}");
    assert!(src.contains("new.perform(room, message)"), "{src}");
}

#[test]
fn the_registry_writes_and_reads_each_payload_job() {
    let app = app();
    let src = registry_source(&app.job_plans, false);

    assert!(src.contains("def self.payload_push_job(room, message)"), "{src}");
    // The wrapper hands its rest parameter over as one Array.
    assert!(src.contains("def self.payload_notify_job(messages)"), "{src}");
    assert!(
        src.contains(r#"ActiveJob::Payload.build("PushJob", "default", ActiveJob::Arguments.list([ActiveJob::Arguments.record(room.to_gid_uri), ActiveJob::Arguments.record(message.to_gid_uri)]))"#),
        "{src}"
    );
    assert!(src.contains(r#"ActiveJob::Payload.build("QueueJob", "mailers", ActiveJob::Arguments.list([]))"#), "{src}");
    assert!(
        src.contains("messages.map { |a| ActiveJob::Arguments.record(a.to_gid_uri) }"),
        "a rest parameter flattens into the argument list:\n{src}"
    );
    assert!(src.contains("(user.nil? ? ActiveJob::Arguments.null : ActiveJob::Arguments.record(user.to_gid_uri))"), "{src}");
    assert!(src.contains("ActiveJob::Arguments.list(ids.map { |e| ActiveJob::Arguments.int(e) })"), "{src}");

    assert!(src.contains("      PushJob.new.perform(locate_room_at(args, 0), locate_message_at(args, 1))\n      true\n"), "{src}");
    assert!(src.contains("NotifyJob.new.perform(*locate_message_from(args, 0))"), "{src}");
    assert!(src.contains("NullableJob.new.perform((ActiveJob::Arguments.null_at(args, 0) ? nil : locate_user_at(args, 0)))"), "{src}");
    assert!(
        src.contains("PrimitivesJob.new.perform(ActiveJob::Arguments.int_at(args, 0), ActiveJob::Arguments.str_at(args, 1), ActiveJob::Arguments.float_at(args, 2), ActiveJob::Arguments.bool_at(args, 3), ActiveJob::Arguments.sym_at(args, 4), ActiveJob::Arguments.int_array_at(args, 5))"),
        "{src}"
    );
    assert!(src.contains("      QueueJob.new.perform\n"), "{src}");
    assert!(!src.contains(r#"when "UntypedJob""#), "a Proc job has no arm:\n{src}");

    assert!(src.contains(r#"job_class == "QueueJob""#), "the discard table:\n{src}");
    assert!(
        src.contains(r#"unless name == "Room" || name == "Rooms::Open""#),
        "the locator accepts the STI subclass by literal name:\n{src}"
    );
    assert!(src.contains("record = Room.find_by(id: GlobalID::Locator.cast_id(parts[2]))"), "{src}");
    assert!(!src.contains("constantize"), "{src}");
}

#[test]
fn an_app_without_jobs_keeps_an_empty_registry() {
    let src = registry_source(&[], false);
    assert_eq!(
        src,
        "  def self.perform(job_class, args)\n    false\n  end\n\n  def self.discards_deserialization_error(job_class)\n    false\n  end\n"
    );
}

/// A job that enqueues itself without naming its class, and a job that
/// takes a class with no table, keep their Procs.
#[test]
fn unattributable_calls_and_tableless_models_keep_the_proc() {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/models/search.rb",
            "class Search\n  include ActiveModel::Model\n\n  def self.run\n    SearchJob.perform_later(Search.new)\n    RetryJob.perform_later(User.first)\n  end\nend\n",
        ),
        ("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\nend\n"),
        (
            "app/jobs/search_job.rb",
            "class SearchJob < ApplicationJob\n  def perform(search)\n    nil\n  end\nend\n",
        ),
        (
            "app/jobs/retry_job.rb",
            "class RetryJob < ApplicationJob\n  def perform(user)\n    self.class.perform_later(user)\n  end\nend\n",
        ),
    ]))
    .expect("ingest job app");
    roundhouse::session::analyze_and_lower(&mut app);
    let search = plan(&app, "SearchJob");
    assert!(!search.is_payload(), "a model without a table has no codec: {search:#?}");
    let retry = plan(&app, "RetryJob");
    let fallback = retry.fallback.as_ref().expect("self.class.perform_later is unattributable");
    assert!(fallback.reason.contains("doesn't name the job class"), "{fallback:?}");
}

/// The list reader a rest of records gets calls only readers the
/// Spinel runtime defines. The runtime renamed `Arguments.count` to
/// `arg_count` (a second user `count(x)` breaks Spinel's poly
/// `adapter.count`), and a stale `count(args)` here makes every
/// `--target spinel` build of such an app refuse with "unsupported
/// call: CallNode `count`".
#[test]
fn the_rest_reader_calls_only_runtime_readers() {
    let app = app();
    let src = registry_source(&app.job_plans, false);

    assert!(
        src.contains("(from...ActiveJob::Arguments.arg_count(args)).map { |j| locate_message_at(args, j) }"),
        "{src}"
    );
    assert!(!src.contains("Arguments.count("), "{src}");

    let runtime = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/active_job_serialization.rb"),
    )
    .expect("read the ActiveJob runtime");
    let arguments: Vec<&str> = runtime
        .lines()
        .skip_while(|l| l.trim_end() != "  module Arguments")
        .take_while(|l| l.trim_end() != "  end")
        .filter_map(|l| l.trim_start().strip_prefix("def self."))
        .map(|l| l.split(|c: char| c == '(' || c.is_whitespace()).next().unwrap_or(""))
        .collect();
    assert!(arguments.contains(&"gid_parts"), "parsed the Arguments module: {arguments:?}");
    for call in src.split("ActiveJob::Arguments.").skip(1) {
        let name: String = call.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        assert!(
            arguments.contains(&name.as_str()),
            "the registry calls ActiveJob::Arguments.{name}, which the runtime doesn't define:\n{src}"
        );
    }
}
