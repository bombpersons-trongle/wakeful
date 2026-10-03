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
// battle store, keyed by participant.
fn decide(participant) {
    // The actor's name rides in its own bubble, overlapping the top
    // edge of the command menu (and lifted over it), so it is always
    // clear whose turn this is.
    ui_window_lift("battle-actor", 168.0, 160.0, 144.0, 24.0, 2.0);
    ui_text("battle-actor", display_name(participant), 8.0, 5.0);
    // The command menu rides above the status panel: their rects
    // overlap, and equal-Z panels draw in arbitrary order.
    ui_window_lift("battle", 168.0, 174.0, 144.0, 44.0, 1.0);

    let phase_key = "phase:" + participant.id;
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
            return #{action: "attack", target: foes[picked].id};
        }
        // Circle backs out to the action menu.
        if just_pressed("circle") {
            battle_set(phase_key, ());
        }
        return ();
    }

    ui_options("battle", 8.0, 8.0, ["Attack", "Flee"]);
    let picked = ui_confirmed("battle");
    if picked == 0 {
        battle_set(phase_key, "target");
    }
    if picked == 1 {
        return "flee";
    }
    // Still deciding: the engine asks again next tick.
    ()
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
    if sheets != () && sheets[participant.id] != () {
        return sheets[participant.id].name;
    }
    participant.id
}
