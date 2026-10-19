//! A parenthesized `(a; b)` in a value position runs where Ruby runs it:
//! after the values to its left and before those to its right, and the
//! right of `&&` only when the left lets it. A model method's body is
//! where the lowering lifts such a Seq ahead of its statement.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true\n\n  def self.in_order\n    log = []\n    [(log << 1; log.size), (log << 2; log.size), log.size]\n  end\n\n  def self.short_circuit\n    log = []\n    ok = log.empty? && false && (log << 3; true)\n    [log.size, ok ? 1 : 0]\n  end\n",
    )
}

const ASSERTIONS: &str = r##"got = [Article.in_order, Article.short_circuit]
raise got.inspect unless got == [[1, 2, 2], [0, 0]]
puts "value seq order passed"
"##;

#[test]
fn a_value_seq_runs_in_order() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_value_seq_runs_in_order_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}
