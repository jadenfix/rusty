#!/usr/bin/env python3
"""Records a real rusty session as an asciinema v2 cast.

A scene is a list of steps run against rusty in a real pseudo-terminal:

    ("wait", r"regex")      wait until the screen output matches
    ("type", "text")        type like a person, then press Enter
    ("keys", "\\x03")        send raw keys
    ("pause", 1.5)          let the viewer read

    python3 scripts/record.py <scene> <out.cast> [-- rusty args]

Render with agg: agg --theme monokai out.cast out.gif
"""

import codecs
import fcntl
import json
import os
import random
import re
import select
import struct
import sys
import termios
import time

COLS, ROWS = 104, 34
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")

SCENES = {
    "fullstack": [
        ("wait", r"\?2004h"),
        ("type", "Fix the local task board API and UI. Check whitespace validation, item rendering and persistence; run the tests. Keep the answer short."),
        ("wait", r"FULLSTACK_COMPLETE"),
        ("wait", r"\?2004h"),
        ("pause", 2.0),
    ],
    "hero": [
        ("wait", r"\?2004h"),
        ("pause", 2.0),
        ("type", "the discount tests are failing. find out why, fix it, and prove it."),
        ("wait", r"✓ [^\n]*ctx"),
        ("pause", 4.0),
    ],
    "swarm": [
        ("wait", r"\?2004h"),
        ("pause", 0.8),
        ("type", "/agents auto"),
        ("wait", r"\?2004h"),
        ("type", "use a swarm with one worker per module in shop/ to look for bugs, then give me a ranked list"),
        ("wait", r"✓ [^\n]*ctx"),
        ("pause", 4.0),
    ],
    "goal": [
        ("wait", r"\?2004h"),
        ("pause", 0.8),
        ("type", "/goal add a `python3 -m shop.cli total <sku> <qty>` command that prints the price, with a test"),
        ("wait", r"goal (done|blocked|paused)"),
        ("pause", 4.0),
    ],
    "compact": [
        ("wait", r"\?2004h"),
        ("pause", 0.8),
        ("type", "read src/context.rs and src/agent.rs, then explain in five bullets how compaction works"),
        ("wait", r"✓ [^\n]*ctx"),
        ("wait", r"\?2004h"),
        ("type", "/context"),
        ("wait", r"\?2004h"),
        ("type", "/compact keep how the working set is built"),
        ("wait", r"compacted"),
        ("wait", r"\?2004h"),
        ("type", "without reading anything again: which function builds the working set, and what four things does it track?"),
        ("wait", r"✓ [^\n]*ctx"),
        ("pause", 4.0),
    ],
    "focus": [
        ("wait", r"\?2004h"),
        ("pause", 0.8),
        ("type", "/view adhd"),
        ("wait", r"\?2004h"),
        ("type", "read every module in shop/ and tell me how a cart total is computed"),
        ("wait", r"✓ \d+s"),
        ("wait", r"\?2004h"),
        ("type", "/remember gotcha: money is in cents; never use floats for prices"),
        ("wait", r"\?2004h"),
        ("type", "/memory"),
        ("wait", r"\?2004h"),
        ("type", "/permissions check rm -rf build && git push --force"),
        ("wait", r"\?2004h"),
        ("type", "/tokens"),
        ("wait", r"\?2004h"),
        ("pause", 3.5),
        ("type", "/view default"),
        ("wait", r"\?2004h"),
        ("pause", 1.0),
    ],
}


def main() -> int:
    scene, out = sys.argv[1], sys.argv[2]
    extra = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    steps = SCENES[scene]
    pid, fd = os.forkpty()
    if pid == 0:
        os.environ.update({"COLORTERM": "truecolor", "TERM": "xterm-256color", "COLUMNS": str(COLS)})
        os.execvp(os.environ.get("RUSTY_BIN", "rusty"), ["rusty", *extra])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    start = time.time()
    events, screen = [], ""
    # Multi-byte characters can straddle reads; decode incrementally.
    decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def pump(timeout: float) -> None:
        nonlocal screen
        end = time.time() + timeout
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], max(0.0, end - time.time()))
            if not r:
                return
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            text = decoder.decode(data)
            if not text:
                continue
            events.append([round(time.time() - start, 4), "o", text])
            screen += text

    since = 0  # waits match output produced after the last Enter
    failed = False
    for kind, arg in steps:
        if kind == "wait":
            # Match output produced since the last Enter or the last match.
            deadline = time.time() + float(os.environ.get("RUSTY_RECORD_TIMEOUT", "300"))
            while True:
                raw = re.search(arg, screen[since:])
                if raw or re.search(arg, ANSI.sub("", screen[since:])):
                    break
                if time.time() > deadline:
                    print(f"timed out waiting for {arg!r}", file=sys.stderr)
                    failed = True
                    break
                pump(0.2)
            if raw:
                since += raw.end()
            pump(0.3)
            if failed:
                break
        elif kind == "type":
            for ch in arg:
                os.write(fd, ch.encode())
                delay = os.environ.get("RUSTY_RECORD_TYPE_DELAY")
                pump(float(delay) if delay else random.uniform(0.025, 0.07))
            pump(0.35)
            since = len(screen)
            os.write(fd, b"\r")
            pump(0.2)
        elif kind == "keys":
            os.write(fd, arg.encode())
            pump(0.2)
        elif kind == "pause":
            pump(arg)
    os.write(fd, b"\x04")
    pump(1.5)
    try:
        os.kill(pid, 9)
    except OSError:
        pass
    os.waitpid(pid, 0)
    header = {"version": 2, "width": COLS, "height": ROWS, "timestamp": int(start), "env": {"TERM": "xterm-256color"}}
    with open(out, "w") as f:
        f.write(json.dumps(header) + "\n")
        for e in events:
            f.write(json.dumps(e) + "\n")
    print(f"{out}: {len(events)} events, {events[-1][0]:.0f}s" if events else f"{out}: empty", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
