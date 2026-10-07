from pathlib import Path
import sys,types
m=types.ModuleType("money")
exec(compile(Path("money.py").read_text(),"money.py","exec"),m.__dict__)
sys.modules["money"]=m
s={}
exec(compile(Path("invoice.py").read_text(),"invoice.py","exec"),s)
total=s["total"]
assert m.cents("1.005") == 101
assert m.cents("-1.005") == -101
assert total([(2,"1.005")],"0.075") == 217
print("public invoice check passed")
