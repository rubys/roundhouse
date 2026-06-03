-- Seed data for the demo blog, for self-contained reproduction from the
-- published archive (the archive is text-only, so no binary DB is shipped).
-- Schema matches config/schema.rb (the binary's Schema.load! is idempotent).
-- Regenerate from a seeded fixture with scripts/… or sqlite3 .dump if data changes.
CREATE TABLE IF NOT EXISTS articles (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT, created_at TEXT NOT NULL, title TEXT, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS comments (id INTEGER PRIMARY KEY AUTOINCREMENT, article_id INTEGER NOT NULL, body TEXT, commenter TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS index_comments_on_article_id ON comments (article_id);
INSERT INTO articles VALUES(1,'Rails is a web application framework running on the Ruby programming language. It makes building web apps faster and easier with conventions over configuration.','2026-05-15 21:14:56.300213','Getting Started with Rails','2026-05-15 21:14:56.300213');
INSERT INTO articles VALUES(2,'MVC stands for Model-View-Controller. Models handle data and business logic, Views display information to users, and Controllers coordinate between them.','2026-05-15 21:14:56.382238','Understanding MVC Architecture','2026-05-15 21:14:56.382238');
INSERT INTO articles VALUES(3,'Ruby2JS transpiles Ruby to JavaScript, enabling Rails applications to run in browsers, on Node.js, and at the edge. Same code, different runtimes.','2026-05-15 21:14:56.386016','Ruby2JS: Rails Everywhere','2026-05-15 21:14:56.386016');
INSERT INTO comments VALUES(1,1,'Great introduction! Rails really does make development faster.','Alice','2026-05-15 21:14:56.328046','2026-05-15 21:14:56.328046');
INSERT INTO comments VALUES(2,1,'I love how Rails handles database migrations automatically.','Bob','2026-05-15 21:14:56.379600','2026-05-15 21:14:56.379600');
INSERT INTO comments VALUES(3,2,'This pattern really helps keep code organized!','Carol','2026-05-15 21:14:56.383950','2026-05-15 21:14:56.383950');
