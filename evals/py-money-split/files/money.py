def split_bill(total_cents, weights):
    """Split `total_cents` (an int) into len(weights) integer shares.

    Each share starts as the floor of total_cents * weight / sum(weights).
    The cents left over after flooring are handed out one at a time to the
    shares with the largest fractional remainders; on a tie, the earlier
    share wins. The shares always add up to exactly total_cents.

    Raises ValueError if weights is empty, any weight is negative, or all
    weights are zero. total_cents may be zero but not negative (ValueError).
    """
    total_weight = sum(weights)
    return [round(total_cents * w / total_weight) for w in weights]
