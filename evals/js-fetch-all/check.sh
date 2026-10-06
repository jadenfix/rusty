grep -q 'assert.deepStrictEqual(out, \[60, 20, 40\])' test.js && node test.js && node "$EVAL_DIR/hidden.js"
