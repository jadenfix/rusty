from pages import paginate, page_count

items = list(range(10))
assert paginate(items, 1, 3) == [0, 1, 2]
assert paginate(items, 4, 3) == [9]
assert paginate(items, 5, 3) == []
assert page_count(items, 3) == 4
assert page_count([], 3) == 0
assert page_count(items, 5) == 2
print("ok")
