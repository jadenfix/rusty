import json, subprocess, sys
from inventory import Inventory
inv = Inventory()
inv.add("bolt", 5); inv.add("bolt", 2)
assert inv.count("bolt") == 7 and inv.count("nut") == 0
inv.remove("bolt", 10)                      # the pivot: clamp, don't raise
assert inv.count("bolt") == 0
inv.add("nut", 3)
again = Inventory.from_json(inv.to_json())
assert again.count("nut") == 3 and again.count("bolt") == 0
json.loads(inv.to_json())
assert subprocess.run([sys.executable, "test_inventory.py"]).returncode == 0
print("ok")
