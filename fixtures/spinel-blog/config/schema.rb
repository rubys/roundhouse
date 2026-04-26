module Schema
  STATEMENTS = [
    <<~SQL,
      CREATE TABLE IF NOT EXISTS articles (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT,
        body TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      )
    SQL
    <<~SQL,
      CREATE TABLE IF NOT EXISTS comments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        article_id INTEGER NOT NULL,
        commenter TEXT,
        body TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      )
    SQL
    "CREATE INDEX IF NOT EXISTS index_comments_on_article_id ON comments (article_id)",
  ].freeze

  def self.load!(adapter)
    STATEMENTS.each { |sql| adapter.execute_ddl(sql) }
  end
end
