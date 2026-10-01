class TypedResource
  include Alba::Resource
  helper Typelizer::DSL
  attributes :id, :nickname
  typelize id: "number", nickname: ["string", optional: true, nullable: true]
end
