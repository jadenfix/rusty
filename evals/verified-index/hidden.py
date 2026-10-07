from pathlib import Path
assert Path('check_slow.py').read_bytes() == (Path(__file__).parent / "files" / 'check_slow.py').read_bytes(), "protected public check changed"
from pathlib import Path
import sys,types
n=types.ModuleType("normalize")
exec(compile(Path("normalize.py").read_text(),"normalize.py","exec"),n.__dict__)
sys.modules["normalize"]=n
s={}
exec(compile(Path("indexer.py").read_text(),"indexer.py","exec"),s)
index=s["index"]
from itertools import product
def reference(lines):
    groups={}
    for value in lines:
        k=value.strip().casefold()
        if not k: continue
        if k not in groups: groups[k]={"key":k,"first":value,"count":0}
        groups[k]["count"] += 1
    return [groups[k] for k in sorted(groups)]
assert index([])==[]
for length in range(1,5):
    for lines in product([""," "," A ","a","Straße","STRASSE"," B"],repeat=length):
        assert index(list(lines))==reference(lines),lines
assert index(["a"]*1000)[0]["count"] == 1000
print("independent index checks passed")
