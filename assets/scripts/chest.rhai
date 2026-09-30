// A treasure chest. Every chest in a scene uses this same script; what
// makes each one its own is the scene entry: a unique id (its private
// memory) and a params.item to announce. Pressing cross close to it
// opens it once — the lid swings via emote("open"), the contents are
// said aloud, and the opened flag lands in this chest's private store
// so it survives leaving and re-entering the scene.
//
// The trailing emote("open") is what keeps the lid open after a
// reload: the animation driver holds an emote that is requested again,
// even a finished one, so prop models never snap back to closed.
fn on_update(x, z, player_x, player_z, dt) {
    if recall("opened") != true {
        let dx = player_x - x;
        let dz = player_z - z;
        if dx*dx + dz*dz < 1.0 && just_pressed("cross") {
            remember("opened", true);
            emote("open");
            // Lower anchor than the goblin's: two wait-bubbles at one spot
            // would overlap into garble.
            say("Found " + param("item") + "!", #{wait: true, at: [160.0, 200.0]});
        }
        return ();
    }
    // The trailing pose("open") is what keeps the lid open after a
    // reload: it snaps straight to the clip's final frame — no swing —
    // so a chest you return to is simply already open.
    pose("open");
    ()
}
