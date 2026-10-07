// Native exception carrier for the shared Router's invalid-capture checks.
// This is separate from application ArgumentError and changes no validation.
struct RoutePathEncodingError: Error, CustomStringConvertible {
    let message: String
    /// Retain the shared decoder's message across the native error boundary.
    init(_ message: String = "Invalid encoding for path parameter") {
        self.message = message
    }
    /// Diagnostics report the same message as the shared Ruby exception.
    var description: String { message }
}
