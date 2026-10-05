RATE = 0.0825


def with_tax(cents: int) -> int:
    return round(cents * (1 + RATE))
