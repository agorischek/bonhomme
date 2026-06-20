defmodule Bonhomme.Example.OrderService do
  @prefix "order"

  def display_name(order_id) do
    "#{@prefix}:#{order_id}"
  end

  def summary(order_id) do
    order_id
    |> display_name()
    |> format_order()
  end

  def format_order(value) do
    String.upcase(value)
  end

  def status(:open), do: :active
  def status(:closed), do: :done
end
