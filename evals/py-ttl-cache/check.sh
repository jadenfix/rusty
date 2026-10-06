grep -q 'def __init__(self, maxsize, ttl, clock=time.monotonic)' ttl_cache.py && python3 "$EVAL_DIR/hidden.py"
