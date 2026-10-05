from shop import pricing
from shop.orders import order_total

assert order_total({"lines": [(2, 1.5), (1, 3.0)]}) == 6.0
assert order_total({"lines": [(1, 10.0)], "tax": 0.2}) == 12.0
assert pricing.compute_total([(1, 1.0)]) == 1.0
print("ok")
