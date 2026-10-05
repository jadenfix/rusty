[ "$(printf 'b\na\nc\na\n' | python3 sort_lines.py --reverse --unique | tr '\n' ' ')" = "c b a " ] &&
[ "$(printf 'b\na\nc\n' | python3 sort_lines.py | tr '\n' ' ')" = "a b c " ]
