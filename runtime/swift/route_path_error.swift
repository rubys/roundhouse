// Native exception carrier for the shared Router's invalid-capture checks.
// An ArgumentError, as the shared Ruby raises it, so a rescue of ArgumentError
// still catches it; the server answers this one class with a 400.
class RoutePathEncodingError: ArgumentError {
    /// Retain the shared decoder's message across the native error boundary.
    override init(_ message: String = "Invalid encoding for path parameter") {
        super.init(message)
    }
}
