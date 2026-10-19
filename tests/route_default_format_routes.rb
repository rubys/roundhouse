get "manifest" => "rails/pwa#manifest", as: :pwa_manifest, defaults: { format: :json }
get "plain" => "pages#index", as: :plain
get "string_manifest" => "pages#index", as: :string_manifest, defaults: { format: "json" }
scope defaults: { format: :json } do
  get "scoped/:id" => "pages#show", as: :scoped
end
