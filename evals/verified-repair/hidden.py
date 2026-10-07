from pathlib import Path

scope = {}
exec(Path("calc.py").read_text(), scope)
add = scope["add"]
for a in [0, 1, -1, 17, -23, 2**80, -(2**81)]:
    for b in [0, 1, -1, 43, -101, 2**79]:
        assert add(a, b) == a + b, (a, b)
        assert add(a=a, b=b) == a + b, "public arguments changed"
