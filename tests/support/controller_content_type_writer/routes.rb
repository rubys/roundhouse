Rails.application.routes.draw do
  get '/media', to: 'media#show'
  get '/other_media', to: 'other_media#show'
  get '/custom_media', to: 'custom_media#show'
end
