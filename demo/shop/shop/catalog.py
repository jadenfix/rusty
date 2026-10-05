"""Prices are in cents so money never touches floating point."""

from __future__ import annotations

PRICES = {
    "tea": 450,
    "mug": 1200,
    "beans": 1800,
    "grinder": 6400,
}


def price(sku: str) -> int:
    try:
        return PRICES[sku]
    except KeyError:
        raise ValueError(f"unknown sku: {sku}") from None
