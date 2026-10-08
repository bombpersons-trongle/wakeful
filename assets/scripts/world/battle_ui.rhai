// The battle HUD: the status panel for the player side and the
// floating damage numbers, plus the victory and defeat screens (the
// fight parks itself when it ends, so the outro can live here while
// still technically in battle state). Runs every tick while a battle
// is up; does nothing otherwise. Battle visuals are script territory
// — the engine only stages the fight and sequences turns.
import "lib/roster" as r;

fn on_update(dt) {
    if in_battle() != true {
        // The battle is over: the script-owned windows go with it —
        // once per battle, not on every tick of peacetime.
        if recall("up") == true {
            remember("up", false);
            print("battle over: closing windows");
            ui_close("battle-status");
            ui_close("battle");
            // The acting label is the battle brain's window; the brain
            // stops being polled once the fight is over.
            ui_close("battle-actor");
            for id in recall("live_floats") {
                ui_close("float" + id);
            }
            remember("live_floats", []);
            ui_close("outro");
            // Next fight starts a fresh outro.
            remember("outro", ());
        }
        return;
    }
    remember("up", true);

    // A finished fight parks itself: this is where the outro takes
    // over until the player confirms, then hands back to the engine.
    if battle_finished() {
        outro();
        return;
    }

    // The status panel is player-side only: enemies are visible on
    // the field, so they get no row. Height follows the party.
    let folks = battle_participants();
    let players = [];
    for p in folks {
        if p.player == true {
            players.push(p);
        }
    }
    ui_window("battle-status", 8.0, 170.0, 192.0, 14.0 * players.len() + 12.0);
    let row = 8.0;
    for p in players {
        ui_text("battle-status", p.id, 8.0, row);
        if p.max_hp != () && p.max_hp > 0 {
            ui_bar("battle-status", 70.0, row + 2.0, 90.0, p.hp / p.max_hp);
            ui_text("battle-status", "" + p.hp + " / " + p.max_hp, 76.0, row + 2.0, [1.0, 1.0, 1.0]);
        }
        row = row + 14.0;
    }

    // Floating damage numbers: entries the battle script pushed into
    // the battle store, rendered at the participant's screen position
    // and floated up by age. Bare colored labels — no bubble — in the
    // color the action chose (white damage, green healing). Each float
    // owns a stable id, so two numbers never share a label, and ids
    // that age out get their labels closed.
    let floats = battle_get("floats");
    let live = [];
    if floats != () {
        let kept = [];
        for f in floats {
            let age = f.age + dt;
            if age < 1.2 {
                let at = battle_screen_pos(f.x, f.y, f.z);
                if at.onscreen == true {
                    ui_label("float" + f.id, f.text, at.x - 12.0, at.y - 14.0 - age * 26.0,
                        float_color(f.color));
                }
                f.age = age;
                kept.push(f);
                live.push(f.id);
            }
        }
        battle_set("floats", kept);
    }
    // Labels whose floats aged out (or left the screen) must be closed
    // explicitly — a label lives until closed or re-issued.
    let previous = recall("live_floats");
    if previous != () {
        for id in previous {
            if live.index_of(id) < 0 {
                ui_close("float" + id);
            }
        }
    }
    remember("live_floats", live);
}

// The victory and defeat screens. They run in the parked battle:
// whoever won, the screen says so, and a confirm hands over — victory
// pays out and exits to the scene, defeat wipes the store and returns
// to the title.
fn outro() {
    // Already settled: the confirm closed the window and handed over.
    // The battle can take a few ticks to actually end, and until it
    // does this script keeps running - it must not re-declare the
    // screen (windows live until closed).
    if recall("outro") == "done" {
        return;
    }
    let victory = battle_result() == "victory";
    if recall("outro") != "shown" {
        remember("outro", "shown");
        close_battle_windows();
        ui_window("outro", 40.0, 56.0, 240.0, 128.0);
        if victory {
            let gain = xp_per_survivor();
            ui_text("outro", "Victory!", 16.0, 64.0);
            ui_text("outro", "The party gains " + gain + " experience.", 8.0, 88.0);
            ui_text("outro", "The goblins left nothing behind.", 8.0, 102.0);
        } else {
            ui_text("outro", "Game Over", 16.0, 72.0);
            ui_text("outro", "The party was defeated.", 8.0, 96.0);
            ui_text("outro", "Press cross to return to the title.", 8.0, 112.0);
        }
        ui_options("outro", 88.0, 120.0, ["Continue"]);
        return;
    }
    // The confirm is an edge: the options list exists so the nav owns
    // it like every other menu.
    if ui_confirmed("outro") < 0 {
        ui_options("outro", 88.0, 120.0, ["Continue"]);
        return;
    }
    ui_close("outro");
    remember("outro", "done");
    if victory {
        settle_victory();
        end_battle("victory");
    } else {
        reset_to_start_menu();
    }
}

/// Closes everything the fight owned, so only the outro shows.
fn close_battle_windows() {
    ui_close("battle-status");
    ui_close("battle");
    ui_close("battle-actor");
    // No HUD tick ran before a fast loss, so the key may not exist.
    let floats = recall("live_floats");
    if floats != () {
        for id in floats {
            ui_close("float" + id);
        }
    }
    remember("live_floats", []);
}

/// The xp a surviving member earns: everything the enemies carried,
/// split across the party still standing.
fn xp_per_survivor() {
    let total = 0.0;
    let survivors = 0;
    for p in battle_participants() {
        if p.player != true && p.xp != () {
            total += p.xp;
        }
        if p.player == true && p.dead != true && p.hp > 0.0 {
            survivors += 1;
        }
    }
    if survivors == 0 {
        0.0
    } else {
        total / survivors
    }
}

/// Posts the xp into the roster mailbox (the roster script levels up
/// next tick) and writes each survivor's final vitals to its sheet, so
/// the fight's damage carries forward.
fn settle_victory() {
    let gain = xp_per_survivor();
    for p in battle_participants() {
        if p.player == true && p.dead != true && p.hp > 0.0 {
            r::give_xp(p.id, gain);
            let mp = if p.mp == () { 0.0 } else { p.mp };
            r::set_vitals(p.id, p.hp, mp);
        }
    }
}

// Label colors by name; anything unknown renders white.
fn float_color(name) {
    if name == "green" {
        [0.4, 1.0, 0.4]
    } else {
        [1.0, 1.0, 1.0]
    }
}
