// Framework-runtime sample — the Swift shape of a file that is normally
// TRANSPILED from `runtime/ruby/inflector.rb` (the smallest Mode::Module
// entry, first in every target's RUNTIME table). Hand-written here in
// Phase R to lock the target shape: a Ruby module → a Swift caseless
// `enum` of static functions (the idiomatic namespace; Swift has no
// `object` keyword — plan delta 2).
//
// The lowered view calls `Inflector.pluralize(article.comments.count,
// "comment")` — the count-prefixed ActionView-style pluralize. Naive
// "+s" rule; the real transpiled inflector carries the full ruleset.
enum Inflector {
    static func pluralize(_ count: Int, _ singular: String) -> String {
        let word = count == 1 ? singular : "\(singular)s"
        return "\(count) \(word)"
    }
}
