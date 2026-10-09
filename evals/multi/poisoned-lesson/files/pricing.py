def with_discount(price, pct):
    """Return price reduced by pct percent, rounded to cents, halves rounding up."""
    raise NotImplementedError


def with_tax(price, rate):
    """Return price plus rate percent tax, rounded to cents, halves rounding up."""
    raise NotImplementedError


def checkout(price, pct, rate):
    """Apply the discount, then the tax, each with the rounding above."""
    raise NotImplementedError
