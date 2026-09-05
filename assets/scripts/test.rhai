// Test script for the devroom goblin: trails the player, stopping at a
// polite distance, and calls out while the player is close. Returns
// nothing to stay put, [x, z] to move.
fn on_update(x, z, player_x, player_z, dt) {
    let dx = player_x - x;
    let dz = player_z - z;
    let dist = sqrt(dx*dx + dz*dz);
    if dist < 2.0 {
        say("Over here! Where are we?");
        return ();
    }
    let speed = 1.5;
    let step = min(speed * dt, dist - 2.0);
    [x + dx / dist * step, z + dz / dist * step]
}
