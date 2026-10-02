// The standard weapon attack, as a battle choreography: run toward the
// foe, swing, and the damage lands on the strike beat; then run back
// home. Choreographies are pure functions of `state` — elapsed seconds,
// the actor's live position, where it started — so every tick
// recomputes the pose from scratch and the sequencer needs no
// per-action memory.
//
// Effects available to choreographies: `battle_set_position` (move),
// `battle_emote` (clips; re-requesting holds), `battle_emote` reads via
// `battle_participants`, bag writes for damage, `float_text`.

/// First participant that isn't `me`, if any.
fn first_foe(me) {
    for p in battle_participants() {
        if p.id != me.id {
            return p;
        }
    }
    ()
}

/// True on exactly the tick that crosses `beat`.
fn crossed(state, beat) {
    state.time >= beat && state.time - state.dt < beat
}

/// Slide `state.pos` toward `target` by one dt at `speed`.
fn step_toward(actor, state, target, speed) {
    let dx = target.x - state.pos[0];
    let dz = target.z - state.pos[2];
    let dist = sqrt(dx * dx + dz * dz);
    if dist < 0.01 {
        return;
    }
    let step = if dist < speed * state.dt { dist } else { speed * state.dt };
    battle_set_position(actor.id, [
        state.pos[0] + dx / dist * step,
        state.pos[2] + dz / dist * step,
    ]);
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

fn run(actor, state) {
    let foe = first_foe(actor);
    if foe == () {
        // Nobody to hit: idle the turn away.
        return 1.0;
    }

    let walk = 0.55;      // seconds closing in
    let strike = 1.0;     // the swing's damage beat
    let swing_done = 1.9; // clip finished, turn around
    let done = 2.8;       // back home

    if state.time < walk {
        step_toward(actor, state, foe, 3.5);
        ()
    } else if state.time < swing_done {
        // Re-requesting holds the swing without restarting it.
        emote("attack");
        if crossed(state, strike) {
            let dmg = 12.0;
            battle_bag_set(foe.id, "hp", battle_bag_get(foe.id, "hp") - dmg);
            float_text(foe, "-" + dmg);
        }
        ()
    } else if state.time < done {
        step_toward(actor, state, #{ x: state.home[0], z: state.home[1] }, 3.5);
        ()
    } else {
        1.4
    }
}
