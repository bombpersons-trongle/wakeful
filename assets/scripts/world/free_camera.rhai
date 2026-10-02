// The free-camera: a flight mode for checking a scene (or a battle) from
// any angle — the painted background becomes a depth card today, and
// flying around it is the way to see the card's actual geometry. Toggle
// with select; while it is on the d-pad looks (left/right yaw, up/down
// pitch) and cross/circle fly forward/back. Off by default, so the
// battle menu owns the same buttons normally.
//
// The moves apply to whichever 3D camera is active — one script covers
// the scene and the battle flight alike.
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
        camera_move(yaw, pitch, dolly);
    }
    ()
}
