#!/usr/bin/env python3
"""zig ar wrapper: pure pass-through. Paths must NOT be rewritten."""
import sys, subprocess, os

LOG = os.environ.get("ZIGAR_LOG")
argv = list(sys.argv[1:])

if LOG:
    try:
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(repr(argv) + "\n")
    except OSError:
        pass

sys.exit(subprocess.run(["zig", "ar"] + argv).returncode)
