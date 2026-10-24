//! YAML (Psych) with its rescuable errors, `REXML::Document`, ERB's
//! `result_with_hash` and `Digest::MD5.file`, as a Rails app writes them
//! (a registry read that tolerates a broken file, a Syncthing config.xml
//! rewrite, a rendered shell script, an asset fingerprint). Each used to
//! stop `check` with "constant not supported (all targets)" or "no known
//! method".

use super::emit_and_run;

const PROBE: &str = r#"class StdlibProbe
  def self.registry(path)
    YAML.safe_load_file(path, permitted_classes: [Symbol], aliases: true) || {}
  rescue Errno::ENOENT, Psych::Exception
    {}
  end

  def self.refusal(path)
    Psych.safe_load_file(path)
    "loaded"
  rescue Psych::SyntaxError => e
    "syntax at line #{e.line}"
  rescue Psych::BadAlias
    "alias"
  rescue Psych::Exception => e
    e.message
  end

  def self.harden(xml)
    doc = REXML::Document.new(xml)
    opts = doc.root.elements["options"] || doc.root.add_element("options")
    (opts.elements["relaysEnabled"] || opts.add_element("relaysEnabled")).text = "false"
    [doc.root.elements["gui"].attribute("tls")&.value, doc.to_s]
  end

  def self.script(name, home)
    ERB.new("NAME=<%= name %>\nHOME=<%= home %>\n", trim_mode: "-").result_with_hash(name: name, home: home)
  end

  def self.fingerprint(path)
    Digest::MD5.file(path).hexdigest[0, 10]
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/stdlib_probe.rb", PROBE)
}

const ASSERTIONS: &str = r##"File.write("registry.yml", "projects:\n  web: &w\n    path: /srv/web\n  copy: *w\n")
File.write("broken.yml", "a: [\n")
File.write("alias.yml", "a: &x 1\nb: *x\n")
File.write("symbol.yml", "a: :b\n")
web = { "path" => "/srv/web" }
got = [StdlibProbe.registry("registry.yml"), StdlibProbe.registry("broken.yml"), StdlibProbe.registry("missing.yml")]
raise got.inspect unless got == [{ "projects" => { "web" => web, "copy" => web } }, {}, {}]
got = [StdlibProbe.refusal("broken.yml"), StdlibProbe.refusal("alias.yml"), StdlibProbe.refusal("symbol.yml")]
raise got.inspect unless got == ["syntax at line 2", "alias", "Tried to load unspecified class: Symbol"]
got = StdlibProbe.harden("<configuration><gui tls=\"true\"><address>x</address></gui></configuration>")
raise got.inspect unless got == ["true", "<configuration><gui tls='true'><address>x</address></gui><options><relaysEnabled>false</relaysEnabled></options></configuration>"]
got = StdlibProbe.script("hm", "/home/hm")
raise got.inspect unless got == "NAME=hm\nHOME=/home/hm\n"
got = StdlibProbe.fingerprint("registry.yml")
raise got.inspect unless got == Digest::MD5.hexdigest(File.binread("registry.yml"))[0, 10]
puts "stdlib probe passed"
"##;

#[test]
fn psych_rexml_erb_and_digest_file_run() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("stdlib probe passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn psych_rexml_erb_and_digest_file_run_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("stdlib probe passed"), "{}", run.stdout);
}
