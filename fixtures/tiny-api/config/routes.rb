Rails.application.routes.draw do
  resources :widgets, only: %i[index show create update]
  get "/feed.json", to: "formats#literal"
  get "/formats", to: "formats#index"
  get "/formats/:id", to: "formats#show"
end
