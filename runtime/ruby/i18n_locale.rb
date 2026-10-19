# frozen_string_literal: true
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

  # The locale set for this thread, by name ("" when none is): a Symbol
  # read back out of `Thread.current` is a value the strict targets
  # cannot narrow, and a String they can. (Not `locale_set`: Spinel
  # spells `locale=` with that name.)
  def self.thread_locale_name
    Thread.current[:roundhouse_i18n_locale].to_s
  end

  def self.locale
    set = thread_locale_name
    set.empty? ? app_default_locale : set.to_sym
  end

  # `I18n.locale = x`: an available locale, a Symbol or its String.
  def self.locale=(value)
    Thread.current[:roundhouse_i18n_locale] = checked_locale(value).to_s
  end

  # `value` as the locale it names, or the gem's InvalidLocale, whose
  # message shows the value as `inspect` does (nil is not a locale).
  def self.checked_locale(value)
    name = value.to_s.to_sym
    return name if !value.nil? && app_available_locales.include?(name)
    shown = value.nil? ? "nil" : (value.is_a?(Symbol) ? ":" + value.to_s : "\"" + value.to_s + "\"")
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
    Thread.current[:roundhouse_i18n_locale] = checked_locale(value).to_s
    previous
  end

  def self.leave_locale(previous)
    Thread.current[:roundhouse_i18n_locale] = previous unless previous.nil?
    nil
  end

  # Whether the running environment is one of `envs` (comma-separated),
  # where `config.i18n.fallbacks` differs by environment.
  def self.running_in?(envs)
    envs.split(",").include?(Rails.env.to_s)
  end

  class MissingTranslationData < ArgumentError
  end

  # Whether this environment raises on a missing translation: `raises`
  # is `config/application.rb`'s setting, and the comma-separated
  # `flipped` environments set the other value in their own file.
  def self.missing_raises(raises, flipped)
    env = Rails.env.to_s
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
    text = ActionView::ViewHelpers.html_escape(ActiveSupport.titleize(last))
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

  # An empty mapping in the files (`feed: {}`), which I18n answers as `{}`.
  def self.insert_empty(tree, path)
    node = tree
    i = 0
    while i < path.length
      key = path[i].to_s.to_sym
      child = node[key]
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
      child = node[key]
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
      text = leaves[l + "." + full]
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
    zero = leaves[node + ".zero"]
    return zero if count == 0 && !zero.nil?
    rule = plural_rule(loc)
    if rule.nil?
      form = count == 1 ? "one" : "other"
      text = leaves[node + "." + form]
      raise InvalidPluralizationData, "translation data #{node} can not be used with :count => #{count}. key '#{form}' is missing." if text.nil?
      return text
    end
    if count == 0 || count == 1
      exact = leaves[node + "." + (count == 0 ? "0" : "1")]
      return exact unless exact.nil?
    end
    form = plural_category(rule, count)
    text = leaves[node + "." + form] || leaves[node + ".other"]
    raise InvalidPluralizationData, "translation data #{node} can not be used with :count => #{count}. key '#{form}' is missing." if text.nil?
    text
  end

  def self.plural_rule(loc)
    rules = runtime_rules
    runtime_chain(loc).each do |l|
      rule = rules[l]
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
      size.times { found = true if plural_num(rule, ops) == x }
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
    value = ops[token]
    value.nil? ? token.to_f : value
  end

  def self.plural_step
    at = @plural_at || 0
    @plural_at = at + 1
    at
  end

  def self.runtime_chain(loc)
    chains = runtime_chains
    env = Rails.env.to_s
    chains[loc + "\u001C" + env] || chains[loc + "\u001C"] || [loc]
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
    kind = entry[0]
    html = entry[1] == "1"
    if kind == "m"
      shown = entry[2].empty? ? loc + "." + key : entry[2]
      return missing_translation_tag(shown, details, raises) if in_view
      return entry[3].empty? ? missing_translation(shown, raises) : missing_translation_among(shown, entry[3], raises)
    end
    return subtree_translation(key) if kind == "s"
    template = entry[2]
    if kind == "p"
      template = (count == 0 && entry[2] == "1") ? entry[3] : (count == 1 ? entry[4] : entry[5])
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
    env = Rails.env.to_s
    sep = "\u001C"
    # Not the any-environment key before this environment's "*": an environment with entries of its own has them all.
    found = table[id + sep + loc + sep + env + sep + key]
    found = table[id + sep + loc + sep + env + sep + "*"] if found.nil? && !key.empty?
    found = table[id + sep + loc + sep + sep + key] if found.nil?
    found = table[id + sep + loc + sep + sep + "*"] if found.nil? && !key.empty?
    found
  end

  def self.render_template(template, count, names, values, html)
    out = ""
    template.split("\u001D").each do |part|
      next if part.empty?
      body = part[1, part.length - 1].to_s
      if part[0, 1] == "n"
        at = names.index(body)
        value = at.nil? ? (body == "count" ? count.to_s : "%{" + body + "}") : values[at].to_s
        out = out + (html ? ActionView::ViewHelpers.html_escape(value) : value)
      else
        out = out + body
      end
    end
    out
  end

  def self.translation_table
    load_translation_table if @translation_table.nil?
    @translation_table || {}
  end

  def self.runtime_leaves
    load_translation_table if @runtime_leaves.nil?
    @runtime_leaves || {}
  end

  def self.runtime_nodes
    load_translation_table if @runtime_nodes.nil?
    @runtime_nodes || {}
  end

  def self.runtime_order
    load_translation_table if @runtime_order.nil?
    @runtime_order || []
  end

  def self.runtime_empties
    load_translation_table if @runtime_empties.nil?
    @runtime_empties || {}
  end

  def self.runtime_chains
    load_translation_table if @runtime_chains.nil?
    @runtime_chains || {}
  end

  def self.translation_entries
    load_translation_table if @translation_entries.nil?
    @translation_entries || []
  end

  # A line ends in a "." field: `split` drops trailing empty fields, and a template or a key can be empty.
  def self.runtime_rules
    load_translation_table if @runtime_rules.nil?
    @runtime_rules || {}
  end

  def self.runtime_opaque
    load_translation_table if @runtime_opaque.nil?
    @runtime_opaque || {}
  end

  def self.load_translation_table
    table = {}
    rules = {}
    opaque = {}
    entries = []
    keys = {}
    leaves = {}
    nodes = {}
    order = []
    empties = {}
    chains = {}
    sep = "\u001C"
    File.read(File.join(Rails.root.to_s, "config", "i18n_translations")).split("\u001E").each do |line|
      fields = line.split("\u001F")
      kind = fields[0]
      if kind == "E"
        entries << fields[1, fields.length - 2]
      elsif kind == "K"
        keys[fields[1]] = fields[2, fields.length - 3]
      elsif kind == "L" || kind == "M"
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
          nodes[parts[0, j].join(".")] = true
          j -= 1
        end
      elsif kind == "O"
        opaque[fields[1]] = true
      elsif kind == "R"
        rules[fields[1]] = fields[2].to_s.split(" ")
      elsif kind == "F"
        chains[fields[1] + sep + fields[2]] = fields[3].to_s.split(",")
      elsif kind == "B"
        names = keys[fields[1]] || []
        prefix = fields[1] + sep + fields[2] + sep + fields[3] + sep
        i = 0
        while i < names.length
          at = fields[4 + i]
          table[prefix + names[i]] = at.to_i unless at == "-"
          i += 1
        end
      end
    end
    @runtime_leaves = leaves
    @runtime_nodes = nodes
    @runtime_order = order
    @runtime_empties = empties
    @runtime_chains = chains
    @runtime_rules = rules
    @runtime_opaque = opaque
    @translation_entries = entries
    @translation_table = table
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
