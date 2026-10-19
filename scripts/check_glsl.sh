#!/usr/bin/env bash
# Validates the c-LoD post-effect shaders with glslangValidator.
#
# The shaders are written for Minecraft's shader preprocessor:
#   - `#moj_import <minecraft:globals.glsl>` is resolved by the game, so we
#     replace it with a minimal stub of the Globals UBO (verified against
#     the vanilla box_blur.fsh consumer).
#   - Custom UBOs come from the chain JSON at runtime; the harness appends
#     declaration stubs collected from assets/post_effect/*.json.
#   - texCoord is the varying produced by vanilla core/screenquad.vsh.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v glslangValidator >/dev/null || {
  echo "glslangValidator not found" >&2
  exit 1
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

python3 - "$WORK" <<'PYEOF'
import json, re, sys
from pathlib import Path

work = Path(sys.argv[1])

GLOBALS_STUB = """layout(std140) uniform Globals {
    ivec3 CameraBlockPos;
    float GlintAlpha;
    vec3 CameraOffset;
    float GameTime;
    vec2 ScreenSize;
    int MenuBlurRadius;
    int UseRgss;
};"""

def load_chain(p: Path):
    # Templates contain __TOKEN__ placeholders (substituted by packgen);
    # replace them with 0 for validation.
    txt = re.sub(r"__[A-Z0-9_]+__", "0", p.read_text())
    return json.loads(txt)

ubos = {}
for chain in Path("assets/post_effect").glob("*.json"):
    j = load_chain(chain)
    for p in j.get("passes", []):
        for name, uniforms in (p.get("uniforms") or {}).items():
            ubos[name] = [(u["name"], u["type"]) for u in uniforms]

for fsh in sorted(Path("assets/glsl").glob("*.fsh")):
    src = fsh.read_text()
    src = src.replace("#moj_import <minecraft:globals.glsl>", GLOBALS_STUB)

    # Harness-only: binding layout qualifier needs 420pack under GL semantics.
    src = src.replace(
        "#version 330",
        "#version 330\n#extension GL_ARB_shading_language_420pack : enable",
        1,
    )

    # glslang -G (SPIR-V) demands explicit bindings; Minecraft assigns them
    # at runtime, so the harness auto-numbers UBO blocks.
    seen = {}
    def add_binding(m):
        name = m.group(1)
        if name not in seen:
            seen[name] = len(seen) + 1
        return f"layout(std140, binding={seen[name]}) uniform {name}"
    src = re.sub(r"layout\(std140\) uniform (\w+)", add_binding, src)

    declared = set(re.findall(r"layout\(std140, binding=\d+\) uniform (\w+)", src))
    used = set(re.findall(r"uniform\s+(\w+)\s*\{", src))
    extra = []
    for name in sorted(used - declared):
        if name not in ubos:
            print(f"FAIL {fsh.name}: UBO {name} neither declared nor in chain JSONs")
            sys.exit(1)
        binding = len(seen) + 1
        seen[name] = binding
        lines = [f"layout(std140, binding={binding}) uniform {name} {{"]
        for fname, ftype in ubos[name]:
            lines.append(f"    {ftype} {fname};")
        lines.append("};")
        extra.append("\n".join(lines))

    if "layout(location = 0) in vec2 texCoord" not in src:
        extra.append("layout(location = 0) in vec2 texCoord;")
    (work / (fsh.stem + ".comp.frag")).write_text(src + "\n" + "\n".join(extra) + "\n")
PYEOF

status=0
for comp in "$WORK"/*.comp.frag; do
    base=$(basename "$comp" .comp.frag)
    if glslangValidator -S frag -G -o /dev/null "$comp" > "$WORK/$base.log" 2>&1; then
        echo "OK   assets/glsl/$base.fsh"
    else
        echo "FAIL assets/glsl/$base.fsh"
        cat "$WORK/$base.log"
        status=1
    fi
done

# Chain JSONs must be structurally valid (templates with __TOKEN__ placeholders).
python3 - <<'PYEOF'
import json, re, sys
from pathlib import Path
ok = True
for chain in sorted(Path("assets/post_effect").glob("*.json")):
    try:
        txt = re.sub(r"__[A-Z0-9_]+__", "0", chain.read_text())
        j = json.loads(txt)
        assert "passes" in j and "targets" in j
        for p in j["passes"]:
            assert "vertex_shader" in p and "fragment_shader" in p
            assert "output" in p and "inputs" in p
        print(f"OK   {chain}")
    except Exception as e:
        print(f"FAIL {chain}: {e}")
        ok = False
sys.exit(0 if ok else 1)
PYEOF

[ "$status" -eq 0 ] && echo "all shaders valid"
exit "$status"
