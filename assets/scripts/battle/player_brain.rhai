// The default player-side battle brain: while it is this
// participant's turn, the engine polls `decide` every tick. Showing
// the command menu and waiting is just "return nothing until the
// player picks something" — the engine has no idea a menu exists.
//
// `participant` is a map of the engine fields (id, position,
// time_until_act) merged with this participant's bag (hp, max_hp, ...).
fn decide(participant) {
    // Options only: the battle HUD (world script) owns the HP bars.
    ui_window("battle", 208.0, 176.0, 104.0, 56.0);
    ui_options("battle", 8.0, 8.0, ["Attack", "Flee"]);

    let picked = ui_confirmed("battle");
    if picked == 0 {
        return "attack";
    }
    if picked == 1 {
        return "flee";
    }
    // Still deciding: the engine asks again next tick.
    ()
}
