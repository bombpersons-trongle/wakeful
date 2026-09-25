// The starting party. The game boots with an empty roster — the field
// player is the placeholder capsule until a leader is declared here.
//
// These are idempotent state writes, so re-declaring them every tick is
// free: the engine only acts when the party actually changes. (Don't
// guard with a scope variable like `if ready < 1` — scripts can't
// assign to variables that don't exist yet; the first tick would error
// and disable the script.)

fn on_update(dt) {
    // The chibi placeholder from tools/generate_character.py: has the
    // standard clips, so the player walks, runs, and emotes.
    party_add("hero", "Hero", "models/character.glb");
    party_leader("hero");
}
