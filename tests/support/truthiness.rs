//! Ruby truthiness on the TypeScript emit: only nil and false are false,
//! where JS also treats `0`, `NaN` and `""` as false, and `a || b` /
//! `a && b` answer an operand by Ruby's test.

use super::emit_and_run::{self, Overlay};

pub const PROBE: &str = r#"class TruthProbe
  def self.run(zero, empty, none, no, list, words)
    out = []
    out << (zero ? "t" : "f") << (empty ? "t" : "f") << (none ? "t" : "f") << (no ? "t" : "f") << (list ? "t" : "f")
    out << (zero || 5) << (empty || "x") << (none || 5) << (no || 5) << (list || 5)
    out << (zero && 5) << (empty && "x") << (none && 5) << (no && 5)
    out << !zero << !empty << !none << !no
    out << "if0" if zero
    out << "unless" unless empty
    maybe = zero > 1 ? nil : zero
    mixed = zero > 1 ? false : zero
    out << (maybe ? "t" : "f") << (mixed ? "t" : "f") << (mixed || 9) << (mixed && 9) << (maybe || 9)
    k = 0
    while k && k < 2
      k += 1
    end
    out << k
    j = zero
    j ||= 7
    out << j
    out << (zero.to_s.length > 0 && empty ? "both" : "not")
    out << words.select { |w| w } << [0, 1, nil].select { |x| x } << [0, 1, nil].reject { |x| x }
    out << [nil, 0].find { |x| x } << [0, nil].count { |x| x } << ["", 0].all? { |x| x } << [nil, 0].any? { |x| x }
    out << [nil, 0].index { |x| x } << [0, ""].none? { |x| x }
    flags = [0, false, nil, ""]
    out << (flags.first || 5) << (flags.last && 5) << (flags[1] || 5) << (flags.first ? "t" : "f") << flags.select { |f| f }
    out
  end
end
"#;

/// `JSON.generate(TruthProbe.run(0, "", nil, false, [], ["", "a"]))` under CRuby.
#[allow(dead_code)]
pub const EXPECTED: &str = r#"["t","t","f","f","t",0,"",5,5,[],5,"x",null,false,false,false,true,true,"if0","t","t",0,9,0,2,0,"both",["","a"],[0,1],[null],0,1,true,true,1,false,0,5,5,"t",[0,""]]"#;

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/truth_probe.rb", PROBE).edit(
        "app/controllers/articles_controller.rb",
        "  def index\n",
        "  def index\n    @truth = TruthProbe.run(0, \"\", nil, false, [], [\"\", \"a\"])\n",
    )
}
