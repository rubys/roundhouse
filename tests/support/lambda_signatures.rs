//! One lambda-signature contract shared by the interpreted and native
//! output lanes: a model method that defines lambdas with optional,
//! keyword and keyword-rest parameters and calls each with and without
//! them. `EXPECTED` is what plain Ruby prints for the same calls.

pub const MODEL_METHOD: &str = r##"
  def self.lambda_signatures
    label = ->(name, size = 18) { "#{name}:#{size}" }
    pick = ->(word, key:, limit: 10) { "#{word}-#{key}-#{limit}" }
    opts = ->(ctx, **opts) { "#{ctx}:" + opts.keys.map(&:to_s).sort.join(",") }
    anon = ->(represented:, **) { represented }
    all = ->(a, b = 2, *rest, key: 1, **more) { "#{a}/#{b}/#{rest.join("+")}/#{key}/#{more.size}" }
    [label.call("a"), label.call("a", 3), pick.call("w", key: :x, limit: 2),
     pick.call("w", key: :x), opts.call(1, b: 2, a: 1), anon.call(represented: "r", extra: 1),
     all.call(1), all.call(1, 5, 6, 7, key: 9, z: 0),
     [1, 2].map { |n, bonus = 2| n + bonus }.sum,
     [[1, 2], [3, 4]].map { |a, b, scale: 10| (a + b) * scale }.sum].join("|")
  end
"##;

pub const EXPECTED: &str = "a:18|a:3|w-x-2|w-x-10|1:a,b|r|1/2//1/0|1/5/6+7/9/1|7|100";

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord\n",
        &format!("class Article < ApplicationRecord\n{MODEL_METHOD}"),
    )
}

pub fn assertions() -> String {
    format!(
        "got = Article.lambda_signatures\nraise \"expected {EXPECTED}, got #{{got}}\" unless got == {EXPECTED:?}\nputs \"lambda signatures OK\"\n"
    )
}
