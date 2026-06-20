from __future__ import annotations

MODULE_TIMEOUT = 30


class OrderService:
    prefix = "order"

    def display_name(self, order_id: str) -> str:
        return f"{self.prefix}:{order_id}"

    def list_orders(self) -> list[str]:
        return ["intake", "packing"]

    def summary(self, order_id: str) -> str:
        return format_order(self.display_name(order_id))


def format_order(value: str) -> str:
    return value.upper()
