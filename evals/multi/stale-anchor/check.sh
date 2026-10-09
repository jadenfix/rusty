# The live prices changed, not the stub left behind.
python3 -c "
import json
from app import price
assert price('widget') == 12, price('widget')
assert price('gadget') == 7, price('gadget')
assert json.load(open('settings/prices.json'))['widget'] == 12
"
