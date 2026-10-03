// Floating damage numbers, as a library: drop an entry in the battle
// store and the battle UI world script projects and animates it. Each
// entry gets a unique id (its label's name) and a color naming a label
// color in the HUD ("white" damage, "green" healing).
fn float_text(foe, text, color) {
    let next = battle_get("float_id");
    let next = if next == () { 0 } else { next + 1 };
    battle_set("float_id", next);
    let floats = battle_get("floats");
    if floats == () {
        floats = [];
    }
    floats.push(#{
        id: next,
        text: text,
        x: foe.x,
        y: foe.y + 1.4,
        z: foe.z,
        age: 0.0,
        color: color,
    });
    battle_set("floats", floats);
}
