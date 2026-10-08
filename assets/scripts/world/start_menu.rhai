// The boot screen: the title, New Game, and Continue when a save
// exists. The script owns nothing else - `start_game` hands the engine
// over (fresh boot, or the saved store/scene/position).
fn on_update(dt) {
    if game_state() != "StartMenu" {
        return;
    }
    ui_window("start-menu", 40.0, 24.0, 240.0, 192.0);
    // The title is a bitmap: the game renders text at one pixel size,
    // so big type ships as an image.
    ui_image("start-menu", "ui/wakeful_title.png", 21.0, 44.0, 198.0, 34.0);
    let options = ["New Game"];
    if save_exists() {
        options.push("Continue");
    }
    ui_options("start-menu", 12.0, 100.0, options);
    let picked = ui_confirmed("start-menu");
    if picked == 0 {
        start_game(true);
    }
    if picked == 1 {
        start_game(false);
    }
}

// Windows live until something closes them, and the guard above stops
// re-declaring the moment the state flips - so the menu cleans up
// after itself when the game leaves StartMenu.
fn on_state_change(from, to) {
    if from == "StartMenu" {
        ui_close("start-menu");
    }
}