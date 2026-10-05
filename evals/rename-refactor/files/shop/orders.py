from shop.pricing import calc_total


def order_total(order):
    return calc_total(order["lines"], order.get("tax", 0.0))
