use super::emit_and_run;

#[test]
fn concurrent_map_runs_in_emitted_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  LOCKS = Concurrent::Map.new

  def self.lock_for(id) = LOCKS.compute_if_absent(id) { Mutex.new }
  def self.forget_locks = LOCKS.clear

  class << self
    def remember(id, at) = seen[id] = at
    def seen_at(id) = seen[id]

    private

    def seen
      @seen ||= Concurrent::Map.new
    end
  end
",
        )
        .run_ruby(
            r#"lock = Article.lock_for(1)
raise "same lock" unless lock.is_a?(Mutex) && Article.lock_for(1).equal?(lock)
raise "other lock" if Article.lock_for(2).equal?(lock)
raise "clear answers the map" unless Article.forget_locks.equal?(Article::LOCKS)
raise "cleared" if Article.lock_for(1).equal?(lock)
at = Time.at(0)
raise "[]= answers the value" unless Article.remember(7, at).equal?(at)
raise "[]" unless Article.seen_at(7).equal?(at) && Article.seen_at(8).nil?
puts "concurrent map passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("concurrent map passed"));
}
