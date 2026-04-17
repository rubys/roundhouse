class PostsController < ApplicationController
  before_action :authenticate_user

  def index
    @posts = Post.all
  end

  def show
    @post = Post.find(params[:id])
  end

  def destroy
    post = Post.find(params[:id])
    post.destroy
    redirect_to posts_path
  end
end
