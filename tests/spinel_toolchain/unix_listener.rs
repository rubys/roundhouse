use super::{emit_and_run, native_http};
use std::process::Command;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn unix_and_tcp_connections_keep_their_transport() {
    let (tree, errors) = emit_and_run::real_blog()
        .write("app/controllers/listener_controller.rb", "class ListenerController < ApplicationController\n  skip_forgery_protection\n  def show\n    headers[\"X-Worker\"] = Process.pid.to_s\n    render plain: request.env[\"roundhouse.listener\"].to_s\n  end\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  root "articles#index"
  resources :articles do
    resources :comments, only: [:create, :destroy]
  end
  get '/listener', to: 'listener#show'
  post '/listener', to: 'listener#show'
end
"#)
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let main = tree.join("main.rb");
    let source = std::fs::read_to_string(&main).unwrap();
    let at = source.find("# Unconditional entry").unwrap();
    let direct = r#"
if ENV["TEP_DIRECT"] == "1"
  port = ENV["PORT"].to_i
  workers = ENV["WORKERS"].to_i
  Tep::APP.bind_host = ENV["BINDING"].to_s
  unix_listener = Tep::UnixListener.new(ARGV[1])
  unix_listener.open
  Main.configure_default_adapter!
  if ENV["TEP_SERVER"] == "blocking"
    Tep::Server.new(MainApp.new).run(port, workers, false, unix_listener)
  elsif ENV["TEP_SERVER"] == "fiber"
    Tep::Server::Scheduled.new(MainApp.new).run(port, workers, false, unix_listener)
  else
    Tep::Server::Threaded.new(MainApp.new).run(port, workers, false, unix_listener)
  end
else
"#;
    std::fs::write(
        &main,
        format!("{}{}{}\nend\n", &source[..at], direct, &source[at..]),
    )
    .unwrap();
    native_http::build(&tree);
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/tep_unix_http.py"
        ))
        .arg(tree.join("build/bin/blog"))
        .arg(&*tree)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
