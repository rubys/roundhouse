package roundhouse

// Not mapped to an existing JVM exception: a `rescue NoMethodError` would then catch that unrelated type too
open class NoMethodError(message: String? = null) : RuntimeException(message)

open class NameError(message: String? = null) : RuntimeException(message)

// Not inline `let` chains at each site: nested inside an inlined forEach they crash the JVM backend's inliner
fun <T> rhListSlice(list: List<T>, start: Long, length: Long): MutableList<T>? {
    val n = list.size.toLong()
    val b = if (start < 0) start + n else start
    if (b < 0 || b > n || length < 0) return null
    val e = minOf(b + minOf(length, n), n)
    return list.subList(b.toInt(), e.toInt()).toMutableList()
}

// Not by UTF-16 unit: Ruby indexes characters, and a character past the BMP is two units
fun rhStrAt(s: String, index: Long): String? {
    val n = s.codePointCount(0, s.length).toLong()
    val i = if (index < 0) index + n else index
    if (i < 0 || i >= n) return null
    val from = s.offsetByCodePoints(0, i.toInt())
    return s.substring(from, s.offsetByCodePoints(from, 1))
}
