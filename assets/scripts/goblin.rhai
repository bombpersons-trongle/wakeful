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
    // The attack is a choreography file: the hero runs in, swings, and
    // the damage lands on the strike beat (see the file's header).
    battle_action_file("attack", "scripts/battle/attack_weapon.rhai");
    battle_action("flee", "flee_action");
    // Spells are data in the roster; this is the one choreography that
    // casts any of them (the menu stores which spell was chosen).
    battle_action_file("cast", "scripts/battle/cast_spell.rhai");
    // The hero fights with the sheet's numbers: the roster owns them
    // (levels, gear, everything) and the battle bag just mirrors what
    // the fight needs.
    let sheets = recall_global("sheets");
    let sheet = sheets["hero"];
    let ember = sheets["ember"];
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
                bag: #{hp: sheet.hp, max_hp: sheet.max_hp, quick: sheet.quick},
            },
            #{
                id: "ember",
                player: true,
                model: "models/character.glb",
                position: [1.4, 0.0, 3.6],
                facing: 180.0,
                brain: "scripts/battle/player_brain.rhai",
                bag: #{hp: ember.hp, max_hp: ember.max_hp, mp: ember.mp,
                    max_mp: ember.max_mp, quick: ember.quick},
            },
            #{
                id: "goblin",
                model: "models/goblin.glb",
                position: [-0.7, 0.0, -3.0],
                facing: 0.0,
                brain: "scripts/battle/goblin_brain.rhai",
                bag: #{hp: 30.0, max_hp: 30.0, quick: 4.0, mdef: 4.0, xp: 25.0},
            },
            #{
                id: "goblin2",
                model: "models/goblin.glb",
                position: [0.9, 0.0, -3.6],
                facing: 0.0,
                brain: "scripts/battle/goblin_brain.rhai",
                bag: #{hp: 24.0, max_hp: 24.0, quick: 3.0, mdef: 3.0, xp: 18.0},
            },
        ],
    );
}

// The attack: whoever acts plays a swing-ish emote, the nearest foe
// (here: the only foe) takes fixed damage, and the number floats up
// over them. The recovery time this returns is what spaces the turns.
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


