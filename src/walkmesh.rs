//! Walkable ground: an arbitrary triangle mesh the characters stand on.
//!
//! Pure math with no Bevy ECS involved, so the unit tests below run
//! without an app or a window. A scene carries one of these in place of
//! the old cell grid, which could only ever describe a flat floor.

use core::f32::consts::TAU;
use std::sync::OnceLock;

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

/// Side of one bucket cell, in meters. Village-scale terrain lands a
/// handful of triangles in each.
const CELL: f32 = 1.0;

/// Ceiling on bucket cells: a scene spanning kilometers coarsens its
/// cells instead of allocating forever.
const MAX_CELLS: usize = 65_536;

/// How far outside its triangle a point may fall and still count as on
/// it, in barycentric units — about a centimeter on a meter-wide
/// triangle. Bodies therefore rest against an edge instead of being
/// pushed off it by float error, matching what the cell grid allowed.
const EDGE_EPS: f64 = 1e-4;

/// Points sampled around a body to decide whether it fits on the mesh.
/// Twelve leaves the widest unnoticed gap a twelfth of the body's
/// circumference across; the exact test would need the mesh's boundary
/// edges, which a mesh Blender did not weld does not reliably have.
const RING_SAMPLES: usize = 12;

/// A triangulated walkable surface: where characters may put their feet,
/// and how high the ground is there.
#[derive(Deserialize, Serialize, Debug, Default)]
pub struct WalkMesh {
    /// World-space triangle corners, `[x, y, z]`.
    pub vertices: Vec<[f32; 3]>,
    /// Three corner indices per triangle.
    pub triangles: Vec<[u32; 3]>,
    /// Query acceleration, built on the first query and never
    /// serialized.
    #[serde(skip)]
    buckets: OnceLock<BucketGrid>,
}

impl WalkMesh {
    /// The ground height directly under a world XZ position, if the mesh
    /// covers it. Where triangles overlap — a ledge over a path — the
    /// highest surface wins, so a character stands on top of the stack.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        self.candidates(x, z)
            .iter()
            .filter_map(|index| self.height_of(*index, x, z))
            .max_by(f32::total_cmp)
    }

    /// Whether the mesh covers a world XZ position.
    pub fn contains(&self, x: f32, z: f32) -> bool {
        self.candidates(x, z)
            .iter()
            .any(|index| self.covers(*index, x, z))
    }

    /// Whether a body — a circle of `radius` around its center — lies
    /// entirely on the mesh. The circumference is sampled rather than
    /// solved exactly, so a sliver of mesh narrower than the gap between
    /// samples can slip through. Radius `<= 0` constrains the center
    /// point alone.
    pub fn contains_circle(&self, x: f32, z: f32, radius: f32) -> bool {
        if !self.contains(x, z) {
            return false;
        }
        if radius <= 0.0 {
            return true;
        }
        (0..RING_SAMPLES).all(|sample| {
            let angle = TAU * sample as f32 / RING_SAMPLES as f32;
            self.contains(x + angle.cos() * radius, z + angle.sin() * radius)
        })
    }

    /// Restricts a desired movement so the body stays on the mesh.
    ///
    /// Tries the full move first, then each axis alone, so a character
    /// slides along the mesh's edge instead of sticking to it.
    pub fn constrain(&self, from: Vec2, to: Vec2, radius: f32) -> Vec2 {
        if self.contains_circle(to.x, to.y, radius) {
            return to;
        }
        if self.contains_circle(to.x, from.y, radius) {
            return Vec2::new(to.x, from.y);
        }
        if self.contains_circle(from.x, to.y, radius) {
            return Vec2::new(from.x, to.y);
        }
        from
    }

    /// The triangle indices bucketed under a world XZ position, building
    /// the buckets on first use.
    fn candidates(&self, x: f32, z: f32) -> &[u32] {
        self.buckets
            .get_or_init(|| BucketGrid::build(&self.vertices, &self.triangles))
            .at(x, z)
    }

    /// The plane height of one triangle at a world XZ position, if that
    /// position falls inside its ground-plane footprint.
    fn height_of(&self, index: u32, x: f32, z: f32) -> Option<f32> {
        plane_height(&self.corners(index)?, x, z)
    }

    /// Whether one triangle's footprint covers a world XZ position.
    fn covers(&self, index: u32, x: f32, z: f32) -> bool {
        self.height_of(index, x, z).is_some()
    }

    /// One triangle's corners, or `None` when an index is out of range —
    /// a hand-edited mesh can say anything.
    fn corners(&self, index: u32) -> Option<[[f32; 3]; 3]> {
        let [a, b, c] = *self.triangles.get(index as usize)?;
        Some([
            *self.vertices.get(a as usize)?,
            *self.vertices.get(b as usize)?,
            *self.vertices.get(c as usize)?,
        ])
    }
}

/// The ground a body at world XZ `(x, z)` stands on.
///
/// Falls back to the mesh's surface at `near` when the mesh does not
/// reach `(x, z)`, so a body held against the mesh's edge keeps its
/// footing instead of dropping to the void; with no mesh at all the
/// ground is flat at zero, the way it was before meshes existed.
pub fn ground_height(mesh: Option<&WalkMesh>, x: f32, z: f32, near: Vec2) -> f32 {
    mesh.and_then(|mesh| {
        mesh.height_at(x, z)
            .or_else(|| mesh.height_at(near.x, near.y))
    })
    .unwrap_or(0.0)
}

/// A uniform grid over the ground plane: each cell lists the triangles
/// whose footprint touches it, so a query tests a handful of triangles
/// instead of all of them. Built once, on the mesh's first query.
#[derive(Debug)]
struct BucketGrid {
    /// World XZ of the low corner of cell `[0][0]`.
    origin: [f32; 2],
    cell: f32,
    cols: usize,
    rows: usize,
    /// Triangle indices per cell, row-major.
    cells: Vec<Vec<u32>>,
}

impl BucketGrid {
    /// Buckets every triangle under the cells its ground-plane bounding
    /// box touches, coarsening the cell size until the grid fits
    /// [`MAX_CELLS`].
    fn build(vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> Self {
        let mut grid = BucketGrid {
            origin: [0.0, 0.0],
            cell: CELL,
            cols: 0,
            rows: 0,
            cells: Vec::new(),
        };
        let Some(bounds) = ground_bounds(vertices, triangles) else {
            return grid;
        };
        let [low, high] = bounds;
        loop {
            grid.cols = cells_across(high[0] - low[0], grid.cell);
            grid.rows = cells_across(high[1] - low[1], grid.cell);
            if grid.cols * grid.rows <= MAX_CELLS {
                break;
            }
            grid.cell *= 2.0;
        }
        grid.origin = low;
        grid.cells = vec![Vec::new(); grid.cols * grid.rows];
        for (index, tri) in triangles.iter().enumerate() {
            let Some(corners) = resolve(tri, vertices) else {
                continue;
            };
            let xs = corners.map(|c| c[0]);
            let zs = corners.map(|c| c[2]);
            let (min_col, max_col) = span(low[0], grid.cell, grid.cols, [xs[0], xs[1], xs[2]]);
            let (min_row, max_row) = span(low[1], grid.cell, grid.rows, [zs[0], zs[1], zs[2]]);
            for row in min_row..=max_row {
                for col in min_col..=max_col {
                    grid.cells[row * grid.cols + col].push(index as u32);
                }
            }
        }
        grid
    }

    /// The triangles bucketed under a world XZ position. Empty off the
    /// grid, which is how a query past the mesh's edge finds nothing.
    fn at(&self, x: f32, z: f32) -> &[u32] {
        let col = self.cell_of(self.origin[0], x, self.cols);
        let row = self.cell_of(self.origin[1], z, self.rows);
        match (col, row) {
            (Some(col), Some(row)) => &self.cells[row * self.cols + col],
            _ => &[],
        }
    }

    /// The cell a coordinate falls in, or `None` outside the grid.
    fn cell_of(&self, origin: f32, value: f32, count: usize) -> Option<usize> {
        let cell = ((value - origin) / self.cell).floor();
        if cell < 0.0 {
            return None;
        }
        let cell = cell as usize;
        (cell < count).then_some(cell)
    }
}

/// How many cells a span covers, never fewer than one.
fn cells_across(extent: f32, cell: f32) -> usize {
    ((extent / cell).floor() as usize + 1).max(1)
}

/// The inclusive cell range covering some coordinates, clamped to the
/// grid.
fn span(origin: f32, cell: f32, count: usize, values: [f32; 3]) -> (usize, usize) {
    let cell_of = |value: f32| (((value - origin) / cell).floor().max(0.0) as usize).min(count - 1);
    let low = cell_of(values[0].min(values[1]).min(values[2]));
    let high = cell_of(values[0].max(values[1]).max(values[2]));
    (low, high)
}

/// The triangles' ground-plane extent, ignoring malformed ones and any
/// non-finite vertex. `None` when nothing usable is left.
fn ground_bounds(vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> Option<[[f32; 2]; 2]> {
    let mut low = [f32::INFINITY; 2];
    let mut high = [f32::NEG_INFINITY; 2];
    for corners in triangles.iter().filter_map(|tri| resolve(tri, vertices)) {
        for corner in corners {
            for (axis, value) in [corner[0], corner[2]].into_iter().enumerate() {
                if value.is_finite() {
                    low[axis] = low[axis].min(value);
                    high[axis] = high[axis].max(value);
                }
            }
        }
    }
    (low[0] <= high[0] && low[1] <= high[1]).then_some([low, high])
}

/// A triangle's corners, or `None` when an index is out of range.
fn resolve(tri: &[u32; 3], vertices: &[[f32; 3]]) -> Option<[[f32; 3]; 3]> {
    let corner = |index: u32| vertices.get(index as usize).copied();
    Some([corner(tri[0])?, corner(tri[1])?, corner(tri[2])?])
}

/// The height of a triangle's plane at a world XZ position, when the
/// position falls inside its footprint on the ground plane. A triangle
/// standing on edge has no footprint and holds nothing.
fn plane_height(tri: &[[f32; 3]; 3], x: f32, z: f32) -> Option<f32> {
    // f64 throughout: scene coordinates run to hundreds of meters, where
    // f32 barycentric weights wobble enough to flicker along an edge.
    let (ax, ay, az) = point(tri[0]);
    let (bx, by, bz) = point(tri[1]);
    let (cx, cy, cz) = point(tri[2]);
    let (px, pz) = (x as f64, z as f64);
    let det = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
    if det.abs() < 1e-12 {
        return None;
    }
    let u = ((bz - cz) * (px - cx) + (cx - bx) * (pz - cz)) / det;
    let v = ((cz - az) * (px - cx) + (ax - cx) * (pz - cz)) / det;
    let w = 1.0 - u - v;
    if u < -EDGE_EPS || v < -EDGE_EPS || w < -EDGE_EPS {
        return None;
    }
    Some((u * ay + v * by + w * cy) as f32)
}

/// A corner as the f64 `(x, y, z)` the barycentric math wants.
fn point(v: [f32; 3]) -> (f64, f64, f64) {
    (v[0] as f64, v[1] as f64, v[2] as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat quad spanning `[low]..[high]` in XZ at height `y`.
    fn quad(low: [f32; 2], high: [f32; 2], y: f32) -> WalkMesh {
        WalkMesh {
            vertices: vec![
                [low[0], y, low[1]],
                [high[0], y, low[1]],
                [high[0], y, high[1]],
                [low[0], y, high[1]],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            ..Default::default()
        }
    }

    /// Meshes concatenate: every quad's vertices and triangles appended.
    fn joined(meshes: Vec<WalkMesh>) -> WalkMesh {
        let mut joined = WalkMesh::default();
        for mesh in meshes {
            let base = joined.vertices.len() as u32;
            joined.vertices.extend(mesh.vertices);
            joined
                .triangles
                .extend(mesh.triangles.into_iter().map(|t| t.map(|i| i + base)));
        }
        joined
    }

    #[test]
    fn a_flat_quad_reports_its_height() {
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 1.5);
        assert_eq!(mesh.height_at(1.0, 1.0), Some(1.5));
        // Both triangles of the quad answer, including their shared edge.
        assert_eq!(mesh.height_at(0.75, 0.75), Some(1.5));
        assert_eq!(mesh.height_at(1.25, 1.25), Some(1.5));
        assert_eq!(mesh.height_at(1.0, 2.0), Some(1.5));
    }

    #[test]
    fn off_the_mesh_is_not_covered() {
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        assert!(!mesh.contains(-0.5, 1.0));
        assert!(!mesh.contains(1.0, 2.5));
        assert_eq!(mesh.height_at(50.0, 50.0), None);
    }

    #[test]
    fn a_sloped_triangle_interpolates_its_height() {
        // A ramp climbing 2m over 2m of ground.
        let mesh = WalkMesh {
            vertices: vec![[0.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
            triangles: vec![[0, 1, 2]],
            ..Default::default()
        };
        assert_eq!(mesh.height_at(1.0, 0.0), Some(1.0));
        assert_eq!(mesh.height_at(0.0, 1.0), Some(0.0));
        // Halfway up the ramp in both axes: halfway in height.
        assert_eq!(mesh.height_at(0.5, 0.5), Some(0.5));
    }

    #[test]
    fn the_highest_overlapping_surface_wins() {
        // A ledge stacked over a path: characters stand on the ledge.
        let mesh = joined(vec![
            quad([0.0, 0.0], [2.0, 2.0], 0.0),
            quad([0.0, 0.0], [2.0, 2.0], 5.0),
        ]);
        assert_eq!(mesh.height_at(1.0, 1.0), Some(5.0));
    }

    #[test]
    fn a_vertical_triangle_holds_nothing() {
        // A wall has no footprint on the ground, so nothing stands on it.
        let mesh = WalkMesh {
            vertices: vec![[0.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            ..Default::default()
        };
        assert!(!mesh.contains(0.0, 0.5));
        assert_eq!(mesh.height_at(0.0, 0.5), None);
    }

    #[test]
    fn a_body_must_fit_entirely() {
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        assert!(mesh.contains_circle(1.0, 1.0, 0.4));
        // The center is on the mesh, but the body overhangs the edge.
        assert!(!mesh.contains_circle(0.2, 1.0, 0.4));
        assert!(mesh.contains_circle(0.2, 1.0, 0.0));
    }

    #[test]
    fn a_body_rests_against_the_edge() {
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        // The body stops where it touches the edge: center 0.4 short.
        assert!(mesh.contains_circle(0.4, 1.0, 0.4));
        assert!(!mesh.contains_circle(0.39, 1.0, 0.4));
    }

    #[test]
    fn a_body_fits_along_a_narrow_strip() {
        // A 0.8-wide path: a radius-0.4 body runs down its centerline.
        let mesh = quad([0.0, 0.0], [4.0, 0.8], 0.0);
        assert!(mesh.contains_circle(2.0, 0.4, 0.4));
        // Widening the body to 0.5 hangs it over both edges.
        assert!(!mesh.contains_circle(2.0, 0.4, 0.5));
    }

    #[test]
    fn constrain_keeps_free_moves() {
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        let from = Vec2::new(1.0, 1.0);
        let to = Vec2::new(1.2, 1.2);
        assert_eq!(mesh.constrain(from, to, 0.4), to);
    }

    #[test]
    fn constrain_slides_along_the_edge() {
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        // Pushing past the east edge keeps the free axis and holds X.
        let from = Vec2::new(1.0, 1.0);
        let to = Vec2::new(2.5, 1.5);
        assert_eq!(mesh.constrain(from, to, 0.4), Vec2::new(1.0, 1.5));
    }

    #[test]
    fn constrain_stops_at_a_wedge() {
        // A right-triangle of ground: near the hypotenuse, the target and
        // both single-axis slides fall off it, so the body holds its place.
        let mesh = WalkMesh {
            vertices: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 2.0]],
            triangles: vec![[0, 1, 2]],
            ..Default::default()
        };
        let from = Vec2::new(1.4, 0.5);
        let to = Vec2::new(1.6, 0.9);
        assert!(mesh.contains(from.x, from.y));
        assert!(!mesh.contains(to.x, to.y));
        assert_eq!(mesh.constrain(from, to, 0.0), from);
    }

    #[test]
    fn constrain_rejects_moves_that_overhang_the_mesh() {
        // The center point of `to` is covered, but a body around it would
        // stick out past the edge.
        let mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        let from = Vec2::new(1.0, 1.0);
        assert_eq!(mesh.constrain(from, Vec2::new(1.0, 2.2), 0.4), from);
        assert_eq!(
            mesh.constrain(from, Vec2::new(1.0, 1.6), 0.4),
            Vec2::new(1.0, 1.6)
        );
    }

    #[test]
    fn an_empty_mesh_covers_nothing() {
        let mesh = WalkMesh::default();
        assert!(!mesh.contains(0.0, 0.0));
        assert_eq!(mesh.height_at(0.0, 0.0), None);
        assert_eq!(mesh.constrain(Vec2::ZERO, Vec2::ONE, 0.0), Vec2::ZERO);
    }

    #[test]
    fn malformed_triangles_are_skipped() {
        // A hand-edited mesh can point at vertices that are not there.
        let mut mesh = quad([0.0, 0.0], [2.0, 2.0], 0.0);
        mesh.triangles.push([0, 1, 99]);
        assert_eq!(mesh.height_at(1.0, 1.0), Some(0.0));
    }

    #[test]
    fn buckets_span_the_whole_mesh() {
        // A mesh far wider than one cell: every corner of a grid of
        // probes must still be found, so no triangle is lost to a cell
        // it straddles.
        let mesh = quad([-40.0, -30.0], [40.0, 30.0], 2.0);
        for x in (-40..=40).step_by(7) {
            for z in (-30..=30).step_by(5) {
                let (x, z) = (x as f32, z as f32);
                assert!(mesh.contains(x, z), "{x},{z} was not bucketed");
            }
        }
        assert!(!mesh.contains(41.0, 0.0));
        assert!(!mesh.contains(0.0, -31.0));
    }
}
