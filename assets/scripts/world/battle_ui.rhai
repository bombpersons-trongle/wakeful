// The battle HUD: a status panel for everyone on the field and the
// floating damage numbers. Runs every tick while a battle is up; does
// nothing otherwise. Battle visuals are script territory — the engine
// only stages the fight and sequences turns.
fn on_update(dt) {
    if in_battle() != true {
        // The battle is over: the script-owned windows go with it —
        // once per battle, not on every tick of peacetime.
        if recall("up") == true {
            remember("up", false);
            print("battle over: closing windows");
            ui_close("battle-status");
            ui_close("battle");
            for id in recall("live_floats") {
                ui_close("float" + id);
            }
            remember("live_floats", []);
        }
        return;
    }
    remember("up", true);

    // TEMP outro bridge (the victory/game-over screens replace this in
    // the next slice): once `finish_battle` parks the fight, end it —
    // once; the exit transition takes a few ticks and end_battle while
    // one is running just warns.
    if battle_finished() && recall("ending") != true {
        remember("ending", true);
        ui_close("battle-status");
        end_battle(battle_result());
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

// Label colors by name; anything unknown renders white.
fn float_color(name) {
    if name == "green" {
        [0.4, 1.0, 0.4]
    } else {
        [1.0, 1.0, 1.0]
    }
}
