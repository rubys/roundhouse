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
  observations << actual
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
