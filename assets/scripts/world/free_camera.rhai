// The free-camera: a flight mode for checking a scene (or a battle) from
// any angle — the painted background becomes a depth card today, and
// flying around it is the way to see the card's actual geometry. Toggle
// with select; while it is on the d-pad looks (left/right yaw, up/down
// pitch) and cross/circle fly forward/back.
//
// Every button the mode uses is captured per tick, so gameplay, menus,
// bubbles, and other scripts never feel a ghost press while it flies —
// and a captured button read through `pressed` says false, which is why
// the flight reads its buttons through `capture` instead.
//
// (Speeds live inside on_update: rhai functions do not see top-level
// constants.)

fn on_update(dt) {
    let turn = 1.5; // radians per second
    let fly = 5.0; // world units per second
    let on = recall("free_cam") == true;
    if capture_just_pressed("select") {
        on = !on;
        remember("free_cam", on);
    }
    let yaw = 0.0;
    let pitch = 0.0;
    let dolly = 0.0;
    if on {
        if capture("dpad_left") {
            yaw += turn * dt;
        }
        if capture("dpad_right") {
            yaw -= turn * dt;
        }
        if capture("dpad_up") {
            pitch += turn * dt;
        }
        if capture("dpad_down") {
            pitch -= turn * dt;
        }
        if capture("cross") {
            dolly += fly * dt;
        }
        if capture("circle") {
            dolly -= fly * dt;
        }
    }
    if yaw != 0.0 || pitch != 0.0 || dolly != 0.0 {
        camera_move(yaw, pitch, dolly);
    }
    ()
}
