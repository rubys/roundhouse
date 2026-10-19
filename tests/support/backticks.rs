//! One contract for a backtick literal, shared by the interpreted (CRuby
//! overlay) and native (spinel) lanes. Ruby evaluates `` `cmd` `` and
//! `%x{cmd}` as a call of `Kernel#`` with the interpolated command: the
//! result is the command's standard output, and `$?` holds its status.
//! Ingest used to refuse the literal, and `--allow-unsupported` emitted
//! `nil` in its place.
//!
//! The expected lines are CRuby 4.0's output for the same code.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().write("app/models/shell_probe.rb", SOURCE)
}

const SOURCE: &str = r##"class ShellProbe
  def self.echo(word)
    `printf '%s' #{word}`
  end

  def self.percent_x
    %x{printf 'a b'}.split(" ")
  end
end
"##;

pub const SCRIPT: &str = r##"
p ShellProbe.echo("hi")
p $?.success?
p ShellProbe.percent_x
"##;

pub const EXPECTED: &str = "\"hi\"\ntrue\n[\"a\", \"b\"]\n";
