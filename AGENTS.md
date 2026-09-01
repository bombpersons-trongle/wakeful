# Agent Guide

Guidelines for AI agents working on this repository. They apply to every
contribution, whether human-reviewed or machine-made.

## Local extensions

This file holds general project guidance for any agent working on the
repository. Anything specific to one agent or their environment — identity,
voice, sandbox setup, machine paths — belongs in `AGENTS.local.md` instead.

If an `AGENTS.local.md` file exists next to this one, read it too and follow
it as an extension of these guidelines. It is gitignored on purpose and is
never committed; don't count on it existing in fresh checkouts.

## How to write code

High quality or nothing. Concretely:

- **Good isolation.** Small, focused modules with clear boundaries. Dependencies
  injected / passed in, not reached for. No module should need to know how
  another one works inside.
- **Shallow complexity.** Flat call hierarchies, few branches per function,
  early returns over nested ifs. If a function needs a paragraph to explain,
  it probably wants to be two functions.
- **Unit tests.** Every meaningful behavior gets one. Tests run in isolation,
  don't hit the network or the clock, and test behavior — not implementation
  details.
- **Concise, human-readable comments.** Comments explain *why*, not *what*.
  If the code can say it, the comment doesn't. One good line beats a paragraph.

## How to contribute

Contributors work like any outside contributor — no special privileges:

- Work in your own **fork**, on clearly named branches, and deliver
  everything through **pull requests**. Never push directly to this
  repository, even when the token would technically allow it.
- The maintainer reviews and merges. Your job ends at a clean, reviewable PR.
- When the maintainer leaves review comments, fix them promptly, push to the
  same branch, and reply on the thread. Whenever a review comment reveals a
  new guideline — a coding rule, a workflow preference, anything
  generalizable — fold it into this file immediately, without being asked.
- After a merge: sync the fork from upstream and delete the merged branch.
- PRs are small and focused: one logical change, clear description of what
  and why, tests included.

## How to work

- Read before writing. Look at surrounding code and follow the existing
  conventions of the repo, even above your own defaults.
- Run the tests after every change. Lint/typecheck too, if the project has
  them.
- Small, reviewable diffs. One logical change per commit.
- Ask when requirements are fuzzy rather than guessing big. Don't invent
  specifics for things that aren't decided — no filler lore, feature claims,
  or made-up examples in user-facing copy. Keep placeholders general until
  there's a real decision to describe.
- Never commit or push unless explicitly asked.

## Project notes

- **wakeful** is a game built with Bevy 0.19 (edition 2024). `cargo run` to
  play, `cargo test` for unit tests.
- Bevy systems live in `src/systems/<concern>.rs` (e.g. `camera.rs`,
  `debug_draw.rs`); `main.rs` only wires plugins, schedules, and the shared
  components/resources the systems operate on. Self-contained feature modules
  (e.g. the editor) may keep their own systems internally.
- Rendering uses an offscreen pipeline (`src/screen.rs`): the game renders to
  a 320x240 texture, which a present camera integer-upscales with black bars
  — full retro pixelation for 2D and 3D alike.
- Movement runs on a fixed 60hz `FixedUpdate` schedule; the movement math in
  `movement.rs` is ECS-free and unit tested.
- Art direction: FF7-style — 3D characters over pre-rendered backgrounds.
  The ground plane is a placeholder until real background art exists.
- `src/bin/snapshot.rs` renders the scene windowlessly to a PNG, for headless
  verification of visual changes.
