#!/usr/bin/env python3
"""zig cc wrapper for target aarch64-unknown-linux-musl.

Only rewrite the target-triple FLAGS; never touch file paths
(paths point into cargo's target/aarch64-unknown-linux-musl dir).
"""
import sys, subprocess, os

RUST_T = "aarch64-unknown-linux-musl"
ZIG_T = "aarch64-linux-musl"
LOG = os.environ.get("ZIGCC_LOG")

def fix(a):
    if a == RUST_T:
        return ZIG_T
    if a.startswith("--target=" + RUST_T):
        return "--target=" + ZIG_T
    return a

out = []
i = 0
argv = list(sys.argv[1:])
while i < len(argv):
    a = argv[i]
    if a in ("-target", "--target") and i + 1 < len(argv):
        out.append(a)
        out.append(ZIG_T if argv[i + 1] == RUST_T else argv[i + 1])
        i += 2
        continue
    out.append(fix(a))
    i += 1

if LOG:
    try:
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(repr(out) + "\n")
    except OSError:
        pass

sys.exit(subprocess.run(["zig", "cc"] + out).returncode)
