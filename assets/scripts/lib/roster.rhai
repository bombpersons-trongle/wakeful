// The roster library: who is in the party, what their stats are, how
// they grow. This module owns the character-sheet data in the shared
// global store; everything else (menus, battle bags, the victory
// screen, the debug console) imports it — `import "lib/roster" as r;`
// — and calls into it. Everything is idempotent: bootstrapping, XP
// processing and stat refresh are all "make the store match the
// model" writes, so ticks are free once things are settled.
//
// Store keys this script owns:
//   jobs    — job tables: base stats, per-level growth, spell lists
//   items   — item definitions (equipment bonuses, consumable effects)
//   roster  — the sheet owners, in party order: [{id, name, job}]
//   sheets  — per id: {name, job, level, xp, hp, mp, weapon, armor,
//             str, quick, magic, def, mdef, max_hp, max_mp, spells}
//   xp_pending — battle-side mailbox: [{id, amount}]; consumed here
//
// The math (approved defaults): a stat is job base + per-level growth
// x (level - 1) + equipped bonuses; xp to next level is 15 + level x 10
// with the cap at 20. Current hp/mp never heal on level-up; they only
// clamp to the new maximum.

fn ensure_defs() {
    if recall_global("jobs") != () {
        return;
    }
    remember_global("jobs", #{
        fighter: #{
            base: #{str: 10.0, quick: 5.0, magic: 3.0, def: 8.0, mdef: 5.0,
                max_hp: 60.0, max_mp: 10.0},
            growth: #{str: 2.0, quick: 0.6, magic: 0.5, def: 1.6, mdef: 1.0,
                max_hp: 9.0, max_mp: 2.0},
            spells: [],
        },
        mage: #{
            base: #{str: 5.0, quick: 6.0, magic: 11.0, def: 4.0, mdef: 9.0,
                max_hp: 42.0, max_mp: 30.0},
            growth: #{str: 0.6, quick: 1.0, magic: 2.2, def: 0.6, mdef: 1.8,
                max_hp: 5.5, max_mp: 7.0},
            spells: [
                #{name: "fire", level: 1, power: 10.0, cost: 8.0},
                #{name: "ice", level: 3, power: 15.0, cost: 12.0},
                #{name: "bolt", level: 6, power: 22.0, cost: 18.0},
            ],
        },
    });
    remember_global("items", #{
        rusty_knife: #{name: "Rusty knife", slot: "weapon", str: 2.0},
        leather_vest: #{name: "Leather vest", slot: "armor", def: 1.5},
        potion: #{name: "Potion", heals: "hp", amount: 30.0},
        ether: #{name: "Ether", heals: "mp", amount: 15.0},
    });
}

fn ensure_roster() {
    if recall_global("roster") != () {
        return;
    }
    remember_global("roster", [
        #{id: "hero", name: "Hero", job: "fighter"},
        #{id: "ember", name: "Ember", job: "mage"},
    ]);
    remember_global("sheets", #{});
    remember_global("xp_pending", []);
}

fn ensure_sheets() {
    let roster = recall_global("roster");
    let sheets = recall_global("sheets");
    for member in roster {
        if sheets[member.id] == () {
            let job = job_of(member.job);
            sheets[member.id] = #{
                name: member.name,
                job: member.job,
                level: 1,
                xp: 0.0,
                hp: job.base.max_hp,
                mp: job.base.max_mp,
                weapon: "rusty_knife",
                armor: "leather_vest",
            };
        }
    }
    remember_global("sheets", sheets);
}

/// Derived stats are recomputed from the level and the equipment every
/// tick — leveling up changes nothing but the sheet's level and xp, so
/// there is no per-level bookkeeping to get wrong.
fn refresh_sheets() {
    let sheets = recall_global("sheets");
    let jobs = recall_global("jobs");
    let items = recall_global("items");
    for member in recall_global("roster") {
        let sheet = sheets[member.id];
        if sheet == () {
            continue;
        }
        let job = jobs[sheet.job];
        let levels = (sheet.level - 1) * 1.0;
        sheet.str = job.base.str + job.growth.str * levels + gear_bonus(items, sheet, "str");
        sheet.quick = job.base.quick + job.growth.quick * levels;
        sheet.magic = job.base.magic + job.growth.magic * levels + gear_bonus(items, sheet, "magic");
        sheet.def = job.base.def + job.growth.def * levels + gear_bonus(items, sheet, "def");
        sheet.mdef = job.base.mdef + job.growth.mdef * levels;
        sheet.max_hp = job.base.max_hp + job.growth.max_hp * levels;
        sheet.max_mp = job.base.max_mp + job.growth.max_mp * levels;
        // Current pools clamp to the maximum; leveling up does not
        // heal.
        if sheet.hp > sheet.max_hp {
            sheet.hp = sheet.max_hp;
        }
        if sheet.mp > sheet.max_mp {
            sheet.mp = sheet.max_mp;
        }
        // Spells the job grants at or below this level, in learn order.
        let known = [];
        for spell in job.spells {
            if sheet.level >= spell.level {
                known.push(spell.name);
            }
        }
        sheet.spells = known;
        // rhai map indexing hands out copies — write the sheet back or
        // every change above is lost.
        sheets[member.id] = sheet;
    }
    remember_global("sheets", sheets);
}

/// A piece of gear adds its bonus only while it occupies the matching
/// slot (wearing the vest as a weapon gains nothing).
fn gear_bonus(items, sheet, stat) {
    // Gear carries only the stats it boosts; indexing a missing stat
    // yields () and must not reach the arithmetic.
    let bonus = 0.0;
    if sheet.weapon != () && items[sheet.weapon] != ()
        && items[sheet.weapon].slot == "weapon"
        && items[sheet.weapon][stat] != () {
        bonus += items[sheet.weapon][stat];
    }
    if sheet.armor != () && items[sheet.armor] != ()
        && items[sheet.armor].slot == "armor"
        && items[sheet.armor][stat] != () {
        bonus += items[sheet.armor][stat];
    }
    bonus
}

/// The battle posts {id, amount} entries; each becomes xp on the
/// sheet, and levels follow the curve while there is xp left over.
/// Writes a battle's outcome back onto a sheet: the vitals the
/// fighter ended the fight with (clamped to the sheet's maxima), so
/// damage carries between battles. Unowned ids are ignored.
fn set_vitals(id, hp, mp) {
    let sheets = recall_global("sheets");
    if sheets == () || sheets[id] == () {
        return;
    }
    let sheet = sheets[id];
    sheet.hp = clamp(hp, 0.0, sheet.max_hp);
    if sheet.max_mp != () {
        sheet.mp = clamp(mp, 0.0, sheet.max_mp);
    }
    sheets[id] = sheet;
    remember_global("sheets", sheets);
}

/// A spell's definition (power, cost) by name, searched across the
/// job tables: spells are DATA, and every caster - menu, choreography
/// or debug console - looks them up the same way.
fn spell(name) {
    let jobs = recall_global("jobs");
    if jobs == () {
        return ();
    }
    for job_name in jobs.keys() {
        let job = jobs[job_name];
        if job.spells == () {
            continue;
        }
        for s in job.spells {
            if s.name == name {
                return s;
            }
        }
    }
    ()
}

/// Keeps a number inside [lo, hi].
fn clamp(value, lo, hi) {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

fn process_xp() {
    // Level cap. (Script-top-level consts never run when only a
    // function is called, so the cap lives here.)
    let cap = 20;
    let pending = recall_global("xp_pending");
    if pending == () || pending.len() == 0 {
        return;
    }
    let sheets = recall_global("sheets");
    for entry in pending {
        let sheet = sheets[entry.id];
        if sheet == () {
            continue;
        }
        sheet.xp += entry.amount;
        while sheet.level < cap {
            let need = 15.0 + sheet.level * 10.0;
            if sheet.xp < need {
                break;
            }
            sheet.xp -= need;
            sheet.level += 1;
        }
        if sheet.level >= cap {
            sheet.level = XP_CAP;
            sheet.xp = 0.0;
        }
        sheets[entry.id] = sheet;
    }
    remember_global("xp_pending", []);
    remember_global("sheets", sheets);
}

fn job_of(name) {
    recall_global("jobs")[name]
}

/// One roster tick: make the store match the model. The world script
/// (scripts/world/roster.rhai) is a thin scheduler over this.
fn tick_roster() {
    ensure_defs();
    ensure_roster();
    ensure_sheets();
    process_xp();
    refresh_sheets();
}

/// Grant xp to a sheet, following the curve. The battle's victory
/// flow and the debug console both go through here.
fn give_xp(id, amount) {
    let pending = recall_global("xp_pending");
    if pending == () {
        pending = [];
    }
    pending.push(#{id: id, amount: amount});
    remember_global("xp_pending", pending);
    tick_roster();
}

/// The sheet for `id`, if the roster knows it.
fn sheet_of(id) {
    let sheets = recall_global("sheets");
    if sheets == () {
        return ();
    }
    sheets[id]
}

/// A derived stat off a live sheet ("str", "max_hp", ...).
fn stat_of(id, stat) {
    let sheet = sheet_of(id);
    if sheet == () {
        return ();
    }
    sheet[stat]
}
