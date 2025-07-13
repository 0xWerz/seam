class OrdersController < ApplicationController
  before_action :require_login

  def show
    render json: Order.find(params[:id])
  end

  private

  def require_login
    head :unauthorized unless session[:user_id]
  end
end
