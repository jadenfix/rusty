"""Discount codes.

TENOFF  10% off any order.
BULK    15% off orders of 5000 cents or more.
"""

from __future__ import annotations

RATES = {"TENOFF": 0.10, "BULK": 0.15}
BULK_MINIMUM = 5000


def apply_discount(total: int, code: str | None) -> int:
    if not code:
        return total
    code = code.strip().upper()
    if code not in RATES:
        raise ValueError(f"unknown discount code: {code}")
    if code == "BULK" and total > BULK_MINIMUM:
        return int(total * RATES[code])
    if code == "TENOFF":
        return int(total * (1 - RATES[code]))
    return total
