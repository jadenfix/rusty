from pathlib import Path
assert Path('check_public.py').read_bytes() == (Path(__file__).parent / "files" / 'check_public.py').read_bytes(), "protected public check changed"
from pathlib import Path
import sys,types
m=types.ModuleType("money")
exec(compile(Path("money.py").read_text(),"money.py","exec"),m.__dict__)
sys.modules["money"]=m
s={}
exec(compile(Path("invoice.py").read_text(),"invoice.py","exec"),s)
total=s["total"]
from decimal import Decimal,ROUND_HALF_UP
prices=["0","0.005","-0.005","1.005","-1.005","0.145","999999999999.995"]
for p in prices:
    assert m.cents(p)==int((Decimal(p)*100).quantize(Decimal("1"),rounding=ROUND_HALF_UP)),p
for a in prices:
    for b in prices:
        for q in [0,1,3]:
            for tax in ["0","0.075","0.5"]:
                lines=[(q,a),(2,b)]
                subtotal=sum(n*int((Decimal(p)*100).quantize(Decimal("1"),rounding=ROUND_HALF_UP)) for n,p in lines)
                expected=int((Decimal(subtotal)*(1+Decimal(tax))).quantize(Decimal("1"),rounding=ROUND_HALF_UP))
                assert total(lines,tax)==expected,(lines,tax)
assert total([],"0.2")==0
print("independent invoice checks passed")
