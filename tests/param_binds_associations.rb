# Real sentinel-key rows distinguish nil from accidental 0/"" coercion.
# The Rails source has no explicit nil assignment to supply inferred
# nilability to a synthesized association's Arel RHS. These assignments
# happen after emission, so only the reader's own guard can protect it.
Db.exec("INSERT INTO accounts (id, name) VALUES (0, 'zero trap'), (7, 'present')")
Db.exec("INSERT INTO articles (id, title) VALUES ('', 'empty trap'), ('00000000-0000-0000-0000-000000000007', 'present')")
Db.exec("INSERT INTO links (id, account_id, article_id) VALUES (1, NULL, NULL), (2, 7, '00000000-0000-0000-0000-000000000007')")
Db.exec("INSERT INTO taggings (id, taggable_id, taggable_type) VALUES (1, NULL, 'Account'), (2, 7, 'Account'), (3, NULL, NULL)")

8.times do
  absent = Link.new
  absent.account_id = nil
  absent.article_id = nil
  untagged = Tagging.new
  untagged.taggable_id = nil
  untagged.taggable_type = "Account"
  cache_before = Db.gate_cache_size
  raise "nil integer FK resolved an association" unless absent.account.nil?
  raise "nil UUID FK resolved an association" unless absent.article.nil?
  raise "nil polymorphic FK resolved an association" unless untagged.taggable.nil?
  raise "nil FK prepared a query" unless Db.gate_cache_size == cache_before
  present = Link.find(2)
  account = present.account
  article = present.article
  raise "present integer FK lost" if account.nil?
  raise "present integer FK changed" unless account.id == 7
  raise "present UUID FK lost" if article.nil?
  raise "present UUID FK changed" unless article.id == "00000000-0000-0000-0000-000000000007"
  tagged = Tagging.find(2).taggable
  raise "present polymorphic FK lost" if tagged.nil?
  raise "present polymorphic FK changed" unless tagged.id == 7
  untagged.taggable_type = nil
  raise "nil polymorphic type resolved an association" unless untagged.taggable.nil?
end
puts "emit: explicit nil integer/UUID/polymorphic association readers and nonnil controls passed"
