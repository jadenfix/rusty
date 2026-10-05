python3 -c "
import ast, util
assert util.shout('abc') == 'ABC'
tree = ast.parse(open('util.py').read())
fn = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == 'shout')
doc = ast.get_docstring(fn) or ''
assert doc.startswith('RUSTY:'), repr(doc)
"
