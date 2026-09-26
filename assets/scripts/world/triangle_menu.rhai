// The triangle menu: what shows up when you press triangle in a PS1
// JRPG. Character info on the left, a cursor-driven option list on the
// right. Opens on triangle, closes on circle, freezes the field while
// up.
//
// The stats block is a placeholder until the world-state store exists
// (no HP/MP data and no party read API yet) — the menu mechanics are
// all real. Menu state lives in this script's own store: plain
// variables can't be assigned before they exist, but `remember` /
// `recall` persist across ticks.

fn on_update(dt) {
    if just_pressed("triangle") && recall("open") != true {
        remember("open", true);
        ui_pause(true);
    }
    if recall("open") != true {
        return;
    }

    let labels = ["Items", "Status", "Skills", "Bestiary", "Config"];

    ui_window("menu", 8.0, 8.0, 200.0, 110.0);
    ui_text("menu", "Hero", 8.0, 8.0);
    ui_text("menu", "HP", 8.0, 24.0);
    ui_bar("menu", 28.0, 26.0, 60.0, 0.9);
    ui_text("menu", "MP", 8.0, 40.0);
    ui_bar("menu", 28.0, 42.0, 40.0, 0.55);
    ui_options("menu", 110.0, 8.0, labels);

    let picked = ui_confirmed("menu");
    if picked >= 0 {
        ui_window("picked", 60.0, 128.0, 140.0, 22.0);
        ui_text("picked", labels[picked] + ": soon", 6.0, 5.0);
    }

    if just_pressed("circle") && recall("open") == true {
        remember("open", false);
        ui_pause(false);
        ui_close("menu");
        ui_close("picked");
    }
}
