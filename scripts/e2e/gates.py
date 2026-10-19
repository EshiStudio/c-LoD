#!/usr/bin/env python3
"""E2E gate assertions for the c-LoD boot log (live-server job).

Posteffect architecture (v2). Gates (all hard):
  G1  the module injected pre-kernel          [c-LoD] booting: atlas ...
  G2  the runtime admitted it                 pipeline ready: 1 module hook(s)
  G3  the kernel came up and the loop started [c-LoD] kernel ready — starting engine loop
  G4  the Paper JNI surface resolved          [c-LoD] JNI bridge resolved: true
  G5  the atlas world resolved                [c-LoD] atlas world: <name>
  G6  a resource pack was generated           [c-LoD] pack rebuilt: atlas vN -> X bytes, sha1 Y
Exit 0 only when every gate passes.
"""
import re
import sys


def main(path: str) -> int:
    log = open(path, encoding="utf-8", errors="replace").read()

    def need(cond: bool, gate: str, hint: str) -> None:
        if cond:
            print(f"  OK   {gate}")
        else:
            print(f"  FAIL {gate} — {hint}")
            globals()["failed"] = True

    failed = False

    need(
        re.search(r"\[c-LoD\] booting: atlas \d+x\d+ blocks", log) is not None,
        "G1 inject (cplugin_init, config parsed)",
        "module never reached cplugin_init or config parsing failed",
    )
    need(
        re.search(r"pipeline ready: 1 module hook", log) is not None,
        "G2 runtime admitted the module",
        "runtime did not load exactly 1 module hook (scan error or double load)",
    )
    need(
        "kernel ready — starting engine loop" in log,
        "G3 kernel ready, engine loop started",
        "Bukkit never came up / engine thread never started",
    )
    need(
        re.search(r"JNI bridge resolved: true", log) is not None,
        "G4 Paper JNI surface resolved",
        "Paper 26.3 API surface incomplete — module idles (see JNI FAILED above)",
    )
    m = re.search(r"atlas world: (\S+)", log)
    need(m is not None, "G5 atlas world resolved", "no Bukkit world available for the heightmap atlas")
    pm = re.search(r"pack rebuilt: atlas v(\d+) -> (\d+) bytes, sha1 ([0-9a-f]{40})", log)
    need(pm is not None, "G6 resource pack generated", "packgen never produced clod-pack.zip")
    if pm:
        version = int(pm.group(1))
        size = int(pm.group(2))
        need(version >= 1, "G6a atlas has data (v>=1)", f"atlas version={version}")
        need(size > 1024, "G6b pack size sane (>1KB)", f"pack size={size}")

    print()
    if failed:
        print("E2E: GATES FAILED")
        return 1
    print("E2E: ALL GATES PASSED")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("usage: gates.py <boot-log-file>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
