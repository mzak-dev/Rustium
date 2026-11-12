//! Edit animations: how voxels appear and vanish when they are edited.
//!
//! They are only a view. The Model changes at once and the Tools always aim at it; the animator draws the changed
//! cells as overlay boxes and hides the cells it is showing from the chunk meshes until they settle. Every cell runs
//! on its own clock, so nothing queues up behind a long stroke. The Settings preview drives this same animator with
//! a scripted stroke, so the two cannot drift apart.

use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};
use rustium_core::{ChunkCoord, ColorId, Mirror, Placement, ShapeId, Voxel};
use rustium_render::CubeInstance;

use crate::editor::{EditEvent, EditPhase};
use crate::settings::{EditAnimation, Motion};

/// A gesture that changes more cells than this only flashes, whatever the chosen animation.
const FLASH_ONLY: usize = 2_000;
/// A gesture that changes more cells than this is not animated at all.
const NOT_ANIMATED: usize = 60_000;
/// Ghost: how long a whole-gesture change stays see-through before it hardens.
const GHOST_HOLD: f64 = 0.140;
/// Ghost: the gap between cells hardening one after another, and the longest the whole sequence may take.
const GHOST_STEP: f64 = 0.018;
const GHOST_SPAN: f64 = 0.160;
/// Wave: the delay per cell of distance from the start of the gesture, and the longest delay.
const WAVE_STEP: f64 = 0.024;
const WAVE_SPAN: f64 = 0.180;
/// Color mixed into cells a Ghost stroke is about to erase.
const ERASE_TINT: [f32; 3] = [0.9, 0.18, 0.12];

/// Whether a cell was filled or emptied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Add,
    Remove,
}

/// How one cell looks at one moment, relative to a full unit cube in its cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Look {
    /// Size on each axis, 1 = the whole cell.
    pub(crate) scale: Vec3,
    /// Shift of the centre, in cells.
    pub(crate) offset: Vec3,
    pub(crate) alpha: f32,
    /// Push towards white.
    pub(crate) glow: f32,
    /// White edges.
    pub(crate) outline: f32,
    /// Mix towards [`ERASE_TINT`].
    pub(crate) tint: f32,
    /// Slightly larger than the cell, drawn over the voxel itself (a flash over geometry that is already there).
    pub(crate) overlay: bool,
}

impl Look {
    pub(crate) const FULL: Self = Self {
        scale: Vec3::ONE,
        offset: Vec3::ZERO,
        alpha: 1.0,
        glow: 0.0,
        outline: 0.0,
        tint: 0.0,
        overlay: false,
    };
}

/// Seconds a cell's motion lasts. Erasing is always quicker than adding.
pub(crate) fn duration(animation: EditAnimation, change: Change) -> f64 {
    match (animation, change) {
        (EditAnimation::Ghost, Change::Add) | (EditAnimation::FromWall, Change::Remove) => 0.090,
        (EditAnimation::Ghost | EditAnimation::Pop | EditAnimation::Wave, Change::Remove) => 0.080,
        (EditAnimation::Pop | EditAnimation::Wave, Change::Add) => 0.110,
        (EditAnimation::FromWall, Change::Add) => 0.120,
        (EditAnimation::Flash, Change::Add) => 0.220,
        (EditAnimation::Flash, Change::Remove) => 0.200,
    }
}

/// The look of a cell at `progress` (0 to 1) through its motion. `amplitude` (0 to 1) tones big brushes down and
/// `normal` is the face the gesture landed on.
pub(crate) fn look(animation: EditAnimation, change: Change, progress: f32, amplitude: f32, normal: Vec3) -> Look {
    let p = progress.clamp(0.0, 1.0);
    let grow = |size: f32| Look {
        scale: Vec3::splat(size),
        ..Look::FULL
    };
    match (animation, change) {
        (EditAnimation::Ghost, Change::Add) => Look {
            alpha: 0.45 + 0.55 * p,
            outline: 1.0 - p,
            ..Look::FULL
        },
        (EditAnimation::Ghost, Change::Remove) => Look {
            tint: 0.6,
            ..grow(1.0 - ease_in(p))
        },
        (EditAnimation::Pop | EditAnimation::Wave, Change::Add) => {
            let start = 1.0 - 0.4 * amplitude;
            Look {
                alpha: (p * 4.0).min(1.0),
                ..grow(start + (1.0 - start) * ease_out_back(p, 1.7 * amplitude))
            }
        }
        (EditAnimation::Pop | EditAnimation::Wave, Change::Remove) => grow(1.0 - ease_in(p)),
        (EditAnimation::FromWall, change) => {
            let size = match change {
                Change::Add => ease_out(p),
                Change::Remove => 1.0 - ease_in(p),
            };
            // Squashed along the normal and resting on the face behind it.
            let axis = normal.abs();
            Look {
                scale: Vec3::ONE - axis * (1.0 - size),
                offset: -normal * (1.0 - size) / 2.0,
                ..Look::FULL
            }
        }
        (EditAnimation::Flash, Change::Add) => Look {
            alpha: 0.55 * (1.0 - p),
            glow: 1.0,
            overlay: true,
            ..Look::FULL
        },
        (EditAnimation::Flash, Change::Remove) => Look {
            alpha: 0.0,
            outline: 1.0 - p,
            ..Look::FULL
        },
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t * t
}

/// Ease out with an overshoot of strength `back` (0 = none).
fn ease_out_back(t: f32, back: f32) -> f32 {
    let u = t - 1.0;
    1.0 + u * u * ((back + 1.0) * u + back)
}

/// Which animation plays: Motion wins over the chosen animation.
pub(crate) fn effective(motion: Motion, chosen: EditAnimation) -> Option<EditAnimation> {
    match motion {
        Motion::Full => Some(chosen),
        Motion::Reduced => Some(EditAnimation::Flash),
        Motion::Off => None,
    }
}

/// One cell's animation.
#[derive(Debug, Clone, Copy)]
struct CellAnim {
    change: Change,
    animation: EditAnimation,
    /// When the motion starts, on the animator's clock; `None` while a Ghost waits for its stroke to end.
    start: Option<f64>,
    /// Plays the motion backwards (undo).
    reverse: bool,
    amplitude: f32,
    normal: Vec3,
    color: [f32; 3],
}

impl CellAnim {
    fn end(&self) -> Option<f64> {
        self.start.map(|start| start + duration(self.animation, self.change))
    }

    /// Whether the Model holds the cell while it animates, so the meshes must leave it out.
    fn hides_voxel(&self) -> bool {
        let filled = (self.change == Change::Add) != self.reverse;
        filled && self.animation != EditAnimation::Flash
    }

    fn look_at(&self, now: f64) -> Option<Look> {
        let Some(start) = self.start else {
            // A Ghost waiting for its stroke to end.
            return Some(look(self.animation, self.change, 0.0, self.amplitude, self.normal));
        };
        let waiting = now < start;
        if waiting && self.change == Change::Add && !self.reverse && self.animation != EditAnimation::Ghost {
            // Not grown yet (a Wave that has not reached it).
            return None;
        }
        #[expect(clippy::cast_possible_truncation, reason = "a progress between 0 and 1")]
        let progress = ((now - start) / duration(self.animation, self.change)).clamp(0.0, 1.0) as f32;
        let progress = if self.reverse { 1.0 - progress } else { progress };
        Some(look(self.animation, self.change, progress, self.amplitude, self.normal))
    }
}

/// The edit animations running in the 3D view (or the Settings preview).
#[derive(Debug, Default)]
pub(crate) struct Animator {
    cells: HashMap<(ShapeId, IVec3), CellAnim>,
    /// Ghost cells of the stroke in progress, in the order they changed.
    pending: Vec<(ShapeId, IVec3)>,
    /// Chunks whose hidden cells changed, to mesh again.
    remesh: HashSet<(ShapeId, ChunkCoord)>,
}

impl Animator {
    /// Takes in one change from the Editor at time `now` (seconds). `animation` is what plays, `None` for none;
    /// `color` turns a Color into linear RGB.
    pub(crate) fn handle(
        &mut self,
        event: EditEvent,
        animation: Option<EditAnimation>,
        now: f64,
        color: impl Fn(ColorId) -> [f32; 3],
    ) {
        match event {
            EditEvent::Voxels {
                phase,
                changes,
                origin,
                normal,
                mirror,
                brush,
            } => {
                let Some(chosen) = animation else {
                    return;
                };
                if changes.len() > NOT_ANIMATED {
                    return;
                }
                let animation = if changes.len() > FLASH_ONLY {
                    EditAnimation::Flash
                } else {
                    chosen
                };
                // Bigger brushes move less.
                let amplitude = (1.0 / (brush.max(1) as f32).sqrt()).clamp(0.35, 1.0);
                let reverse = phase == EditPhase::Undo;
                let count = changes.len();
                for (index, (shape, cell, before, after)) in changes.into_iter().enumerate() {
                    let (change, voxel) = cell_change(before, after, reverse);
                    let Some(id) = voxel else {
                        continue;
                    };
                    let start = match (animation, phase) {
                        (EditAnimation::Ghost, EditPhase::Stroke) => {
                            self.pending.push((shape, cell));
                            None
                        }
                        (EditAnimation::Ghost, EditPhase::Commit) => Some(now + GHOST_HOLD + ghost_delay(index, count)),
                        (EditAnimation::Ghost, _) => Some(now + ghost_delay(index, count)),
                        (EditAnimation::Wave, _) => Some(now + wave_delay(cell, origin, mirror)),
                        _ => Some(now),
                    };
                    let anim = CellAnim {
                        change,
                        animation,
                        start,
                        reverse,
                        amplitude,
                        normal: normal.as_vec3(),
                        color: color(id),
                    };
                    // A cell edited again in the middle of its animation simply starts over.
                    if let Some(old) = self.cells.insert((shape, cell), anim)
                        && old.hides_voxel()
                    {
                        self.remesh.insert((shape, ChunkCoord::of(cell)));
                    }
                    if anim.hides_voxel() {
                        self.remesh.insert((shape, ChunkCoord::of(cell)));
                    }
                }
            }
            EditEvent::StrokeEnd => {
                let pending = std::mem::take(&mut self.pending);
                let count = pending.len();
                for (index, key) in pending.into_iter().enumerate() {
                    if let Some(anim) = self.cells.get_mut(&key)
                        && anim.start.is_none()
                    {
                        anim.start = Some(now + ghost_delay(index, count));
                    }
                }
            }
            EditEvent::StrokeCancel => {
                for key in std::mem::take(&mut self.pending) {
                    if self.cells.remove(&key).is_some_and(|anim| anim.hides_voxel()) {
                        self.remesh.insert((key.0, ChunkCoord::of(key.1)));
                    }
                }
            }
        }
    }

    /// Drops the animations that ended by `now`.
    pub(crate) fn tick(&mut self, now: f64) {
        let remesh = &mut self.remesh;
        self.cells.retain(|(shape, cell), anim| {
            let running = anim.end().is_none_or(|end| end > now);
            if !running && anim.hides_voxel() {
                remesh.insert((*shape, ChunkCoord::of(*cell)));
            }
            running
        });
    }

    /// Whether anything is moving, so the view must keep drawing frames.
    pub(crate) fn is_moving(&self) -> bool {
        self.cells.values().any(|anim| anim.start.is_some())
    }

    /// Chunks to mesh again because the cells hidden in them changed.
    pub(crate) fn take_remesh(&mut self) -> Vec<(ShapeId, ChunkCoord)> {
        self.remesh.drain().collect()
    }

    /// The cells of `shape` the mesh of the chunk at `coord` must leave out: those in it and in the chunks around it,
    /// whose voxels decide which of its faces show.
    pub(crate) fn hidden(&self, shape: ShapeId, coord: ChunkCoord) -> Vec<IVec3> {
        let near = |cell: IVec3| (ChunkCoord::of(cell).0 - coord.0).abs().max_element() <= 1;
        self.cells
            .iter()
            .filter(|((id, cell), anim)| *id == shape && near(*cell) && anim.hides_voxel())
            .map(|((_, cell), _)| *cell)
            .collect()
    }

    /// Whether any cell of `shape` is hidden from the meshes.
    pub(crate) fn hides_any(&self, shape: ShapeId) -> bool {
        self.cells
            .iter()
            .any(|((id, _), anim)| *id == shape && anim.hides_voxel())
    }

    /// The boxes to draw at `now` for every Model placing an animated Shape.
    pub(crate) fn cubes(&self, now: f64, placements: &[Placement]) -> Vec<CubeInstance> {
        let mut cubes = Vec::new();
        for ((shape, cell), anim) in &self.cells {
            let Some(look) = anim.look_at(now) else {
                continue;
            };
            for placement in placements.iter().filter(|placement| placement.shape == *shape) {
                cubes.push(cube(
                    *cell,
                    &look,
                    anim.color,
                    |point| placement.transform.point(point),
                    |vector| placement.transform.orientation.apply_float(vector),
                ));
            }
        }
        cubes
    }
}

/// Length of the Settings preview loop, in seconds.
const PREVIEW_LOOP: f64 = 3.6;

/// The scripted gesture the Settings preview plays: (time, event) pairs within one loop. An Attach stroke along a
/// row, an Erase stroke back along it, then a 3 Ă— 3 patch added and removed at once.
fn preview_script() -> Vec<(f64, EditEvent)> {
    let shape = preview_shape();
    let stroke = |cells: &[IVec3], add: bool, phase: EditPhase| EditEvent::Voxels {
        phase,
        changes: cells
            .iter()
            .map(|cell| {
                let (before, after) = if add {
                    (None, ColorId::new(2))
                } else {
                    (ColorId::new(2), None)
                };
                (shape, *cell, before, after)
            })
            .collect(),
        origin: cells.first().copied().unwrap_or_default(),
        normal: IVec3::Y,
        mirror: Mirror::default(),
        brush: 1,
    };
    let row: Vec<IVec3> = (0..5).map(|x| IVec3::new(x, 1, 2)).collect();
    let patch: Vec<IVec3> = (1..4).flat_map(|x| (1..4).map(move |z| IVec3::new(x, 1, z))).collect();
    let mut script = Vec::new();
    for (index, cell) in row.iter().enumerate() {
        script.push((0.1 + index as f64 * 0.09, stroke(&[*cell], true, EditPhase::Stroke)));
    }
    script.push((0.6, EditEvent::StrokeEnd));
    for (index, cell) in row.iter().rev().enumerate() {
        script.push((1.2 + index as f64 * 0.07, stroke(&[*cell], false, EditPhase::Stroke)));
    }
    script.push((1.6, EditEvent::StrokeEnd));
    script.push((2.1, stroke(&patch, true, EditPhase::Commit)));
    script.push((3.0, stroke(&patch, false, EditPhase::Commit)));
    script
}

/// The preview's Shape id: any id works, as long as the floor and the script agree.
fn preview_shape() -> ShapeId {
    rustium_core::Scene::new().placements()[0].shape
}

/// Paints the Settings preview of `animation` at `time` (seconds) into `rect`. It replays the scripted gesture on a
/// real [`Animator`], so it shows exactly what the 3D view does.
pub(crate) fn paint_preview(painter: &egui::Painter, rect: egui::Rect, animation: Option<EditAnimation>, time: f64) {
    let now = time.rem_euclid(PREVIEW_LOOP);
    let shape = preview_shape();
    let colors = |id: ColorId| match id.get() {
        1 => [0.16, 0.18, 0.21],
        _ => [0.85, 0.32, 0.08],
    };
    let mut animator = Animator::default();
    let mut present: HashSet<IVec3> = HashSet::new();
    for (at, event) in preview_script().into_iter().take_while(|(at, _)| *at <= now) {
        if let EditEvent::Voxels { changes, .. } = &event {
            for (_, cell, _, after) in changes {
                if after.is_some() {
                    present.insert(*cell);
                } else {
                    present.remove(cell);
                }
            }
        }
        animator.handle(event, animation, at, colors);
    }
    animator.tick(now);
    let hidden: HashSet<IVec3> = animator.hidden(shape, ChunkCoord(IVec3::ZERO)).into_iter().collect();
    let floor = (0..5).flat_map(|x| (0..5).map(move |z| IVec3::new(x, 0, z)));
    let mut cubes: Vec<CubeInstance> = floor
        .map(|cell| (cell, colors(ColorId::FIRST)))
        .chain(
            present
                .iter()
                .filter(|cell| !hidden.contains(*cell))
                .map(|cell| (*cell, [0.85, 0.32, 0.08])),
        )
        .map(|(cell, color)| cube(cell, &Look::FULL, color, |point| point, |vector| vector))
        .collect();
    let placement = rustium_core::Placement {
        node: rustium_core::NodeId::ROOT,
        shape,
        transform: rustium_core::Transform::IDENTITY,
        rank: 0,
    };
    cubes.extend(animator.cubes(now, &[placement]));
    paint_isometric(painter, rect, &mut cubes);
}

/// Draws boxes in an isometric view seen from +X, +Y, +Z, farthest first.
fn paint_isometric(painter: &egui::Painter, rect: egui::Rect, cubes: &mut [CubeInstance]) {
    use egui::{Color32, Pos2, Shape, Stroke};
    let depth = |cube: &CubeInstance| cube.centre[0] + cube.centre[1] + cube.centre[2];
    cubes.sort_by(|a, b| depth(a).total_cmp(&depth(b)));
    let unit = (rect.width() / 9.0).min(rect.height() / 7.5);
    // The middle of the 5 Ă— 5 floor sits in the middle of the frame.
    let origin = rect.center() - egui::vec2(0.0, -unit * 1.2);
    let project = |p: Vec3| {
        let p = p - Vec3::new(2.5, 0.5, 2.5);
        Pos2::new(
            origin.x + (p.x - p.z) * 0.866 * unit,
            origin.y + (f32::midpoint(p.x, p.z) - p.y) * unit,
        )
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 first"
    )]
    let srgb = |channel: f32| (channel.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
    for cube in cubes.iter() {
        let centre = Vec3::new(cube.centre[0], cube.centre[1], cube.centre[2]);
        let half = Vec3::new(cube.half_size[0], cube.half_size[1], cube.half_size[2]);
        if half.min_element() <= 1e-3 && cube.centre[3] <= 0.0 {
            continue;
        }
        let (outline, glow, alpha) = (cube.centre[3], cube.half_size[3], cube.color[3]);
        // The three faces that face the viewer, with a little shading each.
        for (axis, shade) in [(1_usize, 1.0_f32), (0, 0.78), (2, 0.62)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |a: f32, b: f32| {
                let mut offset = Vec3::ZERO;
                offset[axis] = half[axis];
                offset[u] = a * half[u];
                offset[v] = b * half[v];
                project(centre + offset)
            };
            let points = vec![
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ];
            let rgb = Vec3::from_slice(&cube.color[..3]) * shade;
            let rgb = rgb.lerp(Vec3::ONE, glow);
            let fill = Color32::from_rgba_unmultiplied(srgb(rgb.x), srgb(rgb.y), srgb(rgb.z), srgb(alpha.powf(2.2)));
            let edge = Color32::from_white_alpha(srgb((outline * 0.9).powf(2.2)));
            painter.add(Shape::convex_polygon(points, fill, Stroke::new(1.0, edge)));
        }
    }
}

/// The overlay box for `cell` with `look`, mapped into the Scene by `point` and `vector`.
pub(crate) fn cube(
    cell: IVec3,
    look: &Look,
    color: [f32; 3],
    point: impl Fn(Vec3) -> Vec3,
    vector: impl Fn(Vec3) -> Vec3,
) -> CubeInstance {
    let grow = if look.overlay { 1.03 } else { 1.0 };
    let centre = point(cell.as_vec3() + Vec3::splat(0.5) + look.offset);
    let half = vector(look.scale * 0.5 * grow).abs();
    let rgb = Vec3::from(color).lerp(Vec3::from(ERASE_TINT), look.tint);
    CubeInstance {
        centre: centre.extend(look.outline).to_array(),
        half_size: half.extend(look.glow).to_array(),
        color: rgb.extend(look.alpha).to_array(),
    }
}

/// What happened to a cell and the Color to show it in; for undo the change runs backwards, so the motion is the one
/// of the original change, played in reverse.
fn cell_change(before: Voxel, after: Voxel, reverse: bool) -> (Change, Voxel) {
    let (was, now) = if reverse { (after, before) } else { (before, after) };
    match (was, now) {
        (Some(_), None) => (Change::Remove, was),
        // Added, or recolored (shown like a fresh voxel in its new Color).
        _ => (Change::Add, now),
    }
}

fn ghost_delay(index: usize, count: usize) -> f64 {
    let step = GHOST_STEP.min(GHOST_SPAN / count.max(1) as f64);
    index as f64 * step
}

/// Delay of a Wave at `cell`: distance from the start, measured to the nearest Mirror image so the images move
/// together with the original.
fn wave_delay(cell: IVec3, origin: IVec3, mirror: Mirror) -> f64 {
    let mut nearest = f32::MAX;
    mirror.for_each_image(cell, |image| nearest = nearest.min((image - origin).as_vec3().length()));
    (f64::from(nearest) * WAVE_STEP).min(WAVE_SPAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> ShapeId {
        rustium_core::Scene::new().placements()[0].shape
    }

    fn added(cells: &[IVec3], phase: EditPhase) -> EditEvent {
        let color = ColorId::new(1);
        EditEvent::Voxels {
            phase,
            changes: cells.iter().map(|cell| (shape(), *cell, None, color)).collect(),
            origin: IVec3::ZERO,
            normal: IVec3::Y,
            mirror: Mirror::default(),
            brush: 1,
        }
    }

    #[test]
    fn erasing_is_quicker_than_adding_in_every_animation() {
        for animation in EditAnimation::ALL {
            assert!(
                duration(animation, Change::Remove) < duration(animation, Change::Add),
                "{animation:?}"
            );
        }
    }

    #[test]
    fn every_motion_ends_on_a_whole_cube() {
        for animation in EditAnimation::ALL {
            if animation == EditAnimation::Flash {
                continue;
            }
            let end = look(animation, Change::Add, 1.0, 1.0, Vec3::Y);
            assert!(
                (end.scale - Vec3::ONE).length() < 1e-4 && end.offset.length() < 1e-4,
                "{animation:?}"
            );
            assert!((end.alpha - 1.0).abs() < 1e-4, "{animation:?}");
            let gone = look(animation, Change::Remove, 1.0, 1.0, Vec3::Y);
            assert!(gone.scale.min_element() < 1e-4, "{animation:?} vanishes");
        }
    }

    #[test]
    fn ghost_cells_wait_for_the_stroke_then_harden_one_after_another() {
        let mut animator = Animator::default();
        let cells: Vec<IVec3> = (0..20).map(|x| IVec3::new(x, 0, 0)).collect();
        animator.handle(
            added(&cells, EditPhase::Stroke),
            Some(EditAnimation::Ghost),
            0.0,
            |_| [1.0; 3],
        );
        animator.tick(5.0);
        assert!(!animator.is_moving(), "nothing moves before the release");
        assert_eq!(animator.hidden(shape(), ChunkCoord::of(IVec3::ZERO)).len(), 20);
        animator.handle(EditEvent::StrokeEnd, Some(EditAnimation::Ghost), 5.0, |_| [1.0; 3]);
        assert!(animator.is_moving());
        // The last cell starts at most 160 ms after the release and takes 90 ms.
        animator.tick(5.0 + GHOST_SPAN + 0.091);
        assert!(animator.cells.is_empty());
        assert!(
            !animator.take_remesh().is_empty(),
            "the hardened cells go back into the mesh"
        );
    }

    #[test]
    fn a_cell_edited_again_restarts_and_nothing_queues() {
        let mut animator = Animator::default();
        let cell = [IVec3::ZERO];
        animator.handle(added(&cell, EditPhase::Commit), Some(EditAnimation::Pop), 0.0, |_| {
            [1.0; 3]
        });
        animator.handle(added(&cell, EditPhase::Commit), Some(EditAnimation::Pop), 0.05, |_| {
            [1.0; 3]
        });
        animator.tick(0.05 + duration(EditAnimation::Pop, Change::Add) + 1e-3);
        assert!(animator.cells.is_empty(), "the second edit ends on its own clock");
    }

    #[test]
    fn motion_wins_over_the_chosen_animation() {
        assert_eq!(effective(Motion::Full, EditAnimation::Wave), Some(EditAnimation::Wave));
        assert_eq!(
            effective(Motion::Reduced, EditAnimation::Wave),
            Some(EditAnimation::Flash)
        );
        assert_eq!(effective(Motion::Off, EditAnimation::Wave), None);
    }

    #[test]
    fn wave_images_move_together() {
        let mirror = Mirror {
            axes: [true, false, false],
            pivot: IVec3::ZERO,
        };
        let origin = IVec3::new(3, 0, 0);
        assert!((wave_delay(origin, origin, mirror) - wave_delay(IVec3::new(-3, 0, 0), origin, mirror)).abs() < 1e-9);
    }
}
