// The roster scheduler: all of the character-sheet logic lives in the
// shared library (scripts/lib/roster.rhai) so the battle scripts, the
// debug console and this tick all run the exact same code.
import "lib/roster" as r;

fn on_update(dt) {
    r::tick_roster();
}
