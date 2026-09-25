//! The script contracts: compiled Rhai files run at three tiers, each
//! with its own lifetime and entry points. Scripts never touch Bevy
//! directly — the engine hands each script the state it may react to as
//! arguments, and reads back a single value.
//!
//! - **World** scripts (`assets/scripts/world/*.rhai`) live for the
//!   whole game, independent of scenes: `on_update(dt)` every fixed
//!   tick. The future home of menus, saving, and quest logic.
//! - **Scene** scripts (a `script` path in the `.scene` file) live with
//!   their scene: `on_enter(player_x, player_z)` when it applies,
//!   `on_update(player_x, player_z, dt)` every fixed tick, `on_exit()`
//!   when it tears down. All three hooks are optional — a missing one
//!   is a no-op, so a cutscene may only use `on_enter`.
//! - **Actor** scripts (per scene actor) move and speak:
//!   `on_update(x, z, player_x, player_z, dt)` returns nothing to stay
//!   put, or `[new_x, new_z]` to move. `on_update` is required.
//!
//! Host functions are tier-scoped. Actors get `say(text)` and
//! `say(text, opts)` — collecting a line (with placement, timing, and
//! wait options) to show as a speech bubble — and `waiting()`, which
//! reports whether the actor's wait-mode bubble is still open. There is
//! no file or network access; scripts can only compute.
//!
//! Each runtime is called with a caller-owned [`Scope`] that persists
//! script state between calls.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use bevy::log::warn;
use bevy::prelude::Component;
use rhai::{Dynamic, Engine, Map, Position, Scope};

/// What an actor's `say` call produced: the line plus how it should be
/// shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    pub text: String,
    /// Free screen placement in virtual pixels, (0, 0) top-left;
    /// `None` anchors the bubble above the actor instead.
    pub at: Option<[f32; 2]>,
    /// Direction the tail points. The default is down for anchored
    /// bubbles and no tail for free-placed ones.
    pub tail: Option<[f32; 2]>,
    /// Never draw the tail.
    pub no_tail: bool,
    /// Stay open until the player presses confirm.
    pub wait: bool,
    /// Auto-dismiss after this many seconds.
    pub ttl: Option<f64>,
}

/// What one actor script update produced: where the actor goes and
/// what it said, in call order.
pub struct Tick {
    /// `None` to stay put, or the new ground position.
    pub position: Option<[f32; 2]>,
    /// Lines the script `say`-ed this update.
    pub said: Vec<Said>,
}

/// Marks a script runtime that errored: the runtime is skipped from
/// then on, so a broken file can't spam warnings every tick.
#[derive(Component)]
pub struct ScriptBroken;

/// What a script runtime call reports.
type ScriptError = Box<rhai::EvalAltResult>;

/// A compiled Rhai script: an engine carrying the tier's host
/// functions plus the AST, ready to call entry points against a
/// caller-owned scope.
struct CompiledScript {
    engine: Engine,
    ast: rhai::AST,
}

impl CompiledScript {
    /// Compiles script text on an engine configured by `register`.
    fn compile(text: &str, register: impl FnOnce(&mut Engine)) -> Result<Self, rhai::ParseError> {
        let mut engine = Engine::new();
        register(&mut engine);
        let ast = engine.compile(text)?;
        Ok(Self { engine, ast })
    }

    /// Calls an entry point; whatever it returns comes back as a
    /// `Dynamic`.
    fn call(
        &self,
        scope: &mut Scope,
        name: &str,
        args: impl rhai::FuncArgs,
    ) -> Result<Dynamic, ScriptError> {
        self.engine.call_fn(scope, &self.ast, name, args)
    }

    /// Whether the script defines the function.
    fn defines(&self, name: &str) -> bool {
        self.ast.iter_functions().any(|f| f.name == name)
    }
}

/// Reads and compiles a tier script file: `None` (after a warning) when
/// unreadable or non-compiling, so content runs without the script.
pub(crate) fn compile_script_file<T>(
    path: &Path,
    tier: &str,
    compile: fn(&str) -> Result<T, rhai::ParseError>,
) -> Option<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            warn!(
                "{tier} script {} could not be read, content runs without it: {e}",
                path.display()
            );
            return None;
        }
    };
    match compile(&text) {
        Ok(script) => Some(script),
        Err(e) => {
            warn!(
                "{tier} script {} failed to compile, content runs without it: {e}",
                path.display()
            );
            None
        }
    }
}

/// Loads a script from the assets folder by its relative path.
fn load_script_file<T>(
    path: &str,
    tier: &str,
    compile: fn(&str) -> Result<T, rhai::ParseError>,
) -> Option<T> {
    compile_script_file(&crate::editor::assets_root().join(path), tier, compile)
}

/// The actor tier's runtime: compiled script plus the `say`/`waiting`
/// bridges to the host.
pub struct ActorScript {
    script: CompiledScript,
    /// `say` output appends here; drained once per update.
    said: Arc<Mutex<Vec<Said>>>,
    /// Mirrored in by the host each update; read by `waiting()`.
    waiting: Arc<Mutex<bool>>,
}

impl ActorScript {
    /// Compiles an actor script.
    pub fn compile(text: &str) -> Result<Self, rhai::ParseError> {
        let said = Arc::new(Mutex::new(Vec::new()));
        let waiting = Arc::new(Mutex::new(false));
        let script = CompiledScript::compile(text, |engine| {
            let sink = said.clone();
            engine.register_fn("say", move |line: &str| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Said {
                        text: line.to_owned(),
                        ..Said::default()
                    });
            });
            let sink = said.clone();
            engine.register_fn(
                "say",
                move |line: &str, opts: Map| -> Result<(), Box<rhai::EvalAltResult>> {
                    let said = parse_said(line, &opts).map_err(runtime_error)?;
                    sink.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(said);
                    Ok(())
                },
            );
            let flag = waiting.clone();
            engine.register_fn("waiting", move || {
                *flag.lock().unwrap_or_else(PoisonError::into_inner)
            });
        })?;
        Ok(Self {
            script,
            said,
            waiting,
        })
    }

    /// Loads an actor script from the assets folder: `None` (after a
    /// warning) when unreadable or non-compiling, so the actor runs
    /// without it.
    pub fn load(path: &str) -> Option<Self> {
        load_script_file(path, "Actor", Self::compile)
    }

    /// Tells the script whether the actor's wait-mode bubble is still
    /// open; read back through `waiting()` during the next update.
    pub fn set_waiting(&self, waiting: bool) {
        *self.waiting.lock().unwrap_or_else(PoisonError::into_inner) = waiting;
    }

    /// Runs one `on_update`. The position is `None` when the script (or
    /// its missing `on_update`) wants the actor to stay put;
    /// `Some([x, z])` is the new ground position. `said` carries
    /// whatever the script `say`-ed this update.
    pub fn update(
        &self,
        scope: &mut Scope,
        x: f32,
        z: f32,
        player_x: f32,
        player_z: f32,
        dt: f32,
    ) -> Result<Tick, ScriptError> {
        // say() output belongs to the update that calls it; clear any
        // residue from a previous update that errored mid-drain.
        self.said
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        // rhai computes in f64; pass doubles in and read the pair out.
        let result = self.script.call(
            scope,
            "on_update",
            (
                x as f64,
                z as f64,
                player_x as f64,
                player_z as f64,
                dt as f64,
            ),
        )?;
        let position = convert_position(result)?;
        let said = std::mem::take(
            self.said
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_mut(),
        );
        Ok(Tick { position, said })
    }
}

/// The scene tier's runtime: lifecycle hooks over one scene's lifetime.
pub struct SceneScript {
    script: CompiledScript,
}

impl SceneScript {
    /// Compiles a scene script.
    pub fn compile(text: &str) -> Result<Self, rhai::ParseError> {
        Ok(Self {
            script: CompiledScript::compile(text, |_| ())?,
        })
    }

    /// Loads a scene script from the assets folder: `None` (after a
    /// warning) when unreadable or non-compiling, so the scene runs
    /// without it.
    pub fn load(path: &str) -> Option<Self> {
        load_script_file(path, "Scene", Self::compile)
    }

    /// Runs `on_enter(player_x, player_z)` once per scene application.
    pub fn enter(
        &self,
        scope: &mut Scope,
        player_x: f32,
        player_z: f32,
    ) -> Result<(), ScriptError> {
        self.call_optional(scope, "on_enter", (player_x as f64, player_z as f64))
    }

    /// Runs `on_update(player_x, player_z, dt)` for one fixed tick.
    pub fn update(
        &self,
        scope: &mut Scope,
        player_x: f32,
        player_z: f32,
        dt: f32,
    ) -> Result<(), ScriptError> {
        self.call_optional(
            scope,
            "on_update",
            (player_x as f64, player_z as f64, dt as f64),
        )
    }

    /// Runs `on_exit()` as the scene is torn down.
    pub fn exit(&self, scope: &mut Scope) -> Result<(), ScriptError> {
        self.call_optional(scope, "on_exit", ())
    }

    /// Calls an optional entry point: an undefined function is a no-op,
    /// a defined one that errors is an error.
    fn call_optional(
        &self,
        scope: &mut Scope,
        name: &str,
        args: impl rhai::FuncArgs,
    ) -> Result<(), ScriptError> {
        if !self.script.defines(name) {
            return Ok(());
        }
        self.script.call(scope, name, args).map(|_| ())
    }
}

/// The world tier's runtime: one per-tick hook over the whole game.
pub struct WorldScript {
    script: CompiledScript,
}

impl WorldScript {
    /// Compiles a world script.
    pub fn compile(text: &str) -> Result<Self, rhai::ParseError> {
        Ok(Self {
            script: CompiledScript::compile(text, |_| ())?,
        })
    }

    /// Runs `on_update(dt)` for one fixed tick. Unlike the scene
    /// tier's hooks, `on_update` is required: it is the tier's whole
    /// contract, and its absence is an error that disables the script.
    pub fn update(&self, scope: &mut Scope, dt: f32) -> Result<(), ScriptError> {
        self.script
            .call(scope, "on_update", (dt as f64,))
            .map(|_| ())
    }
}

fn runtime_error(message: String) -> Box<rhai::EvalAltResult> {
    Box::new(rhai::EvalAltResult::ErrorRuntime(
        message.into(),
        Position::NONE,
    ))
}

/// Reads `say`'s options map. Strict: unknown keys and ill-typed values
/// are runtime errors, so script typos surface at the call site.
fn parse_said(line: &str, opts: &Map) -> Result<Said, String> {
    let mut said = Said {
        text: line.to_owned(),
        ..Said::default()
    };
    for (key, value) in opts {
        match key.as_str() {
            "at" => said.at = Some(pair(value, "at")?),
            "tail" => said.tail = Some(pair(value, "tail")?),
            "no_tail" => said.no_tail = bool_of(value, "no_tail")?,
            "wait" => said.wait = bool_of(value, "wait")?,
            "ttl" => said.ttl = Some(seconds(value, "ttl")?),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if said.tail.is_some() && said.no_tail {
        return Err("'tail' and 'no_tail' cannot be combined".into());
    }
    Ok(said)
}

fn pair(value: &Dynamic, key: &str) -> Result<[f32; 2], String> {
    let array = value
        .clone()
        .into_array()
        .map_err(|_| format!("'{key}' must be an [x, y] array"))?;
    if array.len() != 2 {
        return Err(format!("'{key}' must be an [x, y] array"));
    }
    Ok([number(&array[0], key)?, number(&array[1], key)?])
}

fn number(value: &Dynamic, key: &str) -> Result<f32, String> {
    value
        .as_float()
        .map(|f| f as f32)
        .or_else(|_| value.as_int().map(|i| i as f32))
        .map_err(|_| format!("'{key}' must contain numbers"))
}

fn bool_of(value: &Dynamic, key: &str) -> Result<bool, String> {
    value
        .as_bool()
        .map_err(|_| format!("'{key}' must be a bool"))
}

fn seconds(value: &Dynamic, key: &str) -> Result<f64, String> {
    value
        .as_float()
        .or_else(|_| value.as_int().map(|i| i as f64))
        .map_err(|_| format!("'{key}' must be a number"))
}

/// Reads the actor script's return value: a unit stays put, a
/// two-element array is the new position, anything else is a contract
/// violation.
fn convert_position(result: Dynamic) -> Result<Option<[f32; 2]>, ScriptError> {
    if result.is_unit() {
        return Ok(None);
    }
    if !result.is_array() {
        return Err(Box::new(rhai::EvalAltResult::ErrorMismatchDataType(
            "[] (two-element position)".into(),
            result.type_name().into(),
            Position::NONE,
        )));
    }
    let array = result.into_array()?;
    if array.len() != 2 {
        return Err(Box::new(rhai::EvalAltResult::ErrorMismatchDataType(
            "[] (two-element position)".into(),
            "array of other length".into(),
            Position::NONE,
        )));
    }
    let mut coords = [0.0f32; 2];
    for (coord, value) in coords.iter_mut().zip(array) {
        *coord = value.as_float()? as f32;
    }
    Ok(Some(coords))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_a_pair_moves_the_actor() {
        let script = ActorScript::compile(
            r"
            fn on_update(x, z, player_x, player_z, dt) {
                [x + dt * 2.0, z]
            }
            ",
        )
        .unwrap();
        let mut scope = Scope::new();
        let moved = script.update(&mut scope, 1.0, 2.0, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(moved.position, Some([2.0, 2.0]));
    }

    #[test]
    fn returning_nothing_stays_put() {
        let script =
            ActorScript::compile("fn on_update(x, z, player_x, player_z, dt) { }").unwrap();
        let mut scope = Scope::new();
        assert_eq!(
            script
                .update(&mut scope, 1.0, 2.0, 0.0, 0.0, 0.5)
                .unwrap()
                .position,
            None
        );
    }

    #[test]
    fn a_script_can_say_and_the_host_reads_it_back() {
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                say("hello");
                say("there");
            }
            "#,
        )
        .unwrap();
        let mut scope = Scope::new();
        let tick = script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(tick.said[0].text, "hello");
        assert_eq!(tick.said[1].text, "there");
    }

    #[test]
    fn said_is_per_update_so_silence_reads_as_empty() {
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                if spoke < 1 { say("once"); spoke = 1; }
            }
            "#,
        )
        .unwrap();
        let mut scope = Scope::new();
        scope.push("spoke", 0_i64);
        let first = script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        let second = script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(first.said[0].text, "once");
        assert!(second.said.is_empty());
    }

    #[test]
    fn say_options_round_trip() {
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                say("over there", #{at: [10.0, 20.0], tail: [-1.0, 0.5], ttl: 2});
            }
            "#,
        )
        .unwrap();
        let said = script
            .update(&mut Scope::new(), 0.0, 0.0, 0.0, 0.0, 0.5)
            .unwrap()
            .said
            .remove(0);
        assert_eq!(said.text, "over there");
        assert_eq!(said.at, Some([10.0, 20.0]));
        assert_eq!(said.tail, Some([-1.0, 0.5]));
        assert_eq!(said.ttl, Some(2.0));
        assert!(!said.wait);
        assert!(!said.no_tail);
    }

    #[test]
    fn a_wait_say_defaults_to_tying_up_the_script() {
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                say("press Z", #{wait: true, no_tail: true});
            }
            "#,
        )
        .unwrap();
        let said = script
            .update(&mut Scope::new(), 0.0, 0.0, 0.0, 0.0, 0.5)
            .unwrap()
            .said
            .remove(0);
        assert!(said.wait);
        assert!(said.no_tail);
        assert_eq!(said.ttl, None);
        assert_eq!(said.at, None);
    }

    #[test]
    fn an_unknown_say_option_is_a_runtime_error() {
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                say("hi", #{colour: "red"});
            }
            "#,
        )
        .unwrap();
        assert!(
            script
                .update(&mut Scope::new(), 0.0, 0.0, 0.0, 0.0, 0.5)
                .is_err()
        );
    }

    #[test]
    fn tail_and_no_tail_conflict() {
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                say("hi", #{tail: [0.0, -1.0], no_tail: true});
            }
            "#,
        )
        .unwrap();
        assert!(
            script
                .update(&mut Scope::new(), 0.0, 0.0, 0.0, 0.0, 0.5)
                .is_err()
        );
    }

    #[test]
    fn waiting_mirrors_what_the_host_sets() {
        let script = ActorScript::compile(
            r"
            fn on_update(x, z, player_x, player_z, dt) {
                if waiting() { saw_wait = true; }
            }
            ",
        )
        .unwrap();
        let mut scope = Scope::new();
        scope.push("saw_wait", false);
        script.set_waiting(true);
        script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(scope.get_value::<bool>("saw_wait"), Some(true));
        script.set_waiting(false);
        script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        // Still true: the script only writes it while waiting.
        assert_eq!(scope.get_value::<bool>("saw_wait"), Some(true));
    }

    #[test]
    fn a_missing_update_is_a_contract_violation() {
        let script = ActorScript::compile("fn helper() { 42 }").unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn a_non_array_return_is_rejected() {
        let script =
            ActorScript::compile("fn on_update(x, z, player_x, player_z, dt) { 7 }").unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn a_short_array_return_is_rejected() {
        let script =
            ActorScript::compile("fn on_update(x, z, player_x, player_z, dt) { [x] }").unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn scope_state_persists_between_calls() {
        let script = ActorScript::compile(
            r"
            fn on_update(x, z, player_x, player_z, dt) {
                if visits < 1 { visits = 0; }
                visits += 1;
                [x + visits, z]
            }
            ",
        )
        .unwrap();
        let mut scope = Scope::new();
        scope.push("visits", 0_i64);
        let first = script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        let second = script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(first.position, Some([1.0, 0.0]));
        assert_eq!(second.position, Some([2.0, 0.0]));
    }

    #[test]
    fn compile_errors_surface() {
        assert!(ActorScript::compile("fn broken {").is_err());
    }

    #[test]
    fn scripts_cannot_reach_the_filesystem() {
        // The default engine exposes no file functions; a script trying
        // to call one must fail, not touch the disk.
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, player_x, player_z, dt) {
                let f = open_file("/etc/passwd", false);
                [x, z]
            }
            "#,
        )
        .unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn scene_hooks_receive_their_arguments() {
        let script = SceneScript::compile(
            r"
            fn on_enter(px, pz) { entered_px = px; entered_pz = pz; }
            fn on_update(px, pz, dt) { ticked_px = px; ticked_dt = dt; }
            fn on_exit() { exited = true; }
            ",
        )
        .unwrap();
        let mut scope = Scope::new();
        scope.push("entered_px", 0.0_f64);
        scope.push("entered_pz", 0.0_f64);
        scope.push("ticked_px", 0.0_f64);
        scope.push("ticked_dt", 0.0_f64);
        scope.push("exited", false);
        script.enter(&mut scope, 3.0, 4.0).unwrap();
        script.update(&mut scope, 3.0, 4.0, 0.5).unwrap();
        script.exit(&mut scope).unwrap();
        assert_eq!(scope.get_value::<f64>("entered_px"), Some(3.0));
        assert_eq!(scope.get_value::<f64>("entered_pz"), Some(4.0));
        assert_eq!(scope.get_value::<f64>("ticked_px"), Some(3.0));
        assert_eq!(scope.get_value::<f64>("ticked_dt"), Some(0.5));
        assert_eq!(scope.get_value::<bool>("exited"), Some(true));
    }

    #[test]
    fn missing_scene_hooks_are_no_ops() {
        // Only on_update defined: enter and exit succeed silently.
        let script = SceneScript::compile("fn on_update(px, pz, dt) { }").unwrap();
        let mut scope = Scope::new();
        script.enter(&mut scope, 0.0, 0.0).unwrap();
        script.update(&mut scope, 0.0, 0.0, 0.5).unwrap();
        script.exit(&mut scope).unwrap();
        // And only on_enter defined: update succeeds silently.
        let script = SceneScript::compile("fn on_enter(px, pz) { }").unwrap();
        script.update(&mut Scope::new(), 0.0, 0.0, 0.5).unwrap();
    }

    #[test]
    fn a_scene_hook_that_errors_is_an_error() {
        let script = SceneScript::compile("fn on_enter(px, pz) { bogus_fn(); }").unwrap();
        assert!(script.enter(&mut Scope::new(), 0.0, 0.0).is_err());
    }

    #[test]
    fn world_update_receives_dt_and_state_persists() {
        let script = WorldScript::compile("fn on_update(dt) { total += dt; }").unwrap();
        let mut scope = Scope::new();
        scope.push("total", 0.0_f64);
        script.update(&mut scope, 0.5).unwrap();
        script.update(&mut scope, 0.25).unwrap();
        assert_eq!(scope.get_value::<f64>("total"), Some(0.75));
    }

    #[test]
    fn a_missing_world_update_is_a_contract_violation() {
        let script = WorldScript::compile("fn helper() { 42 }").unwrap();
        assert!(script.update(&mut Scope::new(), 0.5).is_err());
    }

    #[test]
    fn load_reads_a_shipped_script_and_missing_paths_warn_to_none() {
        assert!(ActorScript::load("scripts/test.rhai").is_some());
        assert!(ActorScript::load("scripts/does-not-exist.rhai").is_none());
    }
}
