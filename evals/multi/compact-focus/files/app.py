import config

def connect():
    if config.DB_TIMEOUT > 30:
        raise TimeoutError("db timeout too long for the pool")

connect()
