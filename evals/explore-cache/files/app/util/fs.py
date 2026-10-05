import os
import tempfile


def atomic_write(path, text):
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or ".")
    with os.fdopen(fd, "w") as f:
        f.write(text)
    os.replace(tmp, path)


def write_cache(path, text):
    """Deprecated alias, unused."""
    raise NotImplementedError
