def fetch(url, cache):
    cache.data[url] = "..."
    cache.flush()
