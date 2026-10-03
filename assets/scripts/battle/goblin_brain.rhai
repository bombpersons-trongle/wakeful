// A reusable enemy brain: this goblin always attacks, and with more
// than one player on the field it picks the first one still standing.
// Brains answer either a bare action name or
// #{action: ..., target: ...} — the target rides along to the
// choreography as `state.target`.
//
// Pack tactics and boss coordination work by pointing several
// participants at one brain file and coordinating through
// battle_set / battle_get.
fn decide(participant) {
    for p in battle_participants() {
        if p.player == true && p.dead != true && p.hp > 0.0 {
            return #{action: "attack", target: p.id};
        }
    }
    // Nobody standing to hit: idle the turn away.
    ()
}
