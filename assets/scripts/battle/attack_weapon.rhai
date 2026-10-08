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

/// The named participant, alive and on the field.
fn by_id(id) {
    for p in battle_participants() {
        if p.id == id && p.dead != true && p.hp > 0.0 {
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



import "lib/floats" as floats;

fn run(actor, state) {
    // The brain's target pick rides in on `state.target`; fall back to
    // the first living foe when a brain answered bare.
    let foe = ();
    if state.target != () && state.target != "" {
        foe = by_id(state.target);
    }
    if foe == () {
        foe = first_foe(actor);
    }
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
            floats::float_text(foe, "-" + dmg, "white");
        }
        ()
    } else if state.time < done {
        step_toward(actor, state, #{ x: state.home[0], z: state.home[1] }, 3.5);
        ()
    } else {
        recovery(actor)
    }
}

// The turn's recovery: the base beat, scaled by the actor's Quick
// from the sheet (the stat shifts the denominator: 100/(100+quick)).
fn recovery(actor) {
    let base = 1.4;
    let quick = 0.0;
    let sheets = recall_global("sheets");
    if sheets != () && sheets[actor.id] != () && sheets[actor.id].quick != () {
        quick = sheets[actor.id].quick;
    }
    base * 100.0 / (100.0 + quick)
}
