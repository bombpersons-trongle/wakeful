---
name: wakeful-debug
description: Drive, inspect, and script the running wakeful game through its debug MCP server (debug builds only, port 8399).
---

# wakeful debug control

The wakeful game (debug builds) serves an MCP server at
`http://127.0.0.1:8399/mcp` while it runs. Tools appear as
`wakeful__*`. If a tool call times out, the game is not running (or is
wedged) — launch it first:

```bash
mkdir -p .debug/shots
nohup ./target/debug/wakeful > .debug/log 2>&1 &
sleep 40   # asset + scene load
```

Launching with `--console` instead adds an interactive rhai REPL on
stdin (same evaluator as the `eval` tool) for hands-on sessions —
line editing with up/down history on a terminal, plain lines when
stdin is piped.

`BEVY_ASSET_ROOT` or `WAKEFUL_MCP_PORT` can override defaults. A busy
port is skipped quietly by a second instance.

## Tools

- `screenshot` — the game frame as a PNG (also written to
  `.debug/shots/`). Use it after every input or eval to see the result.
- `tap {action}` / `hold {action}` / `release {action}` — inject input
  using configured pad names: cross, circle, triangle, square,
  dpad_up/down/left/right, l1..r3, start, select. Taps are one-tick
  edges; holds press until release.
- `state` — text dump: shared world stores, battle participants,
  active camera, actors, animation drivers. Read this before and after
  anything you change.
- `eval {code}` — run rhai against the live game: `remember_global` /
  `recall_global`, UI calls, battle reads/writes, `warp_to(scene, x,
  z)`, `teleport_player(x, z)`, party mutators, and the shared
  libraries (pre-imported): `r::give_xp("hero", 50)`,
  `floats::float_text(...)`. Multi-statement snippets are fine; the
  last expression comes back as the result.

## Workflow

1. Launch the game (above) and confirm the MCP tools respond.
2. `screenshot` to see where things stand; `state` for internals.
3. Prefer `eval` over restarts: grant xp, warp between scenes, poke
   store keys, all without recompiling. Scripts themselves still need
   an edit + relaunch (or a scene change to recompile).
4. Drive the player with `tap`/`hold` (edges matter: menus read
   one-tick confirm presses — tap, screenshot, tap).
5. Kill with `pkill -9 -x wakeful` when done.

## Gotchas

- The warp/teleport requests only apply in the Scene state.
- `eval` snippets that error return the rhai message — nothing breaks.
- Screenshots land in `.debug/shots/` named by epoch second; the MCP
  reply carries the same bytes inline.
