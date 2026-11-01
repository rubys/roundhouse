Rails.application.routes.draw do
  get "/pages", to: "pages#index"
  get "/named_pages", to: "named_pages#index"
  get "/inherited_pages", to: "inherited_pages#index"
  get "/rescue_pages", to: "rescue_pages#index"
  get "/other_pages", to: "other_pages#index"
  get "/encoded", to: "encoded#index"
end
