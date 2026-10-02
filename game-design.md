# Game design

What wakeful is, as currently built. This documents the game that
exists today — the engine's mechanics, the content shipped, and the
design direction they establish — rather than a pitch for what it will
become. Ideas not yet built live in the maintainer's local notes.

## Vision

A classic-style JRPG in the spirit of the genre's 16/32-bit era:
3D characters walking over pre-rendered painted backgrounds, a fixed
camera per room, chunky pixel-fidelity presentation, and a world that
carries the player's story forward through its characters and places.
Dialogue, menus, and treasure behave the way they do in the classics —
speech bubbles waiting on a confirm, a pause menu that stops the world,
chests that stay open because you opened them.

## Presentation

- The game renders to an internal 320×240 image — the virtual screen —
  which is integer-upscaled to the window with black bars, so every
  pixel is crisp regardless of window size. A post-process pass adds a
  subtle CRT-style dither over that image.
- Type is a pixel bitmap font (Pixel Operator), drawn at pixel scale
  to match the 16-bit look.
- Devroom's backgrounds are real photographs — standing in for the
  painted background art a production would commission. Each background
  ships with a depth map, and the engine turns the pair into a depth
  card: real geometry in the scene camera's pass, so characters and
  background occlude each other correctly (step behind an awning and it
  covers you). The ground grid in each scene tells the engine where the
  player may walk, matching the fixed-camera staging of the genre.

## Camera and movement

- Each scene [owns a fixed camera](assets/scenes/devroom.scene): a pose
  (position, target, field of view) chosen for the room. Walking between
  scenes keeps the model consistent and stages the new room with its
  own camera.
- The player is a chibi character (about 2.7 heads tall) on a
  walkable grid; fine steps follow the
  grid so movement reads crisply against the background while staying
  collision-free.
- Walk and run are distinct gaits: arrows or WASD (or the left stick)
  to move, hold Shift or R2 to run. Characters turn as they go, and the
  gait follows actual displacement — pushing into a wall reads as
  standing, not walking.

## The world, scenes, and actors

A scene file (RON) declares everything a single room contains:

- the background image, its depth map, and the camera pose — together
  they become the depth card the scene renders as its room,
- the walkable grid,
- teleporters that carry the player to another scene at an arrival
  point (with re-arming so the doorway doesn't instantly bounce back),
- the scene's own script — a program whose lifetime is exactly the
  scene's (they may open choice windows in `on_enter`, run logic in
  `on_update`, and clean up in `on_exit`),
- actors: positioned characters with models, optional world facings,
  and their own scripts.

The player entity persists across scenes — a view of who currently
leads the party — so you never flicker back to a placeholder at a
doorway; only the staging changes.

The player also has a pause menu on triangle, engine-driven (windows,
bars, an option list with a hand cursor) but staged by a world script:
character stats shown are placeholders until player data exists, and
the menu notes which characters you have actually met.

## Scripting

Everything story-shaped is a script: actors, scenes, and the world
itself are each small Rhai programs with their own persistent memory.
The engine owns persistence, input, dialogue, and menus; content is
Rhai, which keeps story work lightweight and lets non-engine scripts
tweak behavior.

- **Actor scripts** run per character each fixed tick: they greet,
  follow, open, and hand things over.
- **Scene scripts** exist for exactly one scene's lifetime.
- **World scripts** run for the life of the game: the party roster is
  declared by a world script, as are menus like the triangle menu.

What they can do, in broad strokes:

- **State**: `remember`/`recall` private state that persists across
  scene changes, and `remember_global`/`recall_global` for the shared
  world state (the future save file — whose first real customer is
  party membership and the met-you-once flags).
- **Party members**: defined entirely in a world script, switchable by
  script (recruiting at an NPC, scenes that force a member). The
  field character is whoever the party says it is, and the model
  persists across teleports.
- **Dialogue**: written lines surface as speech bubbles (with options
  for placement, timing, and wait-for-confirm), the classic pattern.
- **Emotes and poses**: scripts can play clips or snap the character
  into a held final pose — that's how a chest stays open.
- **Menus**: windows, positional text, bars, and cursor-driven option
  lists; the engine handles cursor navigation, pauses the world while
  a menu asks it to, and scene changes close stray windows.
- **Input playback**: invoke any configured PlayStation action by name
  (as found in `assets/input.ron`), reading presses or just-pressed
  edges. A script can also *capture* buttons per tick — a captured
  button reads false everywhere else (engine, other scripts), so a
  mode like the free camera can own the d-pad without the game
  feeling a ghost press.
- **Per-instance configuration**: `params` from the scene data are
  readable as typed values, so one chest script serves every chest.

## Combat direction

No combat exists yet. The design target is the classic JRPG loop the
preamble establishes: encounter, turn-based party combat, HP/MP that
the menu reads, and a world state beneath it that can later persist.
When combat lands, it will likewise be scripts orchestrating, with the
engine providing the choreography.

## Current playable state

`cargo run` gives you:

- A pre-rendered street scene with an FF-style staging: photoreal
  background, fixed camera, walkable strip.
- A chibi player with walk/run animations; a party of one,
  bootstrapped by a world script.
- A goblin that trots after you and counts your meetings.
- Two chests that swing open when approached — with the rotation to
  read them well — and stay open when you leave and return.
- A pause menu on triangle (pauses the world) with HP/MP bars, an
  options list, and a befriended-goblin status line.
- Full gamepad + remappable-keyboard support throughout.
