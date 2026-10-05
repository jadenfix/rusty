import json, os
os.makedirs("fixtures", exist_ok=True)
json.dump({"values": [3, 1, 2]}, open("fixtures/data.json", "w"))
print("fixtures written")
