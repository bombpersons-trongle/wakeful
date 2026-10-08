// Casting a spell. One choreography for every spell: the battle menu
// stored the chosen spell's name, and its numbers (power, mp cost)
// come from the roster's job tables, so a new spell is data, not code.
//
// The beat: pay and pose on the opening tick, the magic lands on the
// strike beat (magic x2 + power - the target's magic defence, within a
// tenth either way), then hold until the turn's recovery.
import "lib/roster" as r;
import "lib/floats" as floats;

fn run(actor, state) {
    let name = battle_get("spell");
    let def = if name == () { () } else { r::spell(name) };
    if def == () {
        // Nothing castable was named (the menu would not offer it):
        // spend the turn on nothing.
        return 0.4;
    }
    let foe = current_foe(actor, state);
    if foe == () {
        return 1.0;
    }
    let cast = 0.4;
    let strike = 0.9;
    let done = 1.4;

    if state.time < cast {
        // Opening beat: the pose starts and the mp is spent - on the
        // first tick only (the sequencer hands us time == dt there).
        emote("cast");
        if state.time <= state.dt * 1.5 && !spend(actor, def.cost) {
            // No mp, no cast: the turn is spent saying so.
            floats::float_text(actor, "Not enough MP", "white");
            return 0.6;
        }
        ()
    } else if state.time < done {
        emote("cast");
        if crossed(state, strike) {
            let dmg = damage(actor, foe, def.power);
            battle_bag_set(foe.id, "hp", battle_bag_get(foe.id, "hp") - dmg);
            floats::float_text(foe, "" + dmg, "green");
        }
        ()
    } else {
        // Quick scales the recovery like every other action; casting
        // takes a touch longer than a swing.
        1.2 * 100.0 / (100.0 + stat_of(actor.id, "quick"))
    }
}

/// Pays `cost` mp from the actor's bag. False when there isn't enough.
fn spend(actor, cost) {
    let mp = battle_bag_get(actor.id, "mp");
    if mp == () || mp < cost {
        return false;
    }
    battle_bag_set(actor.id, "mp", mp - cost);
    true
}

/// The magic formula: the caster's Magic doubled, plus the spell's
/// power, less the target's Magic Defence - never below one, and
/// within a tenth either way.
fn damage(actor, foe, power) {
    let raw = power + stat_of(actor.id, "magic") * 2.0 - defence_of(foe);
    let base = if raw < 1.0 { 1.0 } else { raw };
    base + base * roll(0.1)
}

/// The target's magic defence: its battle bag first (monsters are
/// bag-defined and have no sheet), the sheet as a fallback.
fn defence_of(foe) {
    let from_bag = battle_bag_get(foe.id, "mdef");
    if from_bag != () {
        return from_bag;
    }
    stat_of(foe.id, "mdef")
}

/// A stat off the actor's sheet (0 when the sheet says nothing).
fn stat_of(id, stat) {
    let sheets = recall_global("sheets");
    if sheets == () || sheets[id] == () {
        return 0.0;
    }
    let value = sheets[id][stat];
    if value == () {
        0.0
    } else {
        value
    }
}

/// The named target from the menu, or the first living enemy when it
/// fell mid-cast.
fn current_foe(actor, state) {
    if state.target != () && state.target != "" {
        for p in battle_participants() {
            if p.id == state.target && p.hp > 0.0 {
                return p;
            }
        }
    }
    for p in battle_participants() {
        if p.id != actor.id && p.player != true && p.dead != true && p.hp > 0.0 {
            return p;
        }
    }
    ()
}

/// True exactly on the tick that reaches `beat`.
fn crossed(state, beat) {
    state.time >= beat && state.time - state.dt < beat
}