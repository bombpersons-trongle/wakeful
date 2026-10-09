#!/usr/bin/env bash
# Re-export a scene's assets from its Blender file, then fold the result
# into the .scene file.
#
#     tools/export_scenes.sh raw_assets/village_whole.blend [--resolution 640x480]
#     tools/export_scenes.sh raw_assets/character.blend --name hero --resolution 320x240
#
# Blender writes the plate, the depth map, and a .export file with the half
# of the scene it owns. scene-merge then merges that half into the .scene
# file, keeping the actors, teleporters and scene script that the editor
# and hand-written scenes own — so re-exporting never costs you a
# placement.
#
# The exporter runs through flatpak, whose sandbox cannot see this
# checkout's cwd; the script and the exporter both resolve everything from
# their own location, so it does not matter. BLENDER overrides the command,
# and may be several words (which is why it is an array):
#
#     BLENDER=blender tools/export_scenes.sh raw_assets/village_whole.blend
#     BLENDER="flatpak run org.blender.Blender" tools/export_scenes.sh ...

set -euo pipefail

if [ $# -lt 1 ]; then
    echo "usage: $(basename "$0") <blend file> [exporter args...]" >&2
    exit 2
fi

repo="$(cd "$(dirname "$0")/.." && pwd)"
blend="$1"
shift

if [ ! -f "$repo/$blend" ] && [ ! -f "$blend" ]; then
    echo "$(basename "$0"): no such blend file: $blend" >&2
    exit 2
fi
case "$blend" in
    /*) ;;
    *) blend="$repo/$blend" ;;
esac

read -r -a blender <<< "${BLENDER:-flatpak run org.blender.Blender}"

echo "==> exporting $blend"
"${blender[@]}" --background "$blend" -P "$repo/tools/generate_scene.py" -- "$@"

echo "==> merging exports into the scenes"
cd "$repo"
cargo run --quiet --bin scene-merge -- assets/scenes/*.export

echo "==> done"