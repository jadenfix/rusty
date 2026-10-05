def paginate(items, page, size):
    """Return the items on `page` (1-based) when split into pages of `size`."""
    if page < 1 or size < 1:
        raise ValueError("page and size must be positive")
    start = page * size
    return items[start:start + size]


def page_count(items, size):
    return len(items) // size
