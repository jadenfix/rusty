import sys
sys.path.insert(0, ".")
from fractions import Fraction
from money import split_bill


def reference(total, weights):
    s = sum(weights)
    exact = [Fraction(total * w, s) for w in weights]
    shares = [int(e) for e in exact]
    left = total - sum(shares)
    order = sorted(range(len(weights)), key=lambda i: (-(exact[i] - shares[i]), i))
    for i in order[:left]:
        shares[i] += 1
    return shares


cases = [(100, [1, 1, 1]), (0, [2, 3]), (1, [1, 1]), (7, [0, 1, 0, 1]), (1001, [3, 3, 4]),
         (999999, [7, 11, 13, 17]), (5, [1] * 9), (12345, [10**12, 1, 1]), (2, [1, 2, 1]), (10, [5])]
x = 7
for _ in range(300):
    x = (x * 1103515245 + 12345) % 2**31
    n = 1 + x % 7
    ws = [(x >> (3 * i)) % 6 for i in range(n)]
    if sum(ws) == 0:
        ws[0] = 1
    cases.append((x % 100000, ws))
for total, ws in cases:
    got = split_bill(total, list(ws))
    assert got == reference(total, ws), (total, ws, got, reference(total, ws))
    assert all(isinstance(v, int) for v in got), got
for bad in [(10, []), (10, [1, -1]), (10, [0, 0]), (-1, [1])]:
    try:
        split_bill(*bad)
    except ValueError:
        continue
    raise AssertionError(f"no ValueError for {bad}")
print("hidden ok")
