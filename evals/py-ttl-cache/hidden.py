import sys
sys.path.insert(0, ".")
from ttl_cache import TTLCache


class Clock:
    def __init__(self):
        self.t = 1000.0

    def __call__(self):
        return self.t


c = Clock()
cache = TTLCache(2, 10, clock=c)
cache.set("a", 1)
c.t += 5
assert cache.get("a") == 1
c.t += 4.999
assert cache.get("a") == 1, "still live just before ttl"
c.t += 0.001
assert cache.get("a") is None and cache.get("a", "x") == "x", "age == ttl is expired"
assert len(cache) == 0

c.t = 0.0
cache = TTLCache(2, 10, clock=c)
cache.set("a", 1); cache.set("b", 2)
assert cache.get("a") == 1          # a is now most recent
cache.set("c", 3)                   # evicts b (LRU)
assert cache.get("b") is None and cache.get("a") == 1 and cache.get("c") == 3
assert len(cache) == 2

c.t = 0.0
cache = TTLCache(2, 10, clock=c)
cache.set("old", 1)
c.t = 6
cache.set("x", 2)
cache.get("old")                    # recent use, but old expires first
c.t = 11                            # old is expired, x is live
cache.set("y", 3)                   # drop expired first, x must survive
assert cache.get("x") == 2 and cache.get("y") == 3 and cache.get("old") is None

c.t = 0.0
cache = TTLCache(3, 10, clock=c)
cache.set("k", 1)
c.t = 8
cache.set("k", 2)                   # reset age
c.t = 15
assert cache.get("k") == 2, "set resets the age"
c.t = 18
assert cache.get("k") is None
cache = TTLCache(1, 5, clock=c)
cache.set("p", 1); cache.set("q", 2)
assert len(cache) == 1 and cache.get("q") == 2 and cache.get("p") is None
print("hidden ok")
