// The devroom goblin: trails the player, stopping at a polite
// distance, and remarks on how many times you have met. The count
// lives in this actor's private store — the scene gives it
// id: Some("goblin"), which is the key — so it survives leaving the
// room and coming back. The meeting itself is announced in the shared
// global store for other scripts to react to.
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
    // Say it once per meeting; the wait bubble holds the rest.
    if recall("said") == true { return (); }
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
