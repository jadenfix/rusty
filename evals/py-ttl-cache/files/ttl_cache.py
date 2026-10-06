import time


class TTLCache:
    """A least-recently-used cache whose entries also expire.

    An entry expires `ttl` seconds after it was last *set*; reading it does
    not extend its life. An entry whose age equals `ttl` exactly is already
    expired. `clock` returns the current time in seconds and is injectable
    for tests.

    When a set would make the cache hold more than `maxsize` live entries,
    expired entries are dropped first, then the least recently used live
    entry (a successful get or a set counts as a use).
    """

    def __init__(self, maxsize, ttl, clock=time.monotonic):
        raise NotImplementedError

    def get(self, key, default=None):
        """The live value for key, or default if missing or expired."""
        raise NotImplementedError

    def set(self, key, value):
        """Stores value under key, resetting its age."""
        raise NotImplementedError

    def __len__(self):
        """The number of live (unexpired) entries."""
        raise NotImplementedError
