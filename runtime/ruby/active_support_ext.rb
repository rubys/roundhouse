# ActiveSupport blank-predicate reopen — ruby-family surface only
# (inflector_ext.rb pattern): shipped to the scaffold trees via the
# project.rs stems list, NOT in the runtime_loader tables, so the
# strict-target transpilers never see the `is_a?` dispatch below.
#
# `src/lower/blank.rs` grounds `blank?`/`present?`/`presence` by the
# receiver's static type and every target compiles the result. What it
# CANNOT ground is a receiver it has no type for — an untyped reader, an
# unresolved inference var, a multi-variant union. Those kept their
# dynamic `x.present?` call, which only CRuby could serve (the core_ext
# reopen of Object); on an AOT tree the send had nowhere to land, and
# lobsters' `Pushover.API_TOKEN.present?` — an unassigned `cattr_accessor`
# read, i.e. nil — took every /settings render down with it.
#
# So the residue routes HERE instead. Taking the receiver as an ARGUMENT
# is what makes this legal where the type-directed forms are not: the
# value is evaluated exactly once, so a receiver with effects grounds
# too, and no `respond_to?` is needed because the branch is on the value.
#
# Semantics are Rails', not an approximation — nil, false, and an empty
# String/Array/Hash are blank; `0` and `:sym` are present. Verified shape
# by shape against `Object#blank?`.
#
# The STRING tail is `strip.empty?`, not `empty?`: ActiveSupport's
# `String#blank?` is `/\A[[:space:]]*\z/`, so `" ".blank?` is true.
# `lower::blank` grounds the typed String sites the same way and for the
# same reason — the two must agree, because which of them serves a given
# call is a fact about type INFERENCE, not about the program. campfire's
# bot API is what priced it: `raw_request_body.blank?` reaches this
# function (the reader's type is not inferred), and a boost posted with
# a body of three spaces was accepted where Rails answers 422.
#
# `to_s` first, so the tail is one branch for every remaining shape: an
# Integer's `"0"` is not blank, a Symbol's `"sym"` is not blank, and a
# String is itself.
module ActiveSupport
  def self.blank?(value)
    return true if value.nil?
    return true if value == false
    return value.empty? if value.is_a?(Array)
    return value.empty? if value.is_a?(Hash)
    value.to_s.strip.empty?
  end

  def self.present?(value)
    !blank?(value)
  end

  def self.presence(value)
    blank?(value) ? nil : value
  end

  # Rails' `Object#presence_in(another)` — `in?(another) ? self : nil`,
  # the allow-list spelling for a value that came off the wire
  # (campfire's `params.require(:user)[:role].presence_in(%w[ member
  # administrator ])`). Grounded here rather than as a core_ext reopen
  # so every target has a method to dispatch; the list is a String
  # allow-list in every corpus site, which is what lets the parameter
  # be declared rather than left untyped.
  def self.presence_in(value, list)
    list.include?(value.to_s) ? value : nil
  end

  # Rails' `ActiveModel::Errors#[]` — the messages for ONE attribute,
  # without the humanized attribute prefix (`[ "is not public" ]`, not
  # `[ "Url is not public" ]`). Reached from `src/lower/errors_index.rs`,
  # which passes the prefix the `errors.add` / `validates` lowerings
  # baked in ("Url ", trailing space included so `url` cannot match
  # `url_host`'s "Url host …" on the space boundary alone).
  #
  # Takes the accumulator as an ARGUMENT for the same reason `blank?`
  # above does: the shared accumulator is a plain `Array[String]` and
  # Array has no `[]`-by-Symbol to reopen, so the projection has to be a
  # function over the array rather than a method on it.
  #
  # `m[prefix.length, m.length - prefix.length]` rather than a range or
  # `delete_prefix`: two-integer `String#[]` is the slice spelling every
  # target lowers, and `sub`/`delete_prefix` are the shapes that have
  # bitten this runtime before (a two-string `gsub` has no C# lowering).
  def self.errors_for(errors, prefix)
    out = []
    errors.each do |m|
      out << m[prefix.length, m.length - prefix.length] if m.start_with?(prefix)
    end
    out
  end

  # `ActiveSupport::MessageVerifier::InvalidSignature` — what a signed
  # value that does not verify raises. Rails hangs it off the verifier
  # class, and the NAME is the whole point: a controller that rescues it
  # is naming this class, so answering some other error means the rescue
  # never fires. campfire's `Users::AvatarsController` is
  # `rescue_from(ActiveSupport::MessageVerifier::InvalidSignature) {
  # head :not_found }` over an avatar URL that carries a signed id.
  #
  # A namespace with no verifier in it, because ours is
  # `ActionController::MessageVerifier` (it serves the cookie jar first).
  # The error keeps Rails' path so app code that names it resolves.
  class MessageVerifier
    class InvalidSignature < StandardError
      def initialize(message = "ActiveSupport::MessageVerifier::InvalidSignature")
        super(message)
      end
    end
  end

  # `list.index_by { |x| key }` — the collection as a Hash keyed by the
  # block's value, last write winning on duplicates (Rails' contract).
  #
  # ActiveSupport ships this as an `Enumerable` reopen, which only the
  # CRuby overlay can host. Grounded here by `lower::enumerable_ext`,
  # which takes the receiver as an ARGUMENT for the same reason
  # `blank?` above does: the collection is evaluated exactly once.
  #
  # The `ActiveRecord::Relation` twin (relation.rb) stays where it is —
  # it has a real RBS signature, and routing a typed receiver through
  # this untyped one would trade a typed call for an untyped one to fix
  # nothing. Same body shape as that twin, deliberately: `h[yield x] =
  # x` inside an `each` is the form every target's emitter already
  # compiles.
  def self.index_by(list)
    h = {}
    list.each { |x| h[yield x] = x }
    h
  end

  # AS `Enumerable#many?`, no-block form: MORE THAN ONE element. Rails
  # writes it as a short-circuiting `any?` with a counter so it stops at
  # the second hit; the receivers that reach here are already
  # materialized, so `length` answers the same question without the
  # block. Another core_ext reopen the transpiled runtimes cannot host —
  # same home and same rule as `index_by` above, the receiver evaluated
  # exactly once.
  #
  # The block form (`many? { … }`) is NOT here: it counts matches
  # instead, and no corpus app writes it. `lower::enumerable_ext`
  # rewrites only the bare call, so the block form stays visible.
  def self.many?(list)
    list.length > 1
  end
end
