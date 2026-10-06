grep -q 'assert split_bill(100, \[1, 1, 1\]) == \[34, 33, 33\]' test_money.py &&
python3 test_money.py && python3 "$EVAL_DIR/hidden.py"
