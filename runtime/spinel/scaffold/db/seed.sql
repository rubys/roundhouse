-- Seed data for the demo blog, for self-contained reproduction from the
-- published archive (the archive is text-only, so no binary DB is shipped).
-- Schema matches config/schema.rb (the binary's Schema.load! is idempotent).
-- Regenerate from a seeded fixture with scripts/… or sqlite3 .dump if data changes.
--
-- This file ships in EVERY target archive (injected by src/project.rs
-- target_files), so the seed step is language-agnostic — `sqlite3 <db> <
-- db/seed.sql` populates a fresh DB regardless of which target serves it.
-- INSERTs name their columns explicitly so the file stays valid even if a
-- target emits its schema columns in a different order than another.
CREATE TABLE IF NOT EXISTS articles (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT, created_at TEXT NOT NULL, title TEXT, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS comments (id INTEGER PRIMARY KEY AUTOINCREMENT, article_id INTEGER NOT NULL, body TEXT, commenter TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS index_comments_on_article_id ON comments (article_id);
INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (1,'Getting Started with Rails','Rails is a web application framework running on the Ruby programming language. It makes building web apps faster and easier with conventions over configuration.','2026-05-15 21:14:56.300213','2026-05-15 21:14:56.300213');
INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (2,'Understanding MVC Architecture','MVC stands for Model-View-Controller. Models handle data and business logic, Views display information to users, and Controllers coordinate between them.','2026-05-15 21:14:56.382238','2026-05-15 21:14:56.382238');
INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (3,'Ruby2JS: Rails Everywhere','Ruby2JS transpiles Ruby to JavaScript, enabling Rails applications to run in browsers, on Node.js, and at the edge. Same code, different runtimes.','2026-05-15 21:14:56.386016','2026-05-15 21:14:56.386016');
INSERT INTO comments (id, article_id, commenter, body, created_at, updated_at) VALUES (1,1,'Alice','Great introduction! Rails really does make development faster.','2026-05-15 21:14:56.328046','2026-05-15 21:14:56.328046');
INSERT INTO comments (id, article_id, commenter, body, created_at, updated_at) VALUES (2,1,'Bob','I love how Rails handles database migrations automatically.','2026-05-15 21:14:56.379600','2026-05-15 21:14:56.379600');
INSERT INTO comments (id, article_id, commenter, body, created_at, updated_at) VALUES (3,2,'Carol','This pattern really helps keep code organized!','2026-05-15 21:14:56.383950','2026-05-15 21:14:56.383950');
