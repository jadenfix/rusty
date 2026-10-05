from __future__ import annotations

from shop.catalog import price
from shop.discounts import apply_discount
from shop.tax import with_tax


class Cart:
    def __init__(self):
        self.lines: dict[str, int] = {}

    def add(self, sku: str, qty: int = 1) -> None:
        price(sku)  # validates the sku
        self.lines[sku] = self.lines.get(sku, 0) + qty

    def subtotal(self) -> int:
        return sum(price(sku) * qty for sku, qty in self.lines.items())

    def total(self, code: str | None = None) -> int:
        return with_tax(apply_discount(self.subtotal(), code))
