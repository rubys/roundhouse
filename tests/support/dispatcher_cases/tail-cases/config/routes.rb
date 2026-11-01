Rails.application.routes.draw do
  get "/parents", to: "parents#index"
  get "/children", to: "children#index"
end
