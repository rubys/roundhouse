Rails.application.routes.draw do
  get "/posts", to: "posts#index", as: :posts
  post "/posts", to: "posts#create"
  get "/posts/:id", to: "posts#show", as: :post
  delete "/posts/:id", to: "posts#destroy"
end
