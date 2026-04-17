Rails.application.routes.draw do
  get "/posts", to: "posts#index", as: :posts
  get "/posts/:id", to: "posts#show", as: :post
end
