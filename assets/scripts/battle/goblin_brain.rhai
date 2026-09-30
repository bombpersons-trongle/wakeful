// A reusable enemy brain: this goblin always attacks. Brains return
// the name of a registered battle action, or nothing while "thinking".
//
// Pack tactics and boss coordination work by pointing several
// participants at one brain file and coordinating through
// battle_set / battle_get.
fn decide(participant) {
    "attack"
}
