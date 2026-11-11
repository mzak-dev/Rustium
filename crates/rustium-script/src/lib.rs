//! Sandboxed Luau scripting for the Rustium voxel editor.
//!
//! [`run`] executes one script against one Model of a [`Scene`]; positions are in the Model's own cells. The
//! script sees a small API:
//!
//! | Luau | Meaning |
//! |---|---|
//! | `scene.get(x, y, z)` | Material number (1–255) of a voxel, or `nil` when empty |
//! | `scene.set(x, y, z, material)` | Places a voxel; `nil` or `0` erases |
//! | `scene.erase(x, y, z)` | Removes a voxel |
//! | `scene.fill(x1, y1, z1, x2, y2, z2, material)` | Sets a whole box |
//! | `palette.len()` | Number of Materials |
//! | `palette.color(material)` | `r, g, b` of a Material |
//! | `palette.nearest(r, g, b)` | Material closest to a color |
//! | `print(...)` | Appends a line to [`Output::log`] |
//!
//! Writes are buffered and applied at the end as a single [`Command::SetVoxels`], so a script that fails
//! leaves the Scene untouched and a script that succeeds is exactly one [`Edit`] (labelled
//! [`EditKind::Script`]). The VM has no `io`, `os`, `package` or `debug` library, runs in Luau's sandbox, and
//! is bounded by a memory limit, an interrupt (loop iteration and call) budget, a wall-clock limit and a
//! cap on written voxels; see [`Limits`].
//!
//! ```
//! use rustium_core::{History, NodeId, Scene};
//! use rustium_script::{Limits, run};
//!
//! let mut scene = Scene::new();
//! let model = scene
//!     .node(NodeId::ROOT)
//!     .expect("the root exists")
//!     .children()[0];
//! let mut history = History::new(10);
//! let source = "for x = 0, 9 do scene.set(x, 0, 0, 5) end";
//! let output = run(&mut scene, model, source, &Limits::default())?;
//! history.push(output.edit.expect("the script placed voxels"));
//! let at = glam::IVec3::new(9, 0, 0);
//! assert!(
//!     scene
//!         .model_shape(model)
//!         .expect("a model")
//!         .1
//!         .get(at)
//!         .is_some()
//! );
//! history.undo(&mut scene);
//! assert!(
//!     scene
//!         .model_shape(model)
//!         .expect("a model")
//!         .1
//!         .get(at)
//!         .is_none()
//! );
//! # Ok::<(), rustium_script::Error>(())
//! ```
#![warn(clippy::panic, clippy::expect_used)]
#![expect(
    clippy::cast_possible_truncation,
    reason = "script numbers are range-checked against COORD_LIMIT before the cast to i32"
)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use glam::IVec3;
use mlua::{Lua, LuaOptions, MultiValue, StdLib, Value, VmState};
use rustium_core::{COORD_LIMIT, Command, Edit, EditKind, MaterialId, NodeId, Scene, Voxel, apply};

/// Why a script did not run to completion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The script has a syntax error, raised an error, or hit a resource limit.
    #[error("{0}")]
    Script(String),
    /// The Scene rejected the voxels the script wrote.
    #[error("the scene rejected the script's changes: {0}")]
    Scene(#[from] rustium_core::Error),
    /// The Luau VM could not be set up.
    #[error("cannot start the script engine: {0}")]
    Engine(String),
}

/// Resource limits of one script run.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Memory the VM may allocate, in bytes.
    pub memory_bytes: usize,
    /// Interrupt budget: Luau checks in at every function call and loop iteration.
    pub interrupts: u64,
    /// Wall-clock time the script may run.
    pub time: Duration,
    /// Distinct voxel positions the script may write.
    pub voxels: usize,
    /// Lines `print` may add to the log; later ones are dropped.
    pub log_lines: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            interrupts: 20_000_000,
            time: Duration::from_secs(2),
            voxels: 1_000_000,
            log_lines: 200,
        }
    }
}

/// What a finished script produced.
#[derive(Debug)]
pub struct Output {
    /// The change the script made, already applied to the Scene; `None` when it changed nothing. Push it to
    /// the History as one step.
    pub edit: Option<Edit>,
    /// Lines the script printed.
    pub log: Vec<String>,
}

/// Writes a script made, merged into one batch at the end of the run.
#[derive(Default)]
struct Pending {
    voxels: HashMap<IVec3, Voxel>,
    log: Vec<String>,
}

/// Longest line `print` keeps, in bytes.
const MAX_LOG_LINE: usize = 1000;

/// Runs `source` against the Model `model` of `scene`.
///
/// # Errors
///
/// [`Error::Script`] for syntax errors, runtime errors and exceeded limits (the Scene is unchanged),
/// [`Error::Scene`] when `model` is not an editable Model or the voxels the script wrote are invalid,
/// [`Error::Engine`] when the VM cannot start.
pub fn run(scene: &mut Scene, model: NodeId, source: &str, limits: &Limits) -> Result<Output, Error> {
    if scene.model_shape(model).is_none() {
        return Err(rustium_core::Error::NotAModel(model).into());
    }
    let pending = RefCell::new(Pending::default());
    let result = execute(scene, model, source, limits, &pending);
    result.map_err(|error| Error::Script(error.to_string()))?;
    let Pending { voxels, log } = pending.into_inner();
    let edit = if voxels.is_empty() {
        None
    } else {
        let command = Command::SetVoxels {
            model,
            voxels: voxels.into_iter().collect(),
        };
        apply(scene, &command)?.map(|edit| edit.with_kind(EditKind::Script))
    };
    Ok(Output { edit, log })
}

fn execute(
    scene: &Scene,
    model: NodeId,
    source: &str,
    limits: &Limits,
    pending: &RefCell<Pending>,
) -> mlua::Result<()> {
    let libraries = StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::BIT;
    let lua = Lua::new_with(libraries, LuaOptions::default())?;
    lua.set_memory_limit(limits.memory_bytes)?;
    install_interrupt(&lua, limits);
    // mlua's Luau `require` loads files from disk; scripts get no file access.
    lua.globals().set("require", Value::Nil)?;
    lua.sandbox(true)?;

    let max_voxels = limits.voxels;
    let max_log_lines = limits.log_lines;
    lua.scope(|scope| {
        let globals = lua.globals();
        let scene_api = lua.create_table()?;
        scene_api.set(
            "get",
            scope.create_function(|_, (x, y, z): (f64, f64, f64)| {
                let position = position(x, y, z)?;
                let voxel = match pending.borrow().voxels.get(&position) {
                    Some(voxel) => *voxel,
                    None => scene.model_shape(model).and_then(|(_, shape)| shape.get(position)),
                };
                Ok(voxel.map(MaterialId::get))
            })?,
        )?;
        scene_api.set(
            "set",
            scope.create_function(|_, (x, y, z, material): (f64, f64, f64, Option<u8>)| {
                let voxel = voxel(scene, material)?;
                write(pending, max_voxels, position(x, y, z)?, voxel)
            })?,
        )?;
        scene_api.set(
            "erase",
            scope.create_function(|_, (x, y, z): (f64, f64, f64)| {
                write(pending, max_voxels, position(x, y, z)?, None)
            })?,
        )?;
        scene_api.set(
            "fill",
            scope.create_function(
                |_, (x1, y1, z1, x2, y2, z2, material): (f64, f64, f64, f64, f64, f64, Option<u8>)| {
                    let voxel = voxel(scene, material)?;
                    let (a, b) = (position(x1, y1, z1)?, position(x2, y2, z2)?);
                    let (min, max) = (a.min(b), a.max(b));
                    let size = (max - min + IVec3::ONE).as_i64vec3();
                    let count = size.x * size.y * size.z;
                    if count > i64::try_from(max_voxels).unwrap_or(i64::MAX) {
                        return Err(mlua::Error::runtime(format!(
                            "fill of {count} voxels exceeds the limit of {max_voxels}"
                        )));
                    }
                    for z in min.z..=max.z {
                        for y in min.y..=max.y {
                            for x in min.x..=max.x {
                                write(pending, max_voxels, IVec3::new(x, y, z), voxel)?;
                            }
                        }
                    }
                    Ok(())
                },
            )?,
        )?;
        globals.set("scene", scene_api)?;

        let palette_api = lua.create_table()?;
        palette_api.set("len", scope.create_function(|_, ()| Ok(scene.palette().len()))?)?;
        palette_api.set(
            "color",
            scope.create_function(|_, number: u8| {
                let material = MaterialId::new(number)
                    .and_then(|id| scene.palette().get(id))
                    .ok_or_else(|| mlua::Error::runtime(format!("material {number} is not in the palette")))?;
                let [r, g, b] = material.color;
                Ok((r, g, b))
            })?,
        )?;
        palette_api.set(
            "nearest",
            scope.create_function(|_, (r, g, b): (u8, u8, u8)| {
                Ok(scene.palette().nearest([r, g, b]).map(MaterialId::get))
            })?,
        )?;
        globals.set("palette", palette_api)?;

        globals.set(
            "print",
            scope.create_function(|_, values: MultiValue| {
                let mut line = String::new();
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        line.push('\t');
                    }
                    line.push_str(&display(value)?);
                }
                line.truncate(line.floor_char_boundary(MAX_LOG_LINE));
                let mut pending = pending.borrow_mut();
                if pending.log.len() < max_log_lines {
                    pending.log.push(line);
                }
                Ok(())
            })?,
        )?;

        lua.load(source).set_name("=script").exec()
    })
}

fn display(value: &Value) -> mlua::Result<String> {
    match value {
        Value::Nil => Ok("nil".to_owned()),
        Value::String(text) => Ok(text.to_string_lossy()),
        other => other.to_string(),
    }
}

/// Stops the script when the interrupt budget or the wall-clock limit is used up.
fn install_interrupt(lua: &Lua, limits: &Limits) {
    let budget = limits.interrupts;
    let deadline = Instant::now() + limits.time;
    let count = Rc::new(Cell::new(0_u64));
    lua.set_interrupt(move |_| {
        let used = count.get() + 1;
        count.set(used);
        if used > budget {
            return Err(mlua::Error::runtime("script exceeded its instruction limit"));
        }
        // Reading the clock on every check would dominate tight loops.
        if used.is_multiple_of(1024) && Instant::now() > deadline {
            return Err(mlua::Error::runtime("script exceeded its time limit"));
        }
        Ok(VmState::Continue)
    });
}

fn position(x: f64, y: f64, z: f64) -> mlua::Result<IVec3> {
    let axis = |value: f64| -> mlua::Result<i32> {
        let rounded = value.round();
        if !rounded.is_finite() || rounded.abs() > f64::from(COORD_LIMIT) {
            return Err(mlua::Error::runtime(format!(
                "coordinate {value} is outside ±{COORD_LIMIT}"
            )));
        }
        Ok(rounded as i32)
    };
    Ok(IVec3::new(axis(x)?, axis(y)?, axis(z)?))
}

/// `nil` and `0` mean empty; any other number must be a Material of the Palette.
fn voxel(scene: &Scene, material: Option<u8>) -> mlua::Result<Voxel> {
    let Some(id) = material.and_then(MaterialId::new) else {
        return Ok(None);
    };
    if scene.palette().get(id).is_none() {
        return Err(mlua::Error::runtime(format!(
            "material {} is not in the palette",
            id.get()
        )));
    }
    Ok(Some(id))
}

fn write(pending: &RefCell<Pending>, max_voxels: usize, position: IVec3, voxel: Voxel) -> mlua::Result<()> {
    let mut pending = pending.borrow_mut();
    if pending.voxels.len() >= max_voxels && !pending.voxels.contains_key(&position) {
        return Err(mlua::Error::runtime(format!(
            "script wrote more than {max_voxels} voxels"
        )));
    }
    pending.voxels.insert(position, voxel);
    Ok(())
}

#[cfg(test)]
mod tests {
    use rustium_core::History;

    use super::*;

    fn model(scene: &Scene) -> NodeId {
        scene.node(NodeId::ROOT).unwrap().children()[0]
    }

    fn voxel(scene: &Scene, position: IVec3) -> Voxel {
        scene.model_shape(model(scene)).unwrap().1.get(position)
    }

    fn volume(scene: &Scene) -> Option<rustium_core::Volume> {
        scene.model_shape(model(scene)).unwrap().1.volume()
    }

    fn run_with(scene: &mut Scene, source: &str, limits: &Limits) -> Result<Output, Error> {
        let model = model(scene);
        run(scene, model, source, limits)
    }

    fn run_ok(scene: &mut Scene, source: &str) -> Output {
        run_with(scene, source, &Limits::default()).unwrap()
    }

    fn message(result: Result<Output, Error>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn a_script_run_is_one_undo_step() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let output = run_ok(
            &mut scene,
            "for x = 0, 9 do for y = 0, 9 do scene.set(x, y, 0, 5) end end",
        );
        let edit = output.edit.unwrap();
        assert_eq!(edit.kind(), EditKind::Script);
        history.push(edit);
        assert_eq!(history.done().count(), 1);
        assert_eq!(voxel(&scene, IVec3::new(9, 9, 0)).map(MaterialId::get), Some(5));
        history.undo(&mut scene);
        assert!(voxel(&scene, IVec3::new(9, 9, 0)).is_none());
        assert_eq!(volume(&scene), None);
    }

    #[test]
    fn a_failing_script_leaves_the_scene_untouched() {
        let mut scene = Scene::new();
        let result = run_with(&mut scene, "scene.set(0, 0, 0, 1) error('boom')", &Limits::default());
        assert!(message(result).contains("boom"));
        assert!(voxel(&scene, IVec3::ZERO).is_none());
    }

    #[test]
    fn scripts_read_the_scene_and_their_own_writes() {
        let mut scene = Scene::new();
        run_ok(&mut scene, "scene.set(1, 2, 3, 7)");
        let output = run_ok(
            &mut scene,
            "assert(scene.get(1, 2, 3) == 7) scene.erase(1, 2, 3) assert(scene.get(1, 2, 3) == nil)
             scene.set(0, 0, 0, 2) assert(scene.get(0, 0, 0) == 2) print('ok', 1, nil)",
        );
        assert_eq!(output.log, ["ok\t1\tnil"]);
        assert!(voxel(&scene, IVec3::new(1, 2, 3)).is_none());
        assert!(voxel(&scene, IVec3::ZERO).is_some());
    }

    #[test]
    fn fill_and_palette_api_work() {
        let mut scene = Scene::new();
        let output = run_ok(
            &mut scene,
            "assert(palette.len() == 96)
             local r, g, b = palette.color(1)
             scene.fill(0, 0, 0, 3, 3, 3, palette.nearest(r, g, b))",
        );
        assert!(output.edit.is_some());
        assert_eq!(volume(&scene).map(rustium_core::Volume::size), Some(IVec3::splat(4)));
    }

    #[test]
    fn dangerous_libraries_do_not_exist() {
        let mut scene = Scene::new();
        run_ok(
            &mut scene,
            "for _, name in {'io', 'os', 'package', 'debug', 'require', 'loadfile', 'dofile'} do
                 assert(_G[name] == nil, name)
             end",
        );
    }

    #[test]
    fn globals_cannot_be_tampered_with_across_runs() {
        let mut scene = Scene::new();
        let text = message(run_with(&mut scene, "string.rep = nil", &Limits::default()));
        assert!(text.contains("readonly"), "{text}");
        run_ok(&mut scene, "assert(string.rep ~= nil)");
    }

    #[test]
    fn an_endless_loop_hits_the_instruction_limit() {
        let mut scene = Scene::new();
        let limits = Limits {
            interrupts: 100_000,
            ..Limits::default()
        };
        let text = message(run_with(&mut scene, "while true do end", &limits));
        assert!(text.contains("instruction limit"), "{text}");
    }

    #[test]
    fn a_slow_script_hits_the_time_limit() {
        let mut scene = Scene::new();
        let limits = Limits {
            time: Duration::from_millis(50),
            ..Limits::default()
        };
        let text = message(run_with(&mut scene, "while true do end", &limits));
        assert!(text.contains("time limit"), "{text}");
    }

    #[test]
    fn a_memory_hog_hits_the_memory_limit() {
        let mut scene = Scene::new();
        let limits = Limits {
            memory_bytes: 8 * 1024 * 1024,
            ..Limits::default()
        };
        let text = message(run_with(&mut scene, "local s = string.rep('x', 100000000)", &limits));
        assert!(text.to_lowercase().contains("memory"), "{text}");
        let text = message(run_with(
            &mut scene,
            "local t = {} for i = 1, 1e8 do t[i] = {i} end",
            &limits,
        ));
        assert!(text.to_lowercase().contains("memory"), "{text}");
    }

    #[test]
    fn writing_too_many_voxels_is_refused() {
        let mut scene = Scene::new();
        let limits = Limits {
            voxels: 10,
            ..Limits::default()
        };
        let text = message(run_with(&mut scene, "scene.fill(0, 0, 0, 9, 9, 0, 1)", &limits));
        assert!(text.contains("limit"), "{text}");
        assert!(volume(&scene).is_none());
        let text = message(run_with(
            &mut scene,
            "for x = 0, 20 do scene.set(x, 0, 0, 1) end",
            &limits,
        ));
        assert!(text.contains("more than 10"), "{text}");
    }

    #[test]
    fn bad_arguments_are_script_errors_not_panics() {
        let mut scene = Scene::new();
        for source in [
            "scene.set(0, 0, 0, 200)",
            "scene.set(1e12, 0, 0, 1)",
            "scene.set(0/0, 0, 0, 1)",
            "scene.set('a', 0, 0, 1)",
            "palette.color(0)",
            "this is not luau",
        ] {
            assert!(run_with(&mut scene, source, &Limits::default()).is_err(), "{source}");
        }
        assert!(volume(&scene).is_none());
    }

    #[test]
    fn a_script_that_changes_nothing_has_no_edit() {
        let mut scene = Scene::new();
        assert!(run_ok(&mut scene, "scene.erase(0, 0, 0)").edit.is_none());
    }
}
