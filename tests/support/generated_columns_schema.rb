ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || coalesce(last_name, '')", stored: true
    t.virtual "normalized_first_name", type: :string, as: "coalesce(first_name, '')", stored: true, null: false
  end

  create_table "virtual_people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || coalesce(last_name, '')", stored: false
  end

  create_table "constant_people", force: :cascade do |t|
    t.virtual "display_name", type: :string, as: "'constant'", stored: true
  end
end
