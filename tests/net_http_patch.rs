//! Net::HTTP::Patch and Net HTTP exception classes resolve (stdlib surface).

use roundhouse::analyze::{diagnose, Analyzer, Severity};
use roundhouse::ingest::ingest_app;

#[test]
fn net_http_patch_and_error_classes_check_clean() {
    let root = std::env::temp_dir().join(format!("roundhouse-net-http-{}", std::process::id()));
    for (path, source) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/\", to: \"homes#show\"\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/homes_controller.rb",
            "class HomesController < ApplicationController\n  def show\n    klass = Net::HTTP::Patch\n    begin\n      raise Net::ProtocolError, \"bad\"\n    rescue Net::ProtocolError => e\n      render plain: \"#{klass.name}:#{e.class.name}\"\n    end\n  end\nend\n",
        ),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let mut app = ingest_app(&root).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert!(
        errors.is_empty(),
        "Net::HTTP::Patch and Net::ProtocolError must resolve:\n{errors:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn cruby_only_http_header_syntax_error_stays_refused() {
    // `Net::HTTPBadResponse` is registered on main (Lobsters / Sponge) and
    // Spinel-ledgers via `unavailable_class_module_construct`. Header
    // syntax stays an honest constant gap until Spinel grows it
    // (invariant 6) — Jumpstart Pro still names it in NET_HTTP_ERRORS.
    let root = std::env::temp_dir().join(format!(
        "roundhouse-net-http-cruby-only-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (path, source) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/\", to: \"homes#show\"\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/clients/application_client.rb",
            "class ApplicationClient\n  NET_HTTP_ERRORS = [Timeout::Error, Net::HTTPBadResponse, Net::HTTPHeaderSyntaxError, Net::ProtocolError]\nend\n",
        ),
        (
            "app/controllers/homes_controller.rb",
            "class HomesController < ApplicationController\n  def show\n    render plain: ApplicationClient::NET_HTTP_ERRORS.size.to_s\n  end\nend\n",
        ),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let mut app = ingest_app(&root).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert!(
        !errors.iter().any(|e| e.contains("HTTPBadResponse")),
        "Net::HTTPBadResponse is registered (main) + Spinel-ledgers:\n{errors:#?}"
    );
    assert!(
        errors.iter().any(|e| e.contains("HTTPHeaderSyntaxError")),
        "CRuby-only Net::HTTPHeaderSyntaxError must stay refused:\n{errors:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}
