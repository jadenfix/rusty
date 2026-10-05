.PHONY: check
check: fixtures/data.json
	python3 -m unittest discover -s tests -q

fixtures/data.json:
	@echo "fixtures missing: run 'python3 scripts/gen_fixtures.py' first" && exit 1
