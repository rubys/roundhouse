//! `broadcasts_to` on `queue_adapter :sidekiq` (`lower::broadcast_job`):
//! the create and update broadcasts push Turbo::Streams::ActionBroadcastJob
//! with turbo-rails' own arguments, and the worker's arm runs it.
//!
//! `tests/broadcast_job/rails.json` is what turbo-rails 2.0.23 enqueued
//! for Caboose's Car (`tests/broadcast_job/oracle.rb`); the driver
//! writes the same two broadcasts through the runtime and compares the
//! JSON text, key order included.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::app::App;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::broadcast_job::{self, BroadcastModel};
use roundhouse::lower::job_payload::registry_source_with_broadcasts;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :cars do |t|
    t.string :name
  end
  create_table :rooms do |t|
    t.string :name
    t.string :type
  end
end
"#;

fn app(adapter: &str) -> App {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "config/environments/production.rb",
            &format!("Rails.application.configure do\n  config.active_job.queue_adapter = :{adapter}\nend\n"),
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        ),
        (
            "app/models/car.rb",
            "class Car < ApplicationRecord\n  broadcasts_to ->(_car) { \"cars\" }, inserts_by: :prepend\nend\n",
        ),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  broadcasts_to ->(_room) { \"rooms\" }\nend\n",
        ),
        ("app/models/rooms/open.rb", "class Rooms::Open < Room\nend\n"),
        ("app/views/cars/_car.html.erb", "<div id=\"<%= dom_id(car) %>\"><%= car.name %></div>\n"),
        ("app/views/rooms/_room.html.erb", "<div><%= room.name %></div>\n"),
    ]))
    .expect("ingest broadcast app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn model_file(app: &App, name: &str) -> String {
    ruby::emit_lowered_models(app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(&format!("app/models/{name}.rb")))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("no app/models/{name}.rb"))
}

fn car() -> BroadcastModel {
    BroadcastModel { model: "Car".into(), partial: "cars/car".into(), local: "car".into() }
}

#[test]
fn the_payload_is_turbo_rails() {
    let base = option_env!("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("broadcast_job");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (from, to) in [
        ("runtime/spinel/active_job_serialization.rb", "active_job_serialization.rb"),
        ("tests/broadcast_job/driver.rb", "driver.rb"),
    ] {
        std::fs::copy(root().join(from), dir.join(to)).unwrap();
    }
    let out = Command::new("ruby")
        .arg("driver.rb")
        .arg(root().join("tests/broadcast_job/rails.json"))
        .current_dir(&dir)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && stdout.contains("done"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("match create\n") && stdout.contains("match update\n"), "{stdout}");
    assert!(stdout.contains("read create cars prepend cars cars/car gid://caboose/Car/1\n"), "{stdout}");
    assert!(stdout.contains("read update cars replace car_1 cars/car gid://caboose/Car/1\n"), "{stdout}");
}

#[test]
fn create_and_update_push_the_job_and_destroy_stays_inline() {
    let app = app("sidekiq");
    assert_eq!(broadcast_job::models(&app), vec![car()], "the STI Room is left out");
    let src = model_file(&app, "car");
    assert!(
        src.contains("    if ActiveJob.drain_registered\n      JobRegistry.push_turbo_broadcast(\"cars\", \"prepend\", \"cars\", \"cars/car\", \"car\", self.to_gid_uri)\n    else\n      Broadcasts.prepend(stream: \"cars\""),
        "{src}"
    );
    assert!(
        src.contains("JobRegistry.push_turbo_broadcast(\"cars\", \"replace\", \"#{self.dom_prefix}_#{self.dom_record_key}\", \"cars/car\", \"car\", self.to_gid_uri)"),
        "{src}"
    );
    assert!(!src.contains("push_turbo_broadcast(\"cars\", \"remove\""), "{src}");
    assert!(src.contains("  def turbo_broadcast_html\n    ActionView::ViewHelpers.broadcast_render("), "{src}");
    // The STI model keeps its inline broadcasts.
    assert!(!model_file(&app, "room").contains("push_turbo_broadcast"));
}

#[test]
fn off_sidekiq_nothing_changes() {
    let app = app("solid_queue");
    assert!(broadcast_job::models(&app).is_empty());
    let src = model_file(&app, "car");
    assert!(!src.contains("push_turbo_broadcast") && !src.contains("turbo_broadcast_html"), "{src}");
    assert!(!registry_source_with_broadcasts(&[], false, &[car()]).contains("Turbo"));
}

#[test]
fn the_registry_pushes_and_runs_the_job() {
    let src = registry_source_with_broadcasts(&[], true, &[car()]);
    assert!(src.contains("  def self.push_turbo_broadcast(stream, action, target, partial, local, gid_uri)\n    SidekiqPush.enqueue(\"Turbo::Streams::ActionBroadcastJob\", \"default\", 0.0,"), "{src}");
    assert!(src.contains("    when \"Turbo::Streams::ActionBroadcastJob\"\n      perform_turbo_broadcast(args)\n      true\n"), "{src}");
    assert!(src.contains("    when \"cars/car\"\n      html = locate_broadcast_car(ActiveJob::Arguments.option_local_at(args, 1, \"car\")).turbo_broadcast_html\n"), "{src}");
    assert!(src.contains("      Broadcasts.prepend(stream: stream, target: target, html: html)\n"), "{src}");
    assert!(src.contains("job_class == \"Turbo::Streams::ActionBroadcastJob\""), "the job discards a gone record:\n{src}");
    assert!(src.contains("record = Car.find_by(id: GlobalID::Locator.cast_id(parts[2]))"), "{src}");
}
