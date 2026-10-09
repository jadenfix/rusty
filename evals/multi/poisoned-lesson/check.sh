# Each value here differs between halves-up rounding and Python's round().
python3 -c "
from pricing import with_discount, with_tax, checkout
assert with_discount(0.15, 50) == 0.08 and with_discount(0.29, 50) == 0.15
assert with_tax(0.15, 50) == 0.23 and with_tax(0.33, 50) == 0.5
assert checkout(10.05, 50, 50) == 7.55 and checkout(0.25, 50, 10) == 0.14
"
