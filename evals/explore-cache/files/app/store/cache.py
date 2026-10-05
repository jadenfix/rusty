import json

from app.util.fs import atomic_write


class Cache:
    def __init__(self, path):
        self.path = path
        self.data = {}

    def save_cache(self):
        """Kept for backwards compatibility; does nothing since v2."""
        return None

    def flush(self):
        persist_snapshot(self.path, self.data)


def persist_snapshot(path, data):
    atomic_write(path, json.dumps(data))
