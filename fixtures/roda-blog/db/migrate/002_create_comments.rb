Sequel.migration do
  change do
    create_table(:comments) do
      primary_key :id
      foreign_key :article_id, :articles, null: false, on_delete: :cascade
      String :commenter, null: false
      String :body, text: true, null: false
      DateTime :created_at, null: false
      DateTime :updated_at, null: false
      index :article_id
    end
  end
end
