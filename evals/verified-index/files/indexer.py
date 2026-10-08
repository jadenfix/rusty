from normalize import key

def index(lines):
    return [{"key": key(value), "first": value, "count": 1} for value in lines]
