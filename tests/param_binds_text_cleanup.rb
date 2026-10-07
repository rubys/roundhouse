# Run with either CRuby's or JRuby's Db shim preloaded via -r.
def cleanup_owned_count
  conn = Db.current_dbh
  if RUBY_ENGINE == "jruby"
    conn.open_statements.size
  else
    Db.open_statements(conn).size
  end
end

class CleanupBadText
  def to_s
    raise "text conversion failed"
  end
end

Db.configure(":memory:")
Db.with_connection do
  ["x".encode("UTF-16LE"), CleanupBadText.new].each do |value|
    3.times do
      stmt = Db.prepare("SELECT ? AS preprocessing_failure")
      begin
        Db.bind_text(stmt, 1, value)
        raise "expected text preprocessing failure"
      rescue StandardError => error
        raise error unless error.message.include?("incompatible character encodings") || error.message.include?("text conversion failed")
      end
      count = cleanup_owned_count
      raise "text preprocessing left #{count} owned statements inside the lease" unless count == 0
      Db.finalize(stmt) # Idempotent after the binder's own failure cleanup.
    end
  end
end
puts "cleanup: 6 text preprocessing failures; every owned count was zero before lease end"
Db.close
