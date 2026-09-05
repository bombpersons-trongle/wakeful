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
//! World state goes in as arguments, so the sandbox needs no host
//! functions; a per-actor `Scope` (owned by the caller) persists script
//! state between ticks. The default engine has no file or network
//! access, so scripts can only compute.

use rhai::{Dynamic, Engine, Position, Scope};

/// A compiled Rhai script, ready to run against a caller-owned [`Scope`].
pub struct CompiledScript {
    engine: Engine,
    ast: rhai::AST,
}

impl CompiledScript {
    /// Compiles script text.
    pub fn compile(text: &str) -> Result<Self, rhai::ParseError> {
        let engine = Engine::new();
        let ast = engine.compile(text)?;
        Ok(Self { engine, ast })
    }

    /// Runs one update tick. `Ok(None)` means the script (or its missing
    /// `on_update`) wants the actor to stay put; `Ok(Some([x, z]))` is
    /// the new ground position.
    pub fn update(
        &self,
        scope: &mut Scope,
        x: f32,
        z: f32,
        player_x: f32,
        player_z: f32,
        dt: f32,
    ) -> Result<Option<[f32; 2]>, Box<rhai::EvalAltResult>> {
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
        convert_position(result)
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
        assert_eq!(moved, Some([2.0, 2.0]));
    }

    #[test]
    fn returning_nothing_stays_put() {
        let script =
            CompiledScript::compile("fn on_update(x, z, player_x, player_z, dt) { }").unwrap();
        let mut scope = Scope::new();
        assert_eq!(
            script.update(&mut scope, 1.0, 2.0, 0.0, 0.0, 0.5).unwrap(),
            None
        );
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
        assert_eq!(first, Some([1.0, 0.0]));
        assert_eq!(second, Some([2.0, 0.0]));
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
