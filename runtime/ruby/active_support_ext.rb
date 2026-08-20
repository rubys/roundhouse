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
module ActiveSupport
  def self.blank?(value)
    return true if value.nil?
    return true if value == false
    return value.empty? if value.is_a?(Array)
    return value.empty? if value.is_a?(Hash)
    value.to_s.empty?
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
end
