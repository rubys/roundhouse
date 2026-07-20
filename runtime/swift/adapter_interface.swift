protocol AdapterInterface {
    func all(_ tableName: String) -> [[String: Any?]]
    func find(_ tableName: String, _ id: Int) -> [String: Any?]?
    func `where`(_ tableName: String, _ conditions: [String: Any?]) -> [[String: Any?]]
    func count(_ tableName: String) -> Int
    func existsPred(_ tableName: String, _ id: Int) -> Bool
    func insert(_ tableName: String, _ attributes: [String: Any?]) -> Int
    func update(_ tableName: String, _ id: Int, _ attributes: [String: Any?])
    func delete(_ tableName: String, _ id: Int)
    func truncate(_ tableName: String)
    func deleteAll(_ tableName: String)
}
