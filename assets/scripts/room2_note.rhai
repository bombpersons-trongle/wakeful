// The room2 note: a bubble-styled panel that waits for cross. A
// deliberately simple dismissal probe — if this closes but the
// goblin's say-bubble doesn't, the engine-side dismissal path is the
// suspect; if neither closes, the cross edge never reaches anything.

fn on_enter(px, pz) {
    remember("up", true);
}

fn on_update(px, pz, dt) {
    if recall("up") != true {
        return ();
    }
    if just_pressed("cross") {
        remember("up", false);
        ui_close("note");
        print("note dismissed");
        return ();
    }
    ui_window("note", 60.0, 40.0, 220.0, 48.0);
    ui_text("note", "Press cross to dismiss.", 8.0, 18.0);
    ()
}
