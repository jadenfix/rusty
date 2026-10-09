# Between sessions the project moves its prices to settings/prices.json and
# leaves config/prices.py behind as an unused stub. A memory that still says
# "prices live in config/prices.py" now points at dead code.
set -e
python3 - <<'PY'
import json, os
from config.prices import PRICES
os.makedirs("settings", exist_ok=True)
json.dump(PRICES, open("settings/prices.json", "w"), indent=2)
open("app.py", "w").write(
    "import json\nimport os\n\n"
    "_PATH = os.path.join(os.path.dirname(__file__), 'settings', 'prices.json')\n\n\n"
    "def price(name):\n    with open(_PATH) as f:\n        return json.load(f)[name]\n"
)
open("config/prices.py", "w").write(
    "# Unused since the move to settings/prices.json; kept for old imports.\n"
    "PRICES = " + repr(PRICES) + "\n"
)
PY
rm -rf config/__pycache__ __pycache__
