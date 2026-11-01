//! The Ruby-shaped serving adapters share one generated prefix dispatcher.
//! The Rack adapter returns the application's triple itself; only byte-stream
//! servers consume its body and serialize its headers.

use super::BuildTarget;
use crate::{App, diagnostic::Diagnostic};
use std::fmt::Write;

pub(super) fn check_target(app: &App, target: BuildTarget) -> Result<(), String> {
    if matches!(
        target,
        BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Blog
    ) {
        return Ok(());
    }
    let mounts = crate::lower::rack_mounts::mounts(app);
    if mounts.is_empty() {
        return Ok(());
    }
    for mount in mounts {
        crate::emit::diagnostics::push(Diagnostic::unsupported(
            mount.span,
            Some(crate::Symbol::from(target.as_str())),
            "Rack mount",
            format!(
                "`{}` requires a Rack request/response adapter; in-app Rack mounts are supported on ruby, jruby and spinel",
                mount.target.0
            ),
        ));
    }
    Err(format!(
        "{} does not support in-app Rack mounts",
        target.as_str()
    ))
}

pub(super) fn apply(
    files: &mut Vec<(String, String)>,
    app: &App,
    rack: bool,
) -> Result<(), String> {
    let mounts = crate::lower::rack_mounts::mounts(app);
    if mounts.is_empty() {
        return Ok(());
    }
    let mut source = String::from("# frozen_string_literal: true\nrequire \"stringio\"\n");
    let mut targets = crate::det::HashSet::default();
    for mount in &mounts {
        if targets.insert(mount.target.clone()) {
            writeln!(
                source,
                "require_relative \"../app/{}\"",
                crate::emit::ruby::library_file_anchor(mount.target.0.as_str(), app)
            )
            .unwrap();
        }
    }
    source.push_str(r#"
module RouteTable
  # Rails normalizes the host path before it reaches the mounted app.
  def self.rack_path(path)
    joined = +""
    path.split("/").each { |part| joined = joined + "/" + part unless part.empty? }
    joined = "/" if joined.empty?
    out = +""
    i = 0
    hex = "0123456789abcdefABCDEF"
    while i < joined.length
      piece = joined[i, 3].to_s
      if piece.length == 3 && piece.start_with?("%") && hex.include?(piece[1, 1].to_s) && hex.include?(piece[2, 1].to_s)
        out = out + piece.upcase
        i += 3
      else
        out = out + joined[i, 1].to_s
        i += 1
      end
    end
    out
  end

  # HEAD first considers explicit HEAD routes anywhere in the table.
  def self.rack_index(method, path, cursor = 0)
    path = rack_path(path)
    if method == "HEAD"
      return -1 unless ActionDispatch::Router.match("HEAD", path, rack_head_routes).nil?
      method = "GET"
    end
"#);
    for (index, _) in mounts.iter().enumerate() {
        writeln!(source, "    if cursor <= {index}").unwrap();
        writeln!(source, "      return -1 unless ActionDispatch::Router.match(method, path, rack_routes_before_{index}).nil?").unwrap();
        writeln!(source, "      return {index} unless rack_match({index}, path).nil?").unwrap();
        source.push_str("    end\n");
    }
    source.push_str("    -1\n  end\n\n  def self.rack_match(index, path)\n    case index\n");
    for (index, mount) in mounts.iter().enumerate() {
        // Rails' unanchored literal matcher deliberately accepts punctuation
        // boundaries as well as a slash, but not /filesystem or /files0.
        writeln!(source, "    when {index}\n      Regexp.new({}).match(path)",
            ruby_string(&format!(r"\A{}(?:\b|\Z|/)", regex::escape(&mount.prefix)))).unwrap();
    }
    source.push_str("    else\n      nil\n    end\n  end\n\n  # Keep the env and final response objects: Rack's caller owns the body.\n  def self.call_rack(index, env)\n    path = rack_path(env.fetch(\"PATH_INFO\", \"\").to_s)\n    matched = rack_match(index, path)\n    return [404, {\"x-cascade\" => \"pass\"}, [\"Not Found\"]] if matched.nil?\n    env[\"SCRIPT_NAME\"] = (env.fetch(\"SCRIPT_NAME\", \"\").to_s + matched.to_s).chomp(\"/\")\n    rest = matched.post_match\n    rest = \"/\" + rest unless rest.start_with?(\"/\")\n    env[\"PATH_INFO\"] = rest\n    case index\n");
    for (index, mount) in mounts.iter().enumerate() {
        writeln!(source, "    when {index}").unwrap();
        writeln!(source, "      {}.call(env)", mount.target.0).unwrap();
    }
    source.push_str(r#"    else
      [404, {"x-cascade" => "pass"}, ["Not Found"]]
    end
  end

  # A declined mount resumes the ordered table with the original environment.
  # Earlier controller routes were already tested with this same host path.
  def self.dispatch_rack(env)
    method = env.fetch("REQUEST_METHOD", "GET").to_s
    script_name = env.fetch("SCRIPT_NAME", "")
    path_info = env.fetch("PATH_INFO", "")
    index = rack_index(method, path_info.to_s)
    while index >= 0
      response = call_rack(index, env)
      return response unless response[1]["x-cascade"] == "pass"
      body = response[2]
      begin
        body.close if body.respond_to?(:close)
      ensure
        env["SCRIPT_NAME"] = script_name
        env["PATH_INFO"] = path_info
      end
      index = rack_index(method, path_info.to_s, index + 1)
    end
    nil
  end
end
"#);
    put(files, "config/rack_mounts.rb", source);
    put(files, "sig/config/rack_mounts.rbs", "module RouteTable\n  def self.rack_path: (String path) -> String\n  def self.rack_index: (String method, String path, ?Integer cursor) -> Integer\n  def self.rack_match: (Integer index, String path) -> MatchData?\n  def self.call_rack: (Integer index, Hash[String, untyped] env) -> [Integer, Hash[String, String | Array[String]], untyped]\n  def self.dispatch_rack: (Hash[String, untyped] env) -> [Integer, Hash[String, String | Array[String]], untyped]?\nend\n".into());
    let (_, routes) = files
        .iter_mut()
        .find(|(path, _)| path == "config/routes.rb")
        .ok_or("Rack mount: missing route table")?;
    if !routes.contains("require_relative \"rack_mounts\"") {
        routes.push_str("\nrequire_relative \"rack_mounts\"\n");
    }
    let (_, main) = files
        .iter_mut()
        .find(|(path, _)| path == "main.rb")
        .ok_or("Rack mount: missing serving adapter")?;
    if rack {
        replace(
            main,
            "  def self.run_rack(env)\n",
            "  def self.run_rack(env)\n    rack_response = RouteTable.dispatch_rack(env)\n    return rack_response unless rack_response.nil?\n",
        )?;
        replace(
            main,
            "  def self.run(env, stdin, stdout)\n",
            "  def self.run(env, stdin, stdout)\n    env[\"rack.input\"] ||= stdin\n    rack_response = RouteTable.dispatch_rack(env)\n    unless rack_response.nil?\n      status, headers, body = rack_response\n      begin\n        stdout.write(\"Status: #{status}\\r\\n\")\n        headers.each do |name, value|\n          Array(value).each { |entry| stdout.write(\"#{name}: #{entry}\\r\\n\") }\n        end\n        stdout.write(\"\\r\\n\")\n        body.each { |chunk| stdout.write(chunk) }\n      ensure\n        body.close if body.respond_to?(:close)\n      end\n      return nil\n    end\n",
        )?;
    } else {
        replace(
            main,
            "    matched = ActionDispatch::Router.match(verb, request_path, Main.route_table)\n",
            NATIVE_DISPATCH,
        )?;
    }
    Ok(())
}

const NATIVE_DISPATCH: &str = r#"    rack_index = RouteTable.rack_index(verb, request_path)
    if rack_index >= 0
      env = {"REQUEST_METHOD" => verb, "SCRIPT_NAME" => "", "PATH_INFO" => req.path,
             "QUERY_STRING" => req.raw_query, "rack.url_scheme" => "http",
             "rack.input" => StringIO.new(req.raw_body), "rack.errors" => $stderr}
      req.req_headers.each do |name, value|
        key = name.upcase.tr("-", "_")
        key = "HTTP_" + key unless key == "CONTENT_TYPE" || key == "CONTENT_LENGTH"
        env[key] = value
      end
      rack_response = RouteTable.dispatch_rack(env)
      unless rack_response.nil?
        status, headers, body = rack_response
        begin
          res.status = status
          headers.each do |name, value|
            Array(value).each { |entry| res.append_header(Tep.header_name(name), entry) }
          end
          text = +""
          body.each { |chunk| text = text + chunk }
          res.body = text
        ensure
          body.close if body.respond_to?(:close)
        end
        return
      end
    end
    matched = ActionDispatch::Router.match(verb, request_path, Main.route_table)
"#;

fn ruby_string(value: &str) -> String {
    // A Ruby single-quoted literal: interpolation and shell syntax stay data.
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn put(files: &mut Vec<(String, String)>, path: &str, content: String) {
    if let Some((_, current)) = files.iter_mut().find(|(name, _)| name == path) {
        *current = content;
    } else {
        files.push((path.into(), content));
    }
}

fn replace(source: &mut String, anchor: &str, replacement: &str) -> Result<(), String> {
    if source.matches(anchor).count() != 1 {
        return Err("Rack mount: serving adapter no longer has its unique dispatch anchor".into());
    }
    *source = source.replacen(anchor, replacement, 1);
    Ok(())
}
