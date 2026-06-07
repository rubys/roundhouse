package roundhouse

// Framework-runtime sample — the Kotlin shape of a file that is normally
// TRANSPILED from `runtime/ruby/inflector.rb` (the smallest Mode::Module
// entry, first in every target's RUNTIME table). Hand-written here in
// Phase R to lock the target shape: a Ruby module → a Kotlin `object` of
// pure functions.
//
// The lowered view calls `Inflector.pluralize(article.comments.size,
// "comment")` — the count-prefixed ActionView-style pluralize. Naive
// "+s" rule; the real transpiled inflector carries the full ruleset.
object Inflector {
    fun pluralize(count: Int, singular: String): String {
        val word = if (count == 1) singular else "${singular}s"
        return "$count $word"
    }
}
