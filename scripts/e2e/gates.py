#!/usr/bin/env python3
"""E2E gate assertions for the c-LoD boot log (CI job).

Gates (all hard):
  G1  the module injected pre-kernel          [c-LoD] cplugin_init
  G2  the runtime admitted it                 pipeline ready: 1 module hook(s)
  G3  the full Bukkit+JOML surface resolved   [c-LoD] bootstrap ok
  G4  the pinning backend probed              tickets=nms ... | tickets=forceload ...
  G5  the engine plans cycles                 [c-LoD] cells= ... (>= 3 status lines)
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

    need("[c-LoD] cplugin_init" in log, "G1 inject", "module never reached cplugin_init")
    need(
        re.search(r"pipeline ready: 1 module hook", log) is not None,
        "G2 admitted",
        "runtime did not load exactly 1 module hook (scan error or double load)",
    )
    m = re.search(r"\[c-LoD\] bootstrap ok: view_distance=(\d+) tickets=(\w+): (.+)", log)
    need(m is not None, "G3 bootstrap ok", "Bukkit/NMS/JOML surface did not resolve — see BOOTSTRAP FAILED above")
    if m:
        vd = int(m.group(1))
        backend = m.group(2)
        need(vd >= 2, "G3a view distance sane", f"view_distance={vd}")
        need(
            backend in ("nms", "forceload"),
            "G3b pinning backend",
            f"backend={backend}",
        )
        print(f"       backend note: {m.group(3).strip()}")
    need("BOOTSTRAP FAILED" not in log, "G3c no bootstrap failure", "bootstrap error logged")
    need(
        re.search(r"\[c-LoD\] levels: \[[\d., ]+\]", log) is not None,
        "G4 level table",
        "engine never built the LOD level table",
    )
    lines = len(re.findall(r"\[c-LoD\] cells=", log))
    need(lines >= 3, "G5 engine cycles", f"only {lines} status lines — worker stalled?")

    print()
    if failed:
        print("E2E: FAILED")
        return 1
    print("E2E: ALL GATES PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
