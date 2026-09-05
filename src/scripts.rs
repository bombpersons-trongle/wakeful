//! The actor script contract: compiling Rhai files and running their
//! per-tick update.
//!
//! Scripts never touch Bevy directly — the engine hands each script the
//! state it may react to as arguments, and reads back a single value:
//!
//! ```rhai
//! fn on_update(x, z, player_x, player_z, dt) {
//!     // return nothing to stay put, or [new_x, new_z] to move
//! }
//! ```
//!
//! World state goes in as arguments, so the sandbox has exactly one
//! host function: `say(text)`, which collects speech and is returned to
//! the host in the tick result (to show as a speech bubble). There is
//! no file or network access; scripts can only compute.
//!
//! A per-actor `Scope` (owned by the caller) persists script state
//! between ticks.

use std::sync::{Arc, Mutex, PoisonError};

use rhai::{Dynamic, Engine, Position, Scope};

/// What one script tick produced: where the actor goes and what it
/// said. Speech is drained from the script's `say` buffer, so it is
/// per-tick and in call order.
pub struct Tick {
    /// `None` to stay put, or the new ground position.
    pub position: Option<[f32; 2]>,
    /// Text the script `say`-ed this tick.
    pub said: Vec<String>,
}

/// A compiled Rhai script, ready to run against a caller-owned [`Scope`].
pub struct CompiledScript {
    engine: Engine,
    ast: rhai::AST,
    /// `say(text)` appends here; drained once per tick.
    said: Arc<Mutex<Vec<String>>>,
}

impl CompiledScript {
    /// Compiles script text.
    pub fn compile(text: &str) -> Result<Self, rhai::ParseError> {
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new();
        let sink = said.clone();
        engine.register_fn("say", move |line: &str| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(line.to_owned());
        });
        let ast = engine.compile(text)?;
        Ok(Self { engine, ast, said })
    }

    /// Runs one update tick. The position is `None` when the script (or
    /// its missing `on_update`) wants the actor to stay put; `Some([x,
    /// z])` is the new ground position. `said` carries whatever the
    /// script `say`-ed this tick.
    pub fn update(
        &self,
        scope: &mut Scope,
        x: f32,
        z: f32,
        player_x: f32,
        player_z: f32,
        dt: f32,
    ) -> Result<Tick, Box<rhai::EvalAltResult>> {
        // say() output belongs to the tick that calls it; clear any
        // residue from a previous tick that errored mid-drain.
        self.said
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        // rhai computes in f64; pass doubles in and read the pair out.
        let result: Dynamic = self.engine.call_fn(
            scope,
            &self.ast,
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

/// Reads the script's return value: a unit stays put, a two-element
/// array is the new position, anything else is a contract violation.
fn convert_position(result: Dynamic) -> Result<Option<[f32; 2]>, Box<rhai::EvalAltResult>> {
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
        let script = CompiledScript::compile(
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
            CompiledScript::compile("fn on_update(x, z, player_x, player_z, dt) { }").unwrap();
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
        let script = CompiledScript::compile(
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
        assert_eq!(tick.said, ["hello".to_owned(), "there".to_owned()]);
    }

    #[test]
    fn said_is_per_tick_so_silence_reads_as_empty() {
        let script = CompiledScript::compile(
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
        assert_eq!(first.said, ["once".to_owned()]);
        assert!(second.said.is_empty());
    }

    #[test]
    fn a_missing_update_is_a_contract_violation() {
        let script = CompiledScript::compile("fn helper() { 42 }").unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn a_non_array_return_is_rejected() {
        let script =
            CompiledScript::compile("fn on_update(x, z, player_x, player_z, dt) { 7 }").unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn a_short_array_return_is_rejected() {
        let script =
            CompiledScript::compile("fn on_update(x, z, player_x, player_z, dt) { [x] }").unwrap();
        let mut scope = Scope::new();
        assert!(script.update(&mut scope, 0.0, 0.0, 0.0, 0.0, 0.5).is_err());
    }

    #[test]
    fn scope_state_persists_between_calls() {
        let script = CompiledScript::compile(
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
        assert!(CompiledScript::compile("fn broken {").is_err());
    }

    #[test]
    fn scripts_cannot_reach_the_filesystem() {
        // The default engine exposes no file functions; a script trying
        // to call one must fail, not touch the disk.
        let script = CompiledScript::compile(
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
}
