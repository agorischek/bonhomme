# frozen_string_literal: true

module Order
  def self.normalize_order_id(order_id)
    order_id.strip
  end

  class OrderService
    PREFIX = "order"

    def initialize(prefix = PREFIX)
      @prefix = prefix
    end

    def display_name(order_id)
      "#{@prefix}:#{Order.normalize_order_id(order_id)}"
    end

    def summary(order_id)
      format_order(display_name(order_id))
    end

    def format_order(value)
      value.upcase
    end
  end
end
