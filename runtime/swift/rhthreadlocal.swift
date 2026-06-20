import NIOPosix

final class RhThreadLocal<T> {
    private final class Box {
        var value: T
        init(_ value: T) { self.value = value }
    }

    private let tsv = ThreadSpecificVariable<Box>()
    private let makeDefault: () -> T

    init(_ makeDefault: @escaping () -> T) {
        self.makeDefault = makeDefault
    }

    var value: T {
        get {
            if let box = tsv.currentValue { return box.value }
            let box = Box(makeDefault())
            tsv.currentValue = box
            return box.value
        }
        set {
            if let box = tsv.currentValue {
                box.value = newValue
            } else {
                tsv.currentValue = Box(newValue)
            }
        }
    }
}
