# frozen_string_literal: true
require_relative "i18n_locale_tag"
require_relative "i18n_host"
# `I18n.locale` / `I18n.default_locale` / `I18n.available_locales`, the
# locale a request runs in, and the answers for a key the locale lacks.
#
# Translation is folded at compile time (`src/lower/i18n_translate.rs`),
# once per locale the app makes available; a call that can run in more
# than one picks its branch by `I18n.locale` when it runs. The locale is
# per thread, as the gem holds it (`I18n.config` is thread-local), and
# is the default until something sets it. The app's default and
# available locales are spelled into the generated block below
# (`project::apply_i18n_locales`); an app that sets none keeps Rails'
# `:en`.
#
# `I18n.with_locale(l) { … }` returns its block's value, which no
# signature here can type, so `lower::i18n_translate` spells each call
# site out over `enter_locale` / `leave_locale`.
module I18n
  # `I18n.locale = :xx` for a locale the app does not make available
  # (the gem's `enforce_available_locales`, true by default).
  class InvalidLocale < ArgumentError
  end

  def self.default_locale
    app_default_locale
  end

  def self.available_locales
    app_available_locales
  end

  # The gem's set holds each locale as a String and a Symbol, so "en" and :en answer alike.
  def self.locale_available?(locale)
    !locale.nil? && !available_named(locale.to_s).nil?
  end

  # Not `name.to_sym`: Crystal cannot make a Symbol when it runs, and every locale that passes is one of these.
  def self.available_named(name)
    app_available_locales.find { |l| l.to_s == name }
  end

  # The locale set for this thread, by name ("" when none is): a Symbol
  # read back out of `Thread.current` is a value the strict targets
  # cannot narrow, and a String they can. (Not `locale_set`: Spinel
  # spells `locale=` with that name.)
  def self.thread_locale_name
    I18nHost.locale_name
  end

  def self.locale
    found = available_named(thread_locale_name)
    found.nil? ? app_default_locale : found
  end

  # `I18n.locale = x`: an available locale, a Symbol or its String.
  def self.locale=(value)
    I18nHost.set_locale_name(checked_locale(value).to_s)
  end

  # `value` as the locale it names, or the gem's InvalidLocale, whose
  # message shows the value as `inspect` does (nil is not a locale).
  def self.checked_locale(value)
    found = value.nil? ? nil : available_named(value.to_s)
    return found unless found.nil?
    # Not `value.to_s` in each arm: a strict target narrows `value` to a Symbol there and loses the conversion
    name = value.to_s
    shown = value.nil? ? "nil" : (value.is_a?(Symbol) ? ":" + name : "\"" + name + "\"")
    raise InvalidLocale, shown + " is not a valid locale"
  end

  # A call's `locale:` read when it runs, by name: nil is the current
  # locale, anything else must be available.
  def self.locale_name(value)
    value.nil? ? locale.to_s : checked_locale(value).to_s
  end

  # `with_locale(l)`'s first half: nil changes nothing and answers nil,
  # anything else is set and the locale it replaces answered, for
  # `leave_locale` to put back however the block ends.
  def self.enter_locale(value)
    return nil if value.nil?
    previous = locale.to_s
    I18nHost.set_locale_name(checked_locale(value).to_s)
    previous
  end

  # Not `set_locale_name(previous) unless previous.nil?`: a strict target does not narrow the tested local
  def self.leave_locale(previous)
    return nil if previous.nil?
    I18nHost.set_locale_name(previous.to_s)
    nil
  end

  # Whether the running environment is one of `envs` (comma-separated),
  # where `config.i18n.fallbacks` differs by environment.
  def self.running_in?(envs)
    envs.split(",").include?(I18nHost.env_name)
  end

  class MissingTranslationData < ArgumentError
  end

  # Whether this environment raises on a missing translation: `raises`
  # is `config/application.rb`'s setting, and the comma-separated
  # `flipped` environments set the other value in their own file.
  def self.missing_raises(raises, flipped)
    env = I18nHost.env_name
    flipped.split(",").include?(env) ? !raises : raises
  end

  # A run-time key as `I18n.normalize_keys` reads it: dot-separated,
  # empty segments dropped (`"a..b."` is `a.b`).
  def self.normalize_key(key)
    out = []
    key.split(".").each { |part| out << part unless part.empty? }
    out.join(".")
  end

  # A run-time key that names a subtree, which `I18n.t` answers as a
  # Hash; no folded translation carries one.
  def self.subtree_translation(key)
    raise NotImplementedError, "I18n.t(\"#{key}\") answers a Hash of translations, which is not supported"
  end

  # `I18n.t`'s answer for `key` (`en.shop.title`): I18n's default
  # handler returns the message, `raise_on_missing_translations` raises it.
  def self.missing_translation(key, raises)
    message = "Translation missing: " + key
    raise MissingTranslationData, message if raises
    message
  end

  # The same answer when the call's `default:` was a list of keys none of
  # which is there either: I18n lists every key it considered, `rest`
  # being the defaults' lines (`"\n- en.posts.title"`).
  def self.missing_translation_among(key, rest, raises)
    message = "Translation missing. Options considered were:\n- " + key + rest
    raise MissingTranslationData, message if raises
    message
  end

  # A template's `t` for `key`: Action View's debug span, titled with the
  # key and the call's options (`details`, each value escaped once), and
  # reading as the key's last segment titleized.
  def self.missing_translation_tag(key, details, raises)
    missing_translation(key, raises)
    last = key.split(".").last.to_s
    title = ActionView::ViewHelpers.html_escape("translation missing: " + key + details)
    text = ActionView::ViewHelpers.html_escape(ActiveSupport.titleize(last).to_s)
    "<span class=\"translation_missing\" title=\"" + title + "\">" + text + "</span>"
  end

  class MissingInterpolationArgument < ArgumentError
  end

  class InvalidPluralizationData < ArgumentError
  end

  # `I18n.t(key, **options)` as the gem runs it, for a call whose key,
  # scope, default or options are known only when it runs: the key under
  # its scope (a view's or controller's lazy `.key` under `lazy_scope`),
  # each locale of the fallback chain, then each `default:`, and the
  # missing answer when none is found.
  def self.translate_runtime(key, options, lazy_scope, in_view, raises)
    loc = options.key?(:locale) ? locale_name(options[:locale]) : locale.to_s
    scope = scope_text(options[:scope])
    name = key.to_s
    name = lazy_scope + name if name.start_with?(".") && !lazy_scope.empty?
    full = normalize_key(scope.empty? ? name : scope + "." + name)
    count = options[:count]
    found = runtime_lookup(loc, full, count)
    considered = []
    defaults = options[:default]
    if found.nil? && !defaults.nil?
      list = defaults.is_a?(Array) ? defaults : [defaults]
      list.each do |default|
        next unless found.nil?
        if default.is_a?(Symbol)
          k = normalize_key(scope.empty? ? default.to_s : scope + "." + default.to_s)
          considered << loc + "." + k
          found = runtime_lookup(loc, k, count)
        elsif !default.nil?
          found = default.to_s
        end
      end
    end
    html = in_view && html_safe_key?(full)
    if found.nil?
      shown = loc + "." + full
      return missing_translation_tag(shown, "", raises) if in_view
      if defaults.is_a?(Array) && !considered.empty?
        return missing_translation_among(shown, considered.map { |c| "\n- " + c }.join, raises)
      end
      return missing_translation(shown, raises)
    end
    out = interpolate_runtime(found, options, html)
    in_view && !html ? ActionView::ViewHelpers.html_escape(out) : out
  end

  # A node of Strings as I18n answers it: a nested Hash keyed by Symbols,
  # in the files' order, from the first locale of the chain that has it.
  def self.translate_tree(full, options)
    loc = options.key?(:locale) ? locale_name(options[:locale]) : locale.to_s
    leaves = runtime_leaves
    empties = runtime_empties
    runtime_chain(loc).each do |l|
      next unless runtime_nodes.key?(l + "." + full)
      prefix = l + "." + full + "."
      tree = {}
      runtime_order.each do |k|
        next unless k.start_with?(prefix)
        path = k[prefix.length, k.length - prefix.length].to_s.split(".")
        if empties.key?(k)
          insert_empty(tree, path)
        else
          insert_tree(tree, path, leaves[k].to_s)
        end
      end
      return tree
    end
    {}
  end

  # An Array, a number or a boolean in the files, which I18n answers as it is: its kind letter, then its fields, from the first locale of the chain that has it.
  def self.opaque_fields(full, options)
    loc = options.key?(:locale) ? locale_name(options[:locale]) : locale.to_s
    opaque = runtime_opaque
    runtime_chain(loc).each do |l|
      found = opaque.fetch(l + "." + full, nil)
      return found unless found.nil?
    end
    []
  end

  def self.translate_items(full, options)
    found = opaque_fields(full, options)
    return [] if found.length < 2
    items = found[1, found.length - 1] || []
    items.map { |item| item.start_with?("y") ? item[1, item.length - 1].to_s.to_sym : item[1, item.length - 1].to_s }
  end

  def self.translate_strings(full, options)
    found = opaque_fields(full, options)
    return [] if found.length < 2
    items = found[1, found.length - 1] || []
    items.map { |item| item[1, item.length - 1].to_s }
  end

  def self.translate_symbols(full, options)
    translate_strings(full, options).map { |item| item.to_sym }
  end

  def self.translate_integer(full, options)
    opaque_fields(full, options)[1].to_s.to_i
  end

  def self.translate_float(full, options)
    opaque_fields(full, options)[1].to_s.to_f
  end

  def self.translate_boolean(full, options)
    opaque_fields(full, options)[1].to_s == "true"
  end

  # An empty mapping in the files (`feed: {}`), which I18n answers as `{}`.
  def self.insert_empty(tree, path)
    node = tree
    i = 0
    while i < path.length
      key = path[i].to_s.to_sym
      child = node.fetch(key, nil)
      unless child.is_a?(Hash)
        child = {}
        node[key] = child
      end
      node = child
      i += 1
    end
    nil
  end

  def self.insert_tree(tree, path, value)
    node = tree
    i = 0
    while i < path.length - 1
      key = path[i].to_s.to_sym
      child = node.fetch(key, nil)
      unless child.is_a?(Hash)
        child = {}
        node[key] = child
      end
      node = child
      i += 1
    end
    node[path[path.length - 1].to_s.to_sym] = value
    nil
  end

  def self.scope_text(scope)
    return "" if scope.nil?
    return scope.map { |part| part.to_s }.join(".") if scope.is_a?(Array)
    scope.to_s
  end

  def self.html_safe_key?(key)
    last = key.split(".").last.to_s
    last == "html" || last.end_with?("_html")
  end

  # The translation `full` names in the first locale of `loc`'s chain that
  # has it: a leaf, or the plural form `count` picks from its children.
  def self.runtime_lookup(loc, full, count)
    leaves = runtime_leaves
    runtime_chain(loc).each do |l|
      text = leaves.fetch(l + "." + full, nil)
      return text unless text.nil?
      raise NotImplementedError, "I18n.t(\"#{full}\") answers a value that is not a String, which is not supported" if runtime_opaque.key?(l + "." + full)
      next unless runtime_nodes.key?(l + "." + full)
      return plural_form(leaves, l, l + "." + full, count) unless count.nil?
      subtree_translation(full)
    end
    nil
  end

  # Not `entry[form]` alone: with a locale's rule, I18n's Pluralization backend takes the `zero`, `0` and `1` keys first and `other` for a form the entry lacks.
  def self.plural_form(leaves, loc, node, count)
    zero = leaves.fetch(node + ".zero", nil)
    return zero if count == 0 && !zero.nil?
    rule = plural_rule(loc)
    if rule.nil?
      form = count == 1 ? "one" : "other"
      text = leaves.fetch(node + "." + form, nil)
      raise InvalidPluralizationData, "translation data #{node} can not be used with :count => #{count}. key '#{form}' is missing." if text.nil?
      return text
    end
    if count == 0 || count == 1
      exact = leaves.fetch(node + "." + (count == 0 ? "0" : "1"), nil)
      return exact unless exact.nil?
    end
    form = plural_category(rule, count)
    text = leaves.fetch(node + "." + form, nil) || leaves.fetch(node + ".other", nil)
    raise InvalidPluralizationData, "translation data #{node} can not be used with :count => #{count}. key '#{form}' is missing." if text.nil?
    text
  end

  def self.plural_rule(loc)
    rules = runtime_rules
    runtime_chain(loc).each do |l|
      rule = rules.fetch(l, nil)
      return rule unless rule.nil?
    end
    nil
  end

  # The operands CLDR rules read (n i v f t) and rails-i18n's (r, and d for a fraction's digits), all from the count as Ruby prints it.
  def self.plural_category(rule, count)
    numeric = count.is_a?(Integer) || count.is_a?(Float)
    printed = numeric ? count.abs.to_s : count.to_s
    printed = printed[1, printed.length - 1].to_s if printed.start_with?("-")
    digits = printed.split(".")[1]
    fraction = digits.nil? ? "" : digits.to_s
    trimmed = fraction.sub(/0+\z/, "")
    raw = numeric ? count.to_f : 0.0
    ops = {}
    ops["r"] = raw
    ops["n"] = numeric ? count.abs.to_f : printed.to_f
    ops["i"] = (numeric ? count.abs.to_i : printed.to_i).to_f
    ops["v"] = fraction.length.to_f
    ops["f"] = fraction.to_i.to_f
    ops["t"] = trimmed.to_i.to_f
    ops["d"] = count.is_a?(Float) && raw % 1 != 0 ? count.to_s.split(".").last.to_s.to_i.to_f : raw
    ops["num"] = numeric ? 1.0 : 0.0
    @plural_at = 0
    plural_key(rule, ops)
  end

  def self.plural_key(rule, ops)
    token = rule[plural_step].to_s
    return token[1, token.length - 1].to_s if token.start_with?(":")
    cond = plural_bool(rule, ops)
    yes = plural_key(rule, ops)
    no = plural_key(rule, ops)
    cond ? yes : no
  end

  # Both sides read even when the first decides: the rule is prefix text, and skipping a side means walking it anyway.
  def self.plural_bool(rule, ops)
    token = rule[plural_step].to_s
    if token == "&" || token == "|"
      a = plural_bool(rule, ops)
      b = plural_bool(rule, ops)
      return token == "&" ? a && b : a || b
    end
    return !plural_bool(rule, ops) if token == "!"
    return ops["num"] == 1.0 if token == "num"
    x = plural_num(rule, ops)
    return x % 1 == 0 if token == "int"
    if token == "of"
      size = rule[plural_step].to_s.to_i
      found = false
      # Not `found = true if ...`: each step reads the next operand, so every one runs, and a modifier if in a block is a statement C# cannot put there
      size.times { found = plural_num(rule, ops) == x || found }
      return found
    end
    y = plural_num(rule, ops)
    return x == y if token == "=="
    return x != y if token == "!="
    return x >= y if token == ">="
    return x < y if token == "<"
    hi = plural_num(rule, ops)
    within = x >= y && x <= hi
    token == "inz" ? within && x % 1 == 0 : within
  end

  def self.plural_num(rule, ops)
    token = rule[plural_step].to_s
    if token == "%"
      a = plural_num(rule, ops)
      b = plural_num(rule, ops)
      return a % b
    end
    return plural_num(rule, ops).floor.to_f if token == "floor"
    value = ops.fetch(token, nil)
    value.nil? ? token.to_f : value
  end

  def self.plural_step
    at = @plural_at || 0
    @plural_at = at + 1
    at
  end

  def self.runtime_chain(loc)
    chains = runtime_chains
    env = I18nHost.env_name
    chains.fetch(loc + "\u001C" + env, nil) || chains.fetch(loc + "\u001C", nil) || [loc]
  end

  # `%{name}` filled from the options that are not the gem's reserved ones, as I18n.interpolate does; none given leaves the text as it is.
  def self.interpolate_runtime(text, options, html)
    reserved = [:cascade, :deep_interpolation, :skip_interpolation, :default, :exception_handler, :fallback, :fallback_in_progress, :fallback_original_locale, :format, :object, :raise, :resolve, :scope, :separator, :throw, :locale]
    given = options.reject { |k, _| reserved.include?(k) }
    return text if given.empty?
    out = ""
    i = 0
    while i < text.length
      c = text[i, 1].to_s
      if c == "%" && text[i + 1, 1].to_s == "%"
        out = out + "%"
        i += 2
      elsif c == "%" && text[i + 1, 1].to_s == "{"
        close = text.index("}", i)
        if close.nil?
          out = out + c
          i += 1
        else
          name = text[i + 2, close - i - 2].to_s
          sym = name.to_sym
          raise MissingInterpolationArgument, "missing interpolation argument :#{name} in #{text.inspect} (#{given.inspect} given)" unless given.key?(sym)
          value = given[sym].to_s
          out = out + (html ? ActionView::ViewHelpers.html_escape(value) : value)
          i = close + 1
        end
      else
        out = out + c
        i += 1
      end
    end
    out
  end

  # Not folded per locale at the call: an app with a hundred locales made every `t` a hundred-arm case.
  def self.translate_site(id, loc, key, count, names, values, details, raises, in_view, escape_plain)
    entry = site_entry(id, loc, key)
    # Not the fields read bare: an Array's element may be nil to the typer, and a strict target holds each one as a String
    kind = entry[0].to_s
    html = entry[1].to_s == "1"
    if kind == "m"
      named = entry[2].to_s
      shown = named.empty? ? loc + "." + key : named
      return missing_translation_tag(shown, details, raises) if in_view
      considered = entry[3].to_s
      return considered.empty? ? missing_translation(shown, raises) : missing_translation_among(shown, considered, raises)
    end
    return subtree_translation(key) if kind == "s"
    template = entry[2].to_s
    if kind == "p"
      # Not `count == 0`: a count is an Integer or a Float, and a strict target compares one number type, as Ruby's 1.0 == 1 does
      zero = !count.nil? && count.to_f == 0.0
      one = !count.nil? && count.to_f == 1.0
      template = (zero && entry[2].to_s == "1") ? entry[3].to_s : (one ? entry[4].to_s : entry[5].to_s)
    end
    out = render_template(template, count, names, values, html)
    escape_plain && !html ? ActionView::ViewHelpers.html_escape(out) : out
  end

  # A locale without records of its own answers as the default locale: the table leaves out each one that would repeat it.
  def self.site_entry(id, loc, key)
    found = site_entry_in(id, loc, key)
    found = site_entry_in(id, app_default_locale.to_s, key) if found.nil?
    raise ArgumentError, "no translation table entry for " + id + " in " + loc if found.nil?
    translation_entries[found]
  end

  def self.site_entry_in(id, loc, key)
    table = translation_table
    env = I18nHost.env_name
    sep = "\u001C"
    # Not the any-environment key before this environment's "*": an environment with entries of its own has them all.
    found = table.fetch(id + sep + loc + sep + env + sep + key, nil)
    found = table.fetch(id + sep + loc + sep + env + sep + "*", nil) if found.nil? && !key.empty?
    found = table.fetch(id + sep + loc + sep + sep + key, nil) if found.nil?
    found = table.fetch(id + sep + loc + sep + sep + "*", nil) if found.nil? && !key.empty?
    found
  end

  def self.render_template(template, count, names, values, html)
    out = ""
    template.split("\u001D").each do |part|
      # Not `next if part.empty?`: Rust emits this block as a closure, where `next` has no spelling yet
      unless part.empty?
        body = part[1, part.length - 1].to_s
        if part[0, 1] == "n"
          at = names.index(body)
          value = at.nil? ? (body == "count" ? count.to_s : "%{" + body + "}") : values[at].to_s
          out = out + (html ? ActionView::ViewHelpers.html_escape(value) : value)
        else
          out = out + body
        end
      end
    end
    out
  end

  # Not each reader's own ivar tested for nil: the TypeScript emitter folds an ivar named after its reader into a field that starts filled
  def self.translation_table
    load_translation_table unless @table_loaded == true
    @site_table || {}
  end

  def self.runtime_leaves
    load_runtime_catalog unless @catalog_loaded == true
    @leaf_table || {}
  end

  def self.runtime_nodes
    load_runtime_catalog unless @catalog_loaded == true
    @node_table || {}
  end

  def self.runtime_order
    load_runtime_catalog unless @catalog_loaded == true
    @leaf_order || []
  end

  def self.runtime_empties
    load_runtime_catalog unless @catalog_loaded == true
    @empty_table || {}
  end

  def self.runtime_chains
    load_runtime_catalog unless @catalog_loaded == true
    @chain_table || {}
  end

  def self.translation_entries
    load_translation_table unless @table_loaded == true
    @entry_list || []
  end

  # A line ends in a "." field: `split` drops trailing empty fields, and a template or a key can be empty.
  def self.runtime_rules
    load_runtime_catalog unless @catalog_loaded == true
    @rule_table || {}
  end

  def self.runtime_opaque
    load_runtime_catalog unless @catalog_loaded == true
    @opaque_table || {}
  end

  # Not one loop over every kind: the core targets drop the catalog's readers, and a T line per key keeps each kind a single write
  def self.load_translation_table
    table = {}
    entries = []
    sep = "\u001C"
    I18nHost.table_text.split("\u001E").each do |line|
      fields = line.split("\u001F")
      kind = fields[0].to_s
      if kind == "E"
        entries << (fields[1, fields.length - 2] || [])
      elsif kind == "T"
        table[fields[1].to_s + sep + fields[2].to_s + sep + fields[3].to_s + sep + fields[4].to_s] = fields[5].to_s.to_i
      end
    end
    @entry_list = entries
    @site_table = table
    @table_loaded = true
    nil
  end

  def self.load_runtime_catalog
    rules = {}
    opaque = {}
    leaves = {}
    nodes = {}
    order = []
    empties = {}
    chains = {}
    sep = "\u001C"
    I18nHost.table_text.split("\u001E").each do |line|
      fields = line.split("\u001F")
      kind = fields[0].to_s
      if kind == "L" || kind == "M"
        if kind == "L"
          leaves[fields[1]] = fields[2].to_s
        else
          empties[fields[1]] = true
          nodes[fields[1]] = true
        end
        order << fields[1].to_s
        parts = fields[1].split(".")
        j = parts.length - 1
        while j > 1
          nodes[(parts[0, j] || []).join(".")] = true
          j -= 1
        end
      elsif kind == "O"
        opaque[fields[1]] = fields[2, fields.length - 3] || []
      elsif kind == "R"
        rules[fields[1]] = fields[2].to_s.split(" ")
      elsif kind == "F"
        chains[fields[1].to_s + sep + fields[2].to_s] = fields[3].to_s.split(",")
      end
    end
    @leaf_table = leaves
    @node_table = nodes
    @leaf_order = order
    @empty_table = empties
    @chain_table = chains
    @rule_table = rules
    @opaque_table = opaque
    @catalog_loaded = true
    nil
  end

  # >>> generated: i18n-locales
  def self.app_default_locale
    :en
  end

  def self.app_available_locales
    [:en]
  end
  # <<< generated: i18n-locales
end
