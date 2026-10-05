def compute(prices, rate):
    tmp = 0
    for p in prices:
        tmp += p
    return round(tmp * (1 + rate), 2)


if __name__ == "__main__":
    print(compute([1.0, 2.5], 0.1))
