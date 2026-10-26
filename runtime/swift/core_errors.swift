// Ruby's core errors a runtime raises, as Swift errors a `rescue` can catch.
// Not StandardError, RuntimeError or Exception: an app's own error class
// conforms to Error rather than subclassing them, so a catch on one of those
// would miss it. Not NotImplementedError either: the runtime raises it from
// a method every subclass overrides, and a throwing base would make each
// caller of the override throw.
class ArgumentError: Error, CustomStringConvertible {
    let message: String
    init(_ message: String = "ArgumentError") { self.message = message }
    var description: String { message }
}

class TypeError: Error, CustomStringConvertible {
    let message: String
    init(_ message: String = "TypeError") { self.message = message }
    var description: String { message }
}

class NameError: Error, CustomStringConvertible {
    let message: String
    init(_ message: String = "NameError") { self.message = message }
    var description: String { message }
}

class NoMethodError: NameError {
    override init(_ message: String = "NoMethodError") { super.init(message) }
}

class IndexError: Error, CustomStringConvertible {
    let message: String
    init(_ message: String = "IndexError") { self.message = message }
    var description: String { message }
}

class KeyError: IndexError {
    override init(_ message: String = "KeyError") { super.init(message) }
}
