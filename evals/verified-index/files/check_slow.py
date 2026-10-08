from pathlib import Path
import sys,types
n=types.ModuleType("normalize")
exec(compile(Path("normalize.py").read_text(),"normalize.py","exec"),n.__dict__)
sys.modules["normalize"]=n
s={}
exec(compile(Path("indexer.py").read_text(),"indexer.py","exec"),s)
index=s["index"]
import time
time.sleep(1)
assert index([" A ","a"," ","B"]) == [{"key":"a","first":" A ","count":2},{"key":"b","first":"B","count":1}]
print("public index check passed")
