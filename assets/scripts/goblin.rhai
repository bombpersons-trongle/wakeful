// The devroom goblin: trails the player, stopping at a polite
// distance, and remarks on how many times you have met. The count
// lives in this actor's private store — the scene gives it
// id: Some("goblin"), which is the key — so it survives leaving the
// room and coming back. The meeting itself is announced in the shared
// global store for other scripts to react to.
//
// Press cross while close to challenge the goblin: it builds the whole
// battle instance right here — participants, actions, brains — and
// hands it to the engine, which stages the arena and sequences turns.
fn on_update(x, z, player_x, player_z, dt) {
    if waiting() { return (); }
    remember_global("met.goblin", true);
    let dx = player_x - x;
    let dz = player_z - z;
    let dist = sqrt(dx*dx + dz*dz);
    if dist >= 2.0 {
        // Out of conversation range: re-arm the greeting so the next
        // approach counts as another meeting.
        remember("said", false);
        let speed = 1.5;
        let step = min(speed * dt, dist - 2.0);
        return [x + dx / dist * step, z + dz / dist * step];
    }
    // Say it once per meeting; the wait bubble holds the rest. The
    // cross that dismisses the bubble must not also be the challenge,
    // so the dismissal tick is swallowed and the next cross fights.
    if recall("said") == true {
        let up = waiting();
        let was_up = recall("up") == true;
        remember("up", up);
        let dismissal = was_up && !up;
        if recall("battled") != true && just_pressed("cross") && !dismissal && !up {
            remember("battled", true);
            challenge();
        }
        return ();
    }
    remember("said", true);
    // Rhai expressions have a nesting budget; keeping the branchy
    // greeting in its own function keeps on_update shallow.
    say(greet(), #{wait: true, at: [160.0, 40.0]});
    ()
}

fn greet() {
    let visits = recall("visits");
    if visits == () {
        remember("visits", 1);
        return "Oh! You can see me?";
    }
    if visits == 1 {
        remember("visits", 2);
        return "Back already? I do not mind.";
    }
    remember("visits", visits + 1);
    return "Visit number " + visits + ". Persistent.";
}


// ---------------------------------------------------------------- battle

// The battle instance: any script can start a battle with any
// participants. The engine owns the staging and the turn sequencer;
// everything about WHO fights and WHAT the actions do lives here.
fn challenge() {
    battle_action("attack", "attack_action");
    battle_action("flee", "flee_action");
    start_battle(
        "models/arena.glb",
        #{
            pos: [10.0, 9.0, 10.0],
            look: [0.0, 0.5, 0.0],
        },
        [
            #{
                id: "hero",
                player: true,
                position: [0.0, 0.0, 3.0],
                facing: 180.0,
                brain: "scripts/battle/player_brain.rhai",
                bag: #{hp: 87.0, max_hp: 100.0},
            },
            #{
                id: "goblin",
                model: "models/goblin.glb",
                position: [0.0, 0.0, -3.0],
                facing: 0.0,
                brain: "scripts/battle/goblin_brain.rhai",
                bag: #{hp: 30.0, max_hp: 30.0},
            },
        ],
    );
}

// The attack: whoever acts plays a swing-ish emote, the nearest foe
// (here: the only foe) takes fixed damage, and the number floats up
// over them. The recovery time this returns is what spaces the turns.
fn attack_action(actor_id) {
    battle_emote(actor_id, "pick_up");
    let foe = first_foe(actor_id);
    if foe != () {
        let hp = battle_bag_get(foe.id, "hp") - 12.0;
        battle_bag_set(foe.id, "hp", hp);
        float_text(foe, "-12");
        if hp <= 0 {
            end_battle("victory");
        }
    }
    1.2
}

fn flee_action(actor_id) {
    end_battle("fled");
    0.5
}

fn first_foe(me) {
    for p in battle_participants() {
        if p.id != me {
            return p;
        }
    }
    ()
}

// Floating damage numbers are script-side too: drop an entry in the
// battle store and the battle UI world script projects and animates it.
fn float_text(foe, text) {
    let floats = battle_get("floats");
    if floats == () {
        floats = [];
    }
    floats.push(#{text: text, x: foe.x, y: foe.y + 1.4, z: foe.z, age: 0.0});
    battle_set("floats", floats);
}
