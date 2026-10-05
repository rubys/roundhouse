#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn spinel_gate_date_column_crud_and_month_shifts_run() {
    let overlay = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\"",
            "  create_table \"calendar_entries\" do |t|\n    t.date \"due_on\"\n  end\n\n  create_table \"articles\"",
        )
        .write("app/models/calendar_entry.rb", include_str!("date_columns_model.rb"));
    let run = overlay.run_spinel(r#"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter

entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31))
entry.reload
raise entry.due_on.inspect unless entry.due_on.iso8601 == "2024-01-31"
raise entry.due_on_raw.inspect unless entry.due_on_raw == "2024-01-31"
raise entry.shifted(1).iso8601 unless entry.shifted(1).iso8601 == "2024-02-29"
raise entry.shifted(2).iso8601 unless entry.shifted(2).iso8601 == "2024-03-31"
raise entry.shifted(-1).iso8601 unless entry.shifted(-1).iso8601 == "2023-12-31"
raise entry.due_on.iso8601 unless entry.due_on.iso8601 == "2024-01-31"
puts "before selected date JSON"
raise entry.as_json(only: [:due_on]).inspect unless entry.as_json(only: [:due_on]) == {"due_on" => "2024-01-31"}
puts "before default date JSON"
raise entry.as_json.inspect unless entry.as_json["due_on"] == "2024-01-31"
raise Date.new(2024, 1, 31).as_json.inspect unless Date.new(2024, 1, 31).as_json == "2024-01-31"
raise Date.new(2024, 1, 31).wday.to_s unless Date.new(2024, 1, 31).wday == 3
raise Date.new(2024, 1, 31).yday.to_s unless Date.new(2024, 1, 31).yday == 31
raise Date.new(2024, 1, 31).monday?.to_s unless Date.new(2024, 1, 31).wednesday?
raise Date.new(2024, 1, 31).year.to_s unless Date.new(2024, 1, 31).year == 2024
raise Date.new(2024, 1, 31).month.to_s unless Date.new(2024, 1, 31).mon == 1
raise Date.new(2024, 1, 31).day.to_s unless Date.new(2024, 1, 31).mday == 31
raise Date.new(2024, 1, 31).strftime("%A %Y-%m-%d").inspect unless Date.new(2024, 1, 31).strftime("%A %Y-%m-%d") == "Wednesday 2024-01-31"
raise (Date.new(2024, 1, 31) >> 2).iso8601 unless (Date.new(2024, 1, 31) >> 2).iso8601 == "2024-03-31"
raise (Date.new(2024, 1, 31) << 1).iso8601 unless (Date.new(2024, 1, 31) << 1).iso8601 == "2023-12-31"
raise Date.iso8601("2024-01-31").iso8601 unless Date.iso8601("2024-01-31").iso8601 == "2024-01-31"
raise Date.civil(2024, 2, 29).iso8601 unless Date.civil(2024, 2, 29).iso8601 == "2024-02-29"
raise Date.parse("2024-02-29", false).iso8601 unless Date.parse("2024-02-29", false).iso8601 == "2024-02-29"
raise Date.strptime("2024-02-29", "%Y-%m-%d").iso8601 unless Date.strptime("2024-02-29", "%Y-%m-%d").iso8601 == "2024-02-29"
raise "leap year" unless Date.new(2024, 2, 29).leap?
raise "date ordering" unless Date.new(2024, 1, 31) < Date.new(2024, 2, 1)
raise "date equality" unless Date.new(2024, 1, 31) == Date.civil(2024, 1, 31)
raise Date.new(2024, 1, 31).to_date.iso8601 unless Date.new(2024, 1, 31).to_date.iso8601 == "2024-01-31"
raise Date.new(2024, 1, 31).to_time.strftime("%Y-%m-%d %H:%M:%S") unless Date.new(2024, 1, 31).to_time.strftime("%Y-%m-%d %H:%M:%S") == "2024-01-31 00:00:00"
begin
  Date.iso8601("2023-02-29")
  raise "invalid date was accepted"
rescue Date::Error
end
entry.update!(due_on: nil)
entry.reload
raise entry.due_on.inspect unless entry.due_on.nil?
raise entry.as_json(only: [:due_on]).inspect unless entry.as_json(only: [:due_on]) == {"due_on" => nil}
entry.update!(due_on: "")
entry.reload
raise entry.due_on.inspect unless entry.due_on.nil?
raise entry.as_json(only: [:due_on]).inspect unless entry.as_json(only: [:due_on]) == {"due_on" => nil}
entry.update!(due_on: Date.new(2024, 6, 15))
entry.reload
raise entry.due_on.iso8601 unless entry.due_on.iso8601 == "2024-06-15"
found = CalendarEntry.where(due_on: Date.new(2024, 6, 15)).to_a
raise found.map { |e| e.id }.inspect unless found.length == 1 && found[0].id == entry.id
miss = CalendarEntry.where(due_on: Date.new(2024, 6, 16)).to_a
raise miss.inspect unless miss.empty?
raise ActiveSupport.format_db_date(nil).inspect unless ActiveSupport.format_db_date(nil).nil?
raise ActiveSupport.parse_db_date("").inspect unless ActiveSupport.parse_db_date("").nil?
raise ActiveSupport.parse_db_date(nil).inspect unless ActiveSupport.parse_db_date(nil).nil?
raise ActiveSupport.format_db_date(Date.new(2024, 2, 29)).inspect unless ActiveSupport.format_db_date(Date.new(2024, 2, 29)) == "2024-02-29"
raise ActiveSupport.format_db_date("2024-02-29").inspect unless ActiveSupport.format_db_date("2024-02-29") == "2024-02-29"
raise ActiveSupport.format_db_date(nil).inspect unless ActiveSupport.format_db_date(nil).nil?
raise ActiveSupport.format_db_date("").inspect unless ActiveSupport.format_db_date("").nil?
begin
  ActiveSupport.format_db_date(42)
  raise "invalid DB date value was accepted"
rescue TypeError
end
begin
  ActiveSupport.parse_db_date("2024-02-30")
  raise "invalid parse_db_date was accepted"
rescue Date::Error
end
raise SqliteAdapter.escape_value(Date.new(2024, 2, 29)).inspect unless SqliteAdapter.escape_value(Date.new(2024, 2, 29)) == "'2024-02-29'"
raise Date.new(2024, 1, 31).inspect.inspect unless Date.new(2024, 1, 31).inspect == "2024-01-31"
raise Date.new(2024, 1, 31).xmlschema.inspect unless Date.new(2024, 1, 31).xmlschema == "2024-01-31"
puts "Spinel Date column contract passed"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("Spinel Date column contract passed"));
}
