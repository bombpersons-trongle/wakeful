// The battle free-camera: a flight mode for checking the arena and the
// fighters from any angle. Tap select to toggle it; while it is on the
// d-pad looks (left/right yaw, up/down pitch) and cross/circle fly
// forward/back. Off by default, so the battle menu owns the same
// buttons normally.
//
// Deltas go to the engine as a request each tick; the sequencer
// republishes the camera snapshot from the moved transform, so script
// projections (floating damage) stay truthful while flying.
//
// (Speeds live inside on_update: rhai functions do not see top-level
// constants.)

fn on_update(dt) {
    let turn = 1.5; // radians per second
    let fly = 5.0; // world units per second
    let on = recall("free_cam") == true;
    if just_pressed("select") {
        on = !on;
        remember("free_cam", on);
    }
    if !on {
        return ();
    }
    let yaw = 0.0;
    let pitch = 0.0;
    let dolly = 0.0;
    if pressed("dpad_left") {
        yaw += turn * dt;
    }
    if pressed("dpad_right") {
        yaw -= turn * dt;
    }
    if pressed("dpad_up") {
        pitch += turn * dt;
    }
    if pressed("dpad_down") {
        pitch -= turn * dt;
    }
    if pressed("cross") {
        dolly += fly * dt;
    }
    if pressed("circle") {
        dolly -= fly * dt;
    }
    if yaw != 0.0 || pitch != 0.0 || dolly != 0.0 {
        battle_camera_move(yaw, pitch, dolly);
    }
    ()
}
