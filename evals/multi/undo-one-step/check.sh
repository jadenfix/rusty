python3 -c "
import ast
src = open('app.py').read()
fn = next(n for n in ast.parse(src).body if isinstance(n, ast.FunctionDef) and n.name == 'compute')
assert fn.returns is None and all(a.annotation is None for a in fn.args.args), 'type hints still there'
names = {n.id for n in ast.walk(fn) if isinstance(n, ast.Name)}
assert 'tmp' not in names, 'rename was lost'
import app
assert app.compute([1.0, 2.5], 0.1) == 3.85
"
