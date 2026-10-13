# frozen_string_literal: true

require "json"
lane, source = ARGV
original = source == "original"
if original
  require_relative "boot"
  prepare_writebook
else
  load source
  raise "Rails present in standalone execution" if defined?(Rails) || defined?(ActiveRecord)
end

observations = []
equal = ->(actual, expected) do
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  # Snapshot only the report value. Later public mutations must not rewrite
  # earlier observations; the actual application value is never frozen/copied.
  observations << JSON.parse(JSON.generate(actual))
end

case lane
when "qr"
  first = QrCodeLink.new("https://east.example/books/7?view=reader")
  second = QrCodeLink.new("https://west.example/books/29?view=editor#λ")
  equal.call(first.url, "https://east.example/books/7?view=reader")
  equal.call(second.url, "https://west.example/books/29?view=editor#λ")
  equal.call(first.url, "https://east.example/books/7?view=reader")
  equal.call(QrCodeLink.new("").url, "")
when "attributes"
  first, second = Section.new, Section.new
  equal.call(first.body, nil)
  first.body = "North λ <draft>"
  second.body = "South & final"
  equal.call(first.markable, "North λ <draft>")
  equal.call(second.searchable_content, "South & final")
  first.body = ""
  equal.call(first.searchable_content, "")
  equal.call(second.markable, "South & final")
when "current"
  first, second = Current.new, Current.new
  equal.call(first.user, nil)
  first.user = "east"
  second.user = "west λ"
  equal.call(first.user, "east")
  equal.call(second.user, "west λ")
  first.user = nil
  equal.call(first.user, nil)
  equal.call(second.user, "west λ")
when "current_lifecycle"
  first, second = Current.new, Current.new
  # The real declaration uses NOT_SET, not explicit default: nil. Its public
  # attribute snapshot starts EMPTY; do not seed or inject a replacement store.
  equal.call(first.attributes, {})
  equal.call(second.attributes, {})
  equal.call(first.user, nil)
  equal.call(first.session, nil)
  first.user = "east λ"
  second.user = "west & final"
  equal.call(first.user, "east λ")
  equal.call(second.user, "west & final")
  copy = first.attributes
  copy[:user] = "copy-only"
  equal.call(first.user, "east λ")
  equal.call(first.reset, {})
  equal.call(first.attributes, {})
  equal.call(first.user, nil)
  equal.call(first.session, nil)
  equal.call(second.user, "west & final")
  first.user = "after -13"
  equal.call(first.user, "after -13")
  equal.call(second.user, "west & final")
when "current_registry"
  Current.clear_all
  equal.call(Current.defaults.keys, %i[session user])
  equal.call(Current.user, nil)
  equal.call(Current.session, nil)
  singleton = Current.instance
  equal.call(singleton.attributes, {})
  equal.call(Current.instance.equal?(singleton), true)
  Current.user = "main east λ"
  equal.call(singleton.user, "main east λ")
  independent = Current.new
  independent.user = "independent west"
  equal.call(Current.user, "main east λ")
  equal.call(independent.user, "independent west")
  scoped = Current.set(user: "scope 73") do |receiver|
    equal.call(receiver.equal?(singleton), true)
    equal.call(Current.user, "scope 73")
    "scope result"
  end
  equal.call(scoped, "scope result")
  equal.call(Current.user, "main east λ")
  failure = RuntimeError.new("scope failure")
  begin
    Current.set(user: "exception west") do
      equal.call(Current.user, "exception west")
      raise failure
    end
  rescue RuntimeError => caught
    equal.call(caught.equal?(failure), true)
  end
  equal.call(Current.user, "main east λ")
  worker = Thread.new do
    initial_user = Current.user
    instance = Current.instance
    shared = instance.equal?(singleton)
    Current.user = "worker west -29"
    assigned = Current.user
    Current.reset
    [initial_user, shared, assigned, Current.user, Current.instance.equal?(instance)]
  end.value
  equal.call(worker, [nil, false, "worker west -29", nil, true])
  equal.call(Current.user, "main east λ")
  Current.reset
  equal.call(Current.instance.equal?(singleton), true)
  equal.call(Current.user, nil)
  equal.call(singleton.attributes, {})
  equal.call(independent.user, "independent west")
  Current.user = "before clear"
  Current.clear_all
  equal.call(singleton.user, nil)
  equal.call(Current.instance.equal?(singleton), false)
  equal.call(Current.user, nil)
  equal.call(Current.instance.attributes, {})
when "enum"
  object = Access.new(level: :reader)
  equal.call(object.level, "reader")
  equal.call(object.reader?, true)
  equal.call(object.editor?, false)
  object.level = :editor
  equal.call(object.level, "editor")
  equal.call(object.reader?, false)
  equal.call(object.editor?, true)
  object.level = "reader"
  equal.call(object.editor?, false)
  begin
    object.level = "administrator"
    raise "invalid enum accepted"
  rescue ArgumentError
    equal.call(object.level, "reader")
  end
when "delegate"
  first, second = Section.new, Section.new
  first.leaf = Leaf.new(title: "North λ")
  second.leaf = Leaf.new(title: "South & <draft>")
  equal.call(first.title, "North λ")
  equal.call(second.title, "South & <draft>")
  first.leaf.title = "Changed east"
  equal.call(first.title, "Changed east")
  equal.call(second.title, "South & <draft>")
when "markdown"
  first, second = Page.new, Page.new
  equal.call(first.markable, "")
  first.body = "# North λ\n\n<draft>"
  second.body = "South & final"
  equal.call(first.markable, "# North λ\n\n<draft>")
  equal.call(second.markable, "South & final")
  first.body = ""
  equal.call(first.markable, "")
  equal.call(second.markable, "South & final")
when "slug"
  { "North & East 73" => "north-east-73", "---" => "-", "" => "-", "Élan West 29" => "elan-west-29" }.each do |title, expected|
    equal.call(Leaf.new(title: title).slug, expected)
  end
when "position"
  book = Book.create!(title: "Position contract", slug: "position-contract")
  low = Leaf.create!(book: book, leafable: Section.create!(body: "Low"), title: "Low", position_score: -7)
  middle = Leaf.create!(book: book, leafable: Section.create!(body: "Middle"), title: "Middle", position_score: 13)
  high = Leaf.create!(book: book, leafable: Section.create!(body: "High"), title: "High", position_score: 41)
  # Positionable's around_create chooses scores. Set the asymmetric ordering
  # through actual AR state, not a substituted relation/attribute implementation.
  low.update_columns(position_score: -7)
  middle.update_columns(position_score: 13)
  high.update_columns(position_score: 41)
  equal.call(low.previous, nil)
  equal.call(middle.previous.title, "Low")
  equal.call(high.previous.title, "Middle")
when "embed"
  provider = EmbedProvider.new(name: "video", hosts: ["Video.Example"], path_prefix: "/embed")
  { "https://video.example/embed/73" => true, "https://else.example/embed/73" => false,
    "https://video.example/embedded/73" => false, "https://video.example/embed/../watch" => false,
    "https://video.example/embed/%2Fwatch" => false, "https://VIDEO.EXAMPLE/embed" => true }.each do |uri, expected|
    equal.call(provider.allows?(URI.parse(uri)), expected)
  end
when "embed_accessors"
  hosts = ["VIDEO.EXAMPLE", "archive.example"]
  first = EmbedProvider.new(name: "North λ", hosts: hosts, path_prefix: "/embed",
    attributes: %w[src srcdoc title onload width])
  second = EmbedProvider.new(name: "South & final", hosts: "West.Example", path_prefix: "/video")
  equal.call(first.name, "North λ")
  equal.call(second.name, "South & final")
  equal.call(first.hosts, %w[video.example archive.example])
  equal.call(second.hosts, ["west.example"])
  equal.call(first.path_prefix, "/embed")
  equal.call(second.path_prefix, "/video")
  equal.call(first.attributes, %w[src title width])
  equal.call(second.attributes, %w[src width height allowfullscreen frameborder title loading])
  hosts[0] = "changed.example"
  equal.call(first.hosts, %w[video.example archive.example])
  equal.call(first.csp_sources, ["https://video.example", "https://archive.example"])
  equal.call(second.csp_sources, ["https://west.example"])
  # This public API really returns a mutable array; do not freeze a captured
  # sample and call it equivalent, or share it with another constructed object.
  first.attributes << "local-extra"
  equal.call(first.attributes, %w[src title width local-extra])
  equal.call(second.attributes, %w[src width height allowfullscreen frameborder title loading])
when "arrangement"
  receiver = Object.new.extend(ArrangementHelper)
  expected = "click->arrangement#click dragstart->arrangement#dragStart " \
    "dragover->arrangement#dragOver:prevent dragend->arrangement#dragEnd drop->arrangement#drop " \
    "keydown.up->arrangement#moveBefore keydown.right->arrangement#moveAfter " \
    "keydown.down->arrangement#moveAfter keydown.left->arrangement#moveBefore " \
    "keydown.shift+up->arrangement#moveBefore keydown.shift+right->arrangement#moveAfter " \
    "keydown.shift+down->arrangement#moveAfter keydown.shift+left->arrangement#moveBefore " \
    "keydown.space->arrangement#toggleMoveMode keydown.enter->arrangement#applyMoveMode " \
    "keydown.esc->arrangement#cancelMoveMode"
  value = receiver.arrangement_actions
  equal.call(value, expected)
  value.replace("caller changed this result")
  equal.call(receiver.arrangement_actions, expected)
when "lightbox"
  # Actual sanitizer + actual app constant-dependent method, not a copied body.
  # Lexical constant bindings are not mutated after boot.
  html = '<a id="allowed" data-action="lightbox#open:prevent">North</a>' \
    '<a id="foreign" data-action="other#open:prevent">South</a>' \
    '<details id="wrong_element" data-action="lightbox#open:prevent">West</details>'
  fragment = Loofah.fragment(html).scrub!(HtmlScrubber.new)
  equal.call(fragment.at_css("#allowed")["data-action"], "lightbox#open:prevent")
  equal.call(fragment.at_css("#foreign")["data-action"], nil)
  equal.call(fragment.at_css("#wrong_element")["data-action"], nil)
else
  raise "unknown contract #{lane}"
end

puts JSON.generate(checks: observations.length, observations: observations)
