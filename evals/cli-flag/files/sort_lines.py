import argparse
import sys


def main():
    parser = argparse.ArgumentParser(description="Sort lines from stdin.")
    parser.add_argument("--unique", action="store_true", help="drop duplicate lines")
    args = parser.parse_args()
    lines = sys.stdin.read().splitlines()
    if args.unique:
        lines = list(dict.fromkeys(lines))
    for line in sorted(lines):
        print(line)


if __name__ == "__main__":
    main()
