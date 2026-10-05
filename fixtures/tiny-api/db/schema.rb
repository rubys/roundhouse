ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "widgets", id: :uuid, force: :cascade do |t|
    t.string "name", null: false
    t.integer "status", default: 0, null: false
    t.timestamps
  end

  create_table "parts", id: :uuid, force: :cascade do |t|
    t.uuid "widget_id", null: false
    t.string "name"
    t.timestamps
  end
end
