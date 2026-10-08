// The default player-side battle brain, with target selection. While
// it is this participant's turn the engine polls `decide` every tick;
// showing menus and waiting is just "return nothing until the player
// has committed". The flow per turn:
//
//   1. the action menu (Attack / Flee), with the actor's name on top
//      so it is always clear whose turn it is;
//   2. Attack opens the target list — living enemies, cursor-driven,
//      circle backs out to the action menu.
//
// Brain calls are stateless, so the target-picking phase lives in the
// battle store, keyed by participant (and which action was chosen on
// the way in).
import "lib/roster" as r;

fn decide(participant) {
    // The actor's name rides in its own bubble, overlapping the top
    // edge of the command menu (and lifted over it), nudged left so
    // the two bubbles don't align edge to edge.
    ui_window_lift("battle-actor", 150.0, 160.0, 144.0, 24.0, 2.0);
    ui_text("battle-actor", display_name(participant), 8.0, 5.0);
    // The command menu rides above the status panel: their rects
    // overlap, and equal-Z panels draw in arbitrary order.
    ui_window_lift("battle", 168.0, 174.0, 144.0, 44.0, 1.0);

    let phase_key = "phase:" + participant.id;
    let pending_key = "pending:" + participant.id;
    if battle_get(phase_key) == "target" {
        let foes = living_foes();
        if foes.len() == 0 {
            battle_set(phase_key, ());
            return ();
        }
        ui_text("battle", "Choose a target", 8.0, 8.0);
        let labels = [];
        for f in foes {
            labels.push(f.id);
        }
        ui_options("battle", 8.0, 24.0, labels);
        let picked = ui_confirmed("battle");
        if picked >= 0 {
            battle_set(phase_key, ());
            // Committed: the menus come down while the choreography
            // runs, and return with the next turn.
            ui_close("battle");
            ui_close("battle-actor");
            let action = battle_get(pending_key);
            battle_set(pending_key, ());
            if action == "cast" {
                return #{action: "cast", target: foes[picked].id};
            }
            return #{action: "attack", target: foes[picked].id};
        }
        // Circle backs out to the action menu.
        if just_pressed("circle") {
            battle_set(phase_key, ());
        }
        return ();
    }

    // The action menu is data: a fighter sees Attack / Flee, a mage
    // sees her known spells (that she can currently pay for) between
    // them.
    let labels = action_labels(participant);
    ui_options("battle", 8.0, 8.0, labels);
    let picked = ui_confirmed("battle");
    if picked >= 0 {
        let choice = labels[picked];
        if choice == "Attack" {
            battle_set(pending_key, "attack");
            battle_set(phase_key, "target");
        } else if choice == "Flee" {
            ui_close("battle");
            ui_close("battle-actor");
            return "flee";
        } else {
            // A spell: name it for the choreography, then target.
            battle_set("spell", choice);
            battle_set(pending_key, "cast");
            battle_set(phase_key, "target");
        }
    }
    // Still deciding: the engine asks again next tick.
    ()
}

/// Attack, every spell this participant knows AND can pay for right
/// now, then Flee.
fn action_labels(participant) {
    let labels = ["Attack"];
    let sheets = recall_global("sheets");
    if sheets != () && sheets[participant.id] != () {
        let known = sheets[participant.id].spells;
        if known != () {
            let mp = if participant.mp == () { 0.0 } else { participant.mp };
            for name in known {
                let def = r::spell(name);
                if def != () && def.cost <= mp {
                    labels.push(name);
                }
            }
        }
    }
    labels.push("Flee");
    labels
}

fn living_foes() {
    let foes = [];
    for p in battle_participants() {
        if p.player != true && p.dead != true && p.hp > 0.0 {
            foes.push(p);
        }
    }
    foes
}

fn display_name(participant) {
    let sheets = recall_global("sheets");
    if sheets != () && sheets[participant.id] != () && sheets[participant.id].name != () {
        return sheets[participant.id].name;
    }
    participant.id
}
