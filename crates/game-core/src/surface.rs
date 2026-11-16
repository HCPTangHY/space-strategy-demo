//! Deterministic planetary Voronoi surface generator inspired by Azgaar's Fantasy Map Generator:
//! 1. 1,000-cell jittered hexagonal lattice + 1-pass centroid relaxation (`Grid`)
//! 2. Warped multi-octave continental heightmap + ridged mountain ranges + lake/island cleanup
//! 3. Graded `Pack / regraph` ("近海细、深海大"):
//!    - Land & 1st-ring coastal water (`t = ±1`): retained and split along wide edges for sharp coasts
//!    - 2nd-ring continental shelf (`t = -2`): retained at medium density (`CoastalSea`)
//!    - Open deep ocean (`t <= -3`): merged via coarse minimum-distance sampling + relaxation into broad sea zones (`DeepOcean`)
//! 4. Exact Delaunay half-edge dual loop extraction for closed continent coastlines & inland lakes
//! 5. Region clustering: 18 land macro-regions (absorbing adjacent `CoastalSea` shelves) + 4 pelagic `DeepOcean` zones

use delaunator::{triangulate, Point as DPoint, EMPTY};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};

pub const SURFACE_WIDTH: f32 = 1200.0;
pub const SURFACE_HEIGHT: f32 = 640.0;
pub const DEFAULT_GRID_CELLS: usize = 1000;
pub const DEFAULT_LAND_REGIONS: usize = 18;
pub const DEFAULT_OCEAN_REGIONS: usize = 4;
pub const SEA_LEVEL: u8 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainKind {
    Plains,
    Forest,
    Hills,
    Mountains,
    Arid,
    Tundra,
    CoastalSea,
    DeepOcean,
}

impl TerrainKind {
    pub fn is_water(self) -> bool {
        matches!(self, Self::CoastalSea | Self::DeepOcean)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Plains => "平原",
            Self::Forest => "森林",
            Self::Hills => "丘陵",
            Self::Mountains => "山脉",
            Self::Arid => "荒漠",
            Self::Tundra => "冻土",
            Self::CoastalSea => "近海大陆架",
            Self::DeepOcean => "远洋深海",
        }
    }

    fn expansion_cost(self) -> f32 {
        match self {
            Self::Plains | Self::Forest => 1.0,
            Self::Hills | Self::Arid | Self::Tundra => 1.65,
            Self::Mountains => 3.8,
            Self::CoastalSea | Self::DeepOcean => 1.2,
        }
    }

    fn habitability(self) -> f32 {
        match self {
            Self::Plains => 1.0,
            Self::Forest => 0.85,
            Self::Hills => 0.65,
            Self::Arid => 0.35,
            Self::Tundra => 0.30,
            Self::Mountains => 0.15,
            Self::CoastalSea | Self::DeepOcean => 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceRegion {
    pub id: u16,
    pub name: String,
    pub is_ocean: bool,
    pub color_rgb: [u8; 3],
    pub center: [f32; 2],
    pub tile_count: u16,
    pub land_tile_count: u16,
    pub sea_tile_count: u16,
}

/// Compact simulation metadata stored per tile in ECS (no polygon vertices retained in World).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TileGeography {
    pub region_id: u16,
    pub terrain: TerrainKind,
    pub elevation: u8,
    pub coastal: bool,
    pub center: [f32; 2],
}

/// On-demand polygon tile (covering both land and graded sea tiles) sent once when viewing a planet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfacePolygonTile {
    pub index: u16,
    pub region_id: u16,
    pub terrain: TerrainKind,
    pub elevation: u8,
    pub coastal: bool,
    pub center: [f32; 2],
    pub polygon: Vec<[f32; 2]>,
    pub neighbors: Vec<u16>,
}

/// Full geometric surface payload generated deterministically on demand from planet seed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanetSurfaceSnapshot {
    pub planet_id: String,
    pub seed: u32,
    pub width: f32,
    pub height: f32,
    pub grid_cell_count: usize,
    pub land_tile_count: usize,
    pub coastal_sea_count: usize,
    pub deep_ocean_count: usize,
    pub ocean_merged_count: usize,
    pub regions: Vec<SurfaceRegion>,
    pub tiles: Vec<SurfacePolygonTile>,
    pub land_loops: Vec<Vec<[f32; 2]>>,
    pub lake_loops: Vec<Vec<[f32; 2]>>,
    pub region_borders: Vec<[[f32; 2]; 2]>,
    pub maritime_borders: Vec<[[f32; 2]; 2]>,
}

pub struct GeneratedSimSurface {
    pub regions: Vec<SurfaceRegion>,
    pub tiles: Vec<TileGeography>,
    pub land_tile_count: usize,
    pub sea_tile_count: usize,
    pub starter_tile_index: usize,
}

/// Muted cartographic palette for the 18 land macro-regions.
const LAND_REGION_PALETTE: [[u8; 3]; 18] = [
    [108, 152, 134],
    [176, 138, 102],
    [102, 136, 168],
    [164, 114, 112],
    [134, 154, 104],
    [138, 122, 162],
    [94, 152, 156],
    [182, 158, 108],
    [118, 128, 158],
    [156, 126, 104],
    [112, 162, 122],
    [162, 118, 142],
    [144, 148, 114],
    [98, 142, 136],
    [172, 132, 116],
    [122, 146, 168],
    [152, 144, 98],
    [128, 116, 144],
];

/// Deep bathymetric slate-blue palette for pelagic ocean regions.
const OCEAN_REGION_PALETTE: [[u8; 3]; 4] = [
    [38, 82, 118],
    [32, 72, 106],
    [42, 90, 126],
    [29, 66, 98],
];

#[derive(Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9e37_79b9_7f4a_7c15),
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut z = self.state;
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
    }
}

#[inline]
fn round1(v: f64) -> f32 {
    ((v * 10.0).round() / 10.0) as f32
}

#[inline]
fn next_halfedge(e: usize) -> usize {
    if e % 3 == 2 {
        e - 2
    } else {
        e + 1
    }
}

#[inline]
fn hash2d(seed: u64, ix: i32, iy: i32) -> f64 {
    let mut h = seed
        ^ ((ix as i64 as u64).wrapping_mul(0x517c_c1b7_2722_0a95))
        ^ ((iy as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    ((h >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
}

fn value_noise(seed: u64, x: f64, y: f64) -> f64 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - ix as f64;
    let fy = y - iy as f64;
    let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let uy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);

    let v00 = hash2d(seed, ix, iy);
    let v10 = hash2d(seed, ix + 1, iy);
    let v01 = hash2d(seed, ix, iy + 1);
    let v11 = hash2d(seed, ix + 1, iy + 1);

    let top = v00 + (v10 - v00) * ux;
    let bottom = v01 + (v11 - v01) * ux;
    top + (bottom - top) * uy
}

fn fbm(seed: u64, mut x: f64, mut y: f64, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    for oct in 0..octaves {
        let oct_seed = seed.wrapping_add((oct as u64).wrapping_mul(0x632b_e59b_d9b4_e019));
        sum += amp * value_noise(oct_seed, x, y);
        norm += amp;
        x = x * 2.03 + 13.7;
        y = y * 2.03 - 9.1;
        amp *= 0.5;
    }
    sum / norm
}

fn ridged_fbm(seed: u64, mut x: f64, mut y: f64, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    for oct in 0..octaves {
        let oct_seed = seed.wrapping_add((oct as u64).wrapping_mul(0x85eb_ca6b_c2b2_ae35));
        let n = value_noise(oct_seed, x, y);
        let ridge = 1.0 - (2.0 * n - 1.0).abs();
        sum += amp * ridge * ridge;
        norm += amp;
        x = x * 2.11 - 7.3;
        y = y * 2.11 + 11.9;
        amp *= 0.5;
    }
    sum / norm
}

struct VoronoiDiagram {
    points_n: usize,
    all_points: Vec<[f64; 2]>,
    triangles: Vec<usize>,
    halfedges: Vec<usize>,
    vertices: Vec<[f64; 2]>,
    cell_vertices: Vec<Vec<usize>>,
    cell_neighbors: Vec<Vec<usize>>,
    cell_border: Vec<bool>,
}

fn circumcenter(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    width: f64,
    height: f64,
    margin: f64,
) -> [f64; 2] {
    let (ax, ay) = (a[0], a[1]);
    let (bx, by) = (b[0], b[1]);
    let (cx, cy) = (c[0], c[1]);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-9 {
        return [(ax + bx + cx) / 3.0, (ay + by + cy) / 3.0];
    }
    let ad = ax * ax + ay * ay;
    let bd = bx * bx + by * by;
    let cd = cx * cx + cy * cy;
    let ux = (ad * (by - cy) + bd * (cy - ay) + cd * (ay - by)) / d;
    let uy = (ad * (cx - bx) + bd * (ax - cx) + cd * (bx - ax)) / d;
    [
        ux.clamp(-margin, width + margin),
        uy.clamp(-margin, height + margin),
    ]
}

fn build_boundary_points(width: f64, height: f64, spacing: f64) -> Vec<[f64; 2]> {
    let step = (spacing * 1.6).max(20.0);
    let offset = spacing * 1.15;
    let nx = ((width + 2.0 * offset) / step).ceil() as usize;
    let ny = ((height + 2.0 * offset) / step).ceil() as usize;
    let mut boundary = Vec::with_capacity((nx + ny) * 2 + 4);

    for i in 0..=nx {
        let x = -offset + (i as f64 / nx as f64) * (width + 2.0 * offset);
        boundary.push([x, -offset]);
        boundary.push([x, height + offset]);
    }
    for j in 1..ny {
        let y = -offset + (j as f64 / ny as f64) * (height + 2.0 * offset);
        boundary.push([-offset, y]);
        boundary.push([width + offset, y]);
    }
    boundary
}

fn compute_voronoi(
    points: &[[f64; 2]],
    boundary: &[[f64; 2]],
    width: f64,
    height: f64,
    clamp_margin: f64,
) -> VoronoiDiagram {
    let points_n = points.len();
    let mut all_points = Vec::with_capacity(points_n + boundary.len());
    all_points.extend_from_slice(points);
    all_points.extend_from_slice(boundary);

    let dpoints: Vec<DPoint> = all_points
        .iter()
        .map(|p| DPoint { x: p[0], y: p[1] })
        .collect();
    let tri = triangulate(&dpoints);
    let num_triangles = tri.triangles.len() / 3;

    let mut vertices = Vec::with_capacity(num_triangles);
    for t in 0..num_triangles {
        let a = all_points[tri.triangles[3 * t]];
        let b = all_points[tri.triangles[3 * t + 1]];
        let c = all_points[tri.triangles[3 * t + 2]];
        vertices.push(circumcenter(a, b, c, width, height, clamp_margin));
    }

    let mut cell_vertices = vec![Vec::new(); points_n];
    let mut cell_neighbors = vec![Vec::new(); points_n];
    let mut cell_border = vec![false; points_n];
    let mut seen = vec![false; points_n];

    for e in 0..tri.triangles.len() {
        let p = tri.triangles[next_halfedge(e)];
        if p >= points_n || seen[p] {
            continue;
        }
        seen[p] = true;

        let mut incoming = e;
        let mut verts = Vec::with_capacity(8);
        let mut neibs = Vec::with_capacity(8);
        let mut touches_border = false;

        for _ in 0..48 {
            verts.push(incoming / 3);
            let neighbor = tri.triangles[incoming];
            if neighbor < points_n {
                neibs.push(neighbor);
            } else {
                touches_border = true;
            }
            let outgoing = next_halfedge(incoming);
            let next_in = tri.halfedges[outgoing];
            if next_in == EMPTY || next_in == e {
                if next_in == EMPTY {
                    touches_border = true;
                }
                break;
            }
            incoming = next_in;
        }

        cell_vertices[p] = verts;
        cell_neighbors[p] = neibs;
        cell_border[p] = touches_border;
    }

    VoronoiDiagram {
        points_n,
        all_points,
        triangles: tri.triangles,
        halfedges: tri.halfedges,
        vertices,
        cell_vertices,
        cell_neighbors,
        cell_border,
    }
}

/// Sutherland-Hodgman polygon clipper against `[0, width] x [0, height]` so outer sea cells
/// form flush rectangular borders with zero overhang or gaps.
fn clip_polygon_to_box(poly: &[[f64; 2]], width: f64, height: f64) -> Vec<[f64; 2]> {
    fn clip_edge<F, I>(pts: &[[f64; 2]], inside: F, intersect: I) -> Vec<[f64; 2]>
    where
        F: Fn([f64; 2]) -> bool,
        I: Fn([f64; 2], [f64; 2]) -> [f64; 2],
    {
        if pts.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(pts.len() + 2);
        let mut prev = pts[pts.len() - 1];
        let mut prev_in = inside(prev);
        for &cur in pts {
            let cur_in = inside(cur);
            if cur_in {
                if !prev_in {
                    out.push(intersect(prev, cur));
                }
                out.push(cur);
            } else if prev_in {
                out.push(intersect(prev, cur));
            }
            prev = cur;
            prev_in = cur_in;
        }
        out
    }

    let p1 = clip_edge(
        poly,
        |p| p[0] >= 0.0,
        |a, b| {
            let t = (0.0 - a[0]) / (b[0] - a[0]);
            [0.0, a[1] + t * (b[1] - a[1])]
        },
    );
    let p2 = clip_edge(
        &p1,
        |p| p[0] <= width,
        |a, b| {
            let t = (width - a[0]) / (b[0] - a[0]);
            [width, a[1] + t * (b[1] - a[1])]
        },
    );
    let p3 = clip_edge(
        &p2,
        |p| p[1] >= 0.0,
        |a, b| {
            let t = (0.0 - a[1]) / (b[1] - a[1]);
            [a[0] + t * (b[0] - a[0]), 0.0]
        },
    );
    clip_edge(
        &p3,
        |p| p[1] <= height,
        |a, b| {
            let t = (height - a[1]) / (b[1] - a[1]);
            [a[0] + t * (b[0] - a[0]), height]
        },
    )
}

fn polygon_centroid_pts(pts: &[[f64; 2]], fallback: [f64; 2]) -> [f64; 2] {
    if pts.len() < 3 {
        return fallback;
    }
    let mut area2 = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..pts.len() {
        let p0 = pts[i];
        let p1 = pts[(i + 1) % pts.len()];
        let cross = p0[0] * p1[1] - p1[0] * p0[1];
        area2 += cross;
        cx += (p0[0] + p1[0]) * cross;
        cy += (p0[1] + p1[1]) * cross;
    }
    if area2.abs() < 1e-6 {
        return fallback;
    }
    [cx / (3.0 * area2), cy / (3.0 * area2)]
}

#[derive(Clone, Copy)]
struct PlateBlob {
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    weight: f64,
}

fn build_continental_plates(rng: &mut SplitMix64) -> Vec<PlateBlob> {
    let anchors: [(f64, f64, f64, f64, f64); 6] = [
        (0.22, 0.34, 0.13, 0.19, 0.92),
        (0.28, 0.68, 0.10, 0.20, 0.86),
        (0.53, 0.32, 0.15, 0.17, 0.94),
        (0.55, 0.62, 0.11, 0.19, 0.88),
        (0.73, 0.36, 0.16, 0.20, 0.98),
        (0.81, 0.72, 0.09, 0.12, 0.78),
    ];
    anchors
        .into_iter()
        .map(|(bx, by, rx, ry, w)| PlateBlob {
            cx: (bx + (rng.next_f64() - 0.5) * 0.08).clamp(0.14, 0.86),
            cy: (by + (rng.next_f64() - 0.5) * 0.09).clamp(0.18, 0.82),
            rx: rx * (0.85 + rng.next_f64() * 0.32),
            ry: ry * (0.85 + rng.next_f64() * 0.32),
            weight: w * (0.88 + rng.next_f64() * 0.24),
        })
        .collect()
}

fn evaluate_raw_continental_score(
    seed: u64,
    plates: &[PlateBlob],
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> (f64, f64) {
    let nx = x / width;
    let ny = y / height;

    let edge_x = (nx / 0.08).min((1.0 - nx) / 0.08).clamp(0.0, 1.0);
    let edge_y = (ny / 0.095).min((1.0 - ny) / 0.095).clamp(0.0, 1.0);
    let border_mask =
        edge_x * edge_x * (3.0 - 2.0 * edge_x) * edge_y * edge_y * (3.0 - 2.0 * edge_y);

    let wx = nx * 4.4 + 0.32 * (fbm(seed ^ 0x1111_2222, nx * 3.1, ny * 3.1, 4) - 0.5);
    let wy = ny * 2.6 + 0.32 * (fbm(seed ^ 0x3333_4444, nx * 3.1 + 5.2, ny * 3.1 - 3.4, 4) - 0.5);

    let mut plate_field = 0.0_f64;
    for plate in plates {
        let dx = (nx + 0.05 * (wx - nx * 4.4) - plate.cx) / plate.rx;
        let dy = (ny + 0.05 * (wy - ny * 2.6) - plate.cy) / plate.ry;
        let d2 = dx * dx + dy * dy;
        let contrib = (1.0 - d2).max(0.0).powi(2) * plate.weight;
        plate_field = plate_field.max(contrib * 0.85) + contrib * 0.22;
    }

    let detail = fbm(seed ^ 0x5555_6666, wx * 1.45, wy * 1.45, 5);
    let archipelago = fbm(seed ^ 0x7777_8888, wx * 3.6, wy * 3.6, 3);
    let ridge = ridged_fbm(seed ^ 0x9999_aaaa, wx * 1.9 + 2.1, wy * 1.9 - 1.7, 4);

    let raw_land = (plate_field * 0.56 + detail * 0.52 + archipelago * 0.14) * border_mask;
    (raw_land, ridge)
}

fn compute_heights_for_points(
    seed: u64,
    plates: &[PlateBlob],
    points: &[[f64; 2]],
    cell_border: &[bool],
    width: f64,
    height: f64,
    sea_threshold: f64,
) -> Vec<u8> {
    let mut heights = vec![6u8; points.len()];
    for (i, p) in points.iter().enumerate() {
        if cell_border[i] {
            heights[i] = 4;
            continue;
        }
        let (raw_land, ridge) =
            evaluate_raw_continental_score(seed, plates, p[0], p[1], width, height);
        if raw_land < sea_threshold {
            let ratio = (raw_land / sea_threshold.max(1e-4)).clamp(0.0, 0.99);
            heights[i] = (2.0 + ratio * 17.0).round() as u8;
        } else {
            let excess = ((raw_land - sea_threshold) / (1.0 - sea_threshold).max(0.18)).clamp(0.0, 1.4);
            let mountain_boost = ridge.powf(2.1) * (0.35 + 0.65 * excess.min(1.0));
            let elev = 20.0 + excess * 38.0 + mountain_boost * 42.0;
            heights[i] = elev.round().clamp(20.0, 98.0) as u8;
        }
    }
    heights
}

fn calibrate_sea_threshold(
    seed: u64,
    plates: &[PlateBlob],
    points: &[[f64; 2]],
    cell_border: &[bool],
    width: f64,
    height: f64,
) -> f64 {
    let mut scores: Vec<f64> = points
        .iter()
        .enumerate()
        .filter(|(i, _)| !cell_border[*i])
        .map(|(_, p)| evaluate_raw_continental_score(seed, plates, p[0], p[1], width, height).0)
        .collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    if scores.is_empty() {
        return 0.5;
    }
    let target_ocean_share = 0.66;
    let idx = ((scores.len() as f64) * target_ocean_share).floor() as usize;
    scores[idx.min(scores.len() - 1)]
}

/// Computes Azgaar-style signed coastal distance field `t`:
/// - Land: `1` = coastal land, `>= 2` = inland
/// - Water: `-1` = 1st-ring coastal/lake shore, `-2` = 2nd-ring continental shelf / lake interior,
///   `-3..-10` = progressive deep ocean rings.
/// Also returns `is_ocean` (true for border-connected global ocean, false for inland lakes/land).
fn markup_distance_field(vor: &VoronoiDiagram, heights: &mut [u8]) -> (Vec<i8>, Vec<bool>) {
    let n = vor.points_n;

    // 1. Clean isolated 1-cell land specks.
    let mut visited = vec![false; n];
    for start in 0..n {
        if visited[start] || heights[start] < SEA_LEVEL {
            continue;
        }
        let mut comp = Vec::new();
        let mut q = VecDeque::new();
        visited[start] = true;
        q.push_back(start);
        while let Some(cur) = q.pop_front() {
            comp.push(cur);
            for &nb in &vor.cell_neighbors[cur] {
                if !visited[nb] && heights[nb] >= SEA_LEVEL {
                    visited[nb] = true;
                    q.push_back(nb);
                }
            }
        }
        if comp.len() < 2 {
            for idx in comp {
                heights[idx] = SEA_LEVEL - 3;
            }
        }
    }

    // 2. Flood-fill border-connected ocean and clean 1-cell inland puddles.
    let mut is_ocean = vec![false; n];
    let mut ocean_q = VecDeque::new();
    for i in 0..n {
        if vor.cell_border[i] {
            heights[i] = heights[i].min(SEA_LEVEL - 6);
            is_ocean[i] = true;
            ocean_q.push_back(i);
        }
    }
    while let Some(cur) = ocean_q.pop_front() {
        for &nb in &vor.cell_neighbors[cur] {
            if !is_ocean[nb] && heights[nb] < SEA_LEVEL {
                is_ocean[nb] = true;
                ocean_q.push_back(nb);
            }
        }
    }

    let mut water_seen = vec![false; n];
    for start in 0..n {
        if heights[start] >= SEA_LEVEL || is_ocean[start] || water_seen[start] {
            continue;
        }
        let mut lake = Vec::new();
        let mut q = VecDeque::new();
        water_seen[start] = true;
        q.push_back(start);
        while let Some(cur) = q.pop_front() {
            lake.push(cur);
            for &nb in &vor.cell_neighbors[cur] {
                if heights[nb] < SEA_LEVEL && !is_ocean[nb] && !water_seen[nb] {
                    water_seen[nb] = true;
                    q.push_back(nb);
                }
            }
        }
        if lake.len() < 2 {
            for idx in lake {
                heights[idx] = SEA_LEVEL + 2;
            }
        }
    }

    // 3. Seed `t = 1` (land coast) and `t = -1` (water coast), then propagate outward into ocean (`-2..-10`) and land (`2..10`).
    let mut t = vec![0i8; n];
    let mut water_bfs = VecDeque::new();
    for i in 0..n {
        let land = heights[i] >= SEA_LEVEL;
        let touches_opposite = vor.cell_neighbors[i]
            .iter()
            .any(|&nb| (heights[nb] >= SEA_LEVEL) != land);
        if land {
            t[i] = if touches_opposite { 1 } else { 2 };
        } else if touches_opposite {
            t[i] = -1;
            water_bfs.push_back(i);
        } else if !is_ocean[i] {
            t[i] = -2;
        }
    }

    while let Some(cur) = water_bfs.pop_front() {
        let next_d = (t[cur] - 1).max(-10);
        for &nb in &vor.cell_neighbors[cur] {
            if heights[nb] < SEA_LEVEL && t[nb] == 0 {
                t[nb] = next_d;
                water_bfs.push_back(nb);
            }
        }
    }
    for i in 0..n {
        if heights[i] < SEA_LEVEL && t[i] == 0 {
            t[i] = -10;
        }
    }

    (t, is_ocean)
}

fn classify_land_terrain(
    seed: u64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    elevation: u8,
) -> TerrainKind {
    if elevation >= 72 {
        return TerrainKind::Mountains;
    }
    if elevation >= 53 {
        return TerrainKind::Hills;
    }

    let ny = y / height;
    let lat = (ny - 0.5).abs() * 2.0;
    let temp_jitter = (fbm(seed ^ 0xabcd_1234, x / width * 5.0, ny * 5.0, 3) - 0.5) * 0.16;
    let polar_metric = lat + temp_jitter + (elevation.saturating_sub(20) as f64) * 0.004;

    if polar_metric > 0.74 {
        return TerrainKind::Tundra;
    }

    let moisture = fbm(seed ^ 0xdcba_4321, x / width * 4.8 + 7.1, ny * 4.8 - 2.3, 4);
    if moisture < 0.37 && polar_metric < 0.58 {
        TerrainKind::Arid
    } else if moisture > 0.54 {
        TerrainKind::Forest
    } else {
        TerrainKind::Plains
    }
}

fn extract_shore_loops(
    vor: &VoronoiDiagram,
    heights: &[u8],
    is_ocean: &[bool],
) -> (Vec<Vec<[f32; 2]>>, Vec<Vec<[f32; 2]>>) {
    let n = vor.points_n;

    let mut water_body: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        if heights[i] < SEA_LEVEL && is_ocean[i] {
            water_body[i] = Some(0);
        }
    }

    let mut next_lake_id = 1usize;
    let mut q = VecDeque::new();
    for start in 0..n {
        if heights[start] < SEA_LEVEL && water_body[start].is_none() {
            let lake_id = next_lake_id;
            next_lake_id += 1;
            water_body[start] = Some(lake_id);
            q.push_back(start);
            while let Some(cur) = q.pop_front() {
                for &nb in &vor.cell_neighbors[cur] {
                    if heights[nb] < SEA_LEVEL && water_body[nb].is_none() {
                        water_body[nb] = Some(lake_id);
                        q.push_back(nb);
                    }
                }
            }
        }
    }

    let mut edges_by_body: Vec<HashMap<usize, usize>> = vec![HashMap::new(); next_lake_id];
    for e in 0..vor.triangles.len() {
        let p = vor.triangles[e];
        let q_pt = vor.triangles[next_halfedge(e)];
        let p_is_land = p < n && heights[p] >= SEA_LEVEL;
        if !p_is_land {
            continue;
        }
        let q_body = if q_pt >= n {
            Some(0)
        } else if heights[q_pt] < SEA_LEVEL {
            water_body[q_pt]
        } else {
            None
        };
        let Some(body_id) = q_body else { continue };
        let opp = vor.halfedges[e];
        if opp == EMPTY {
            continue;
        }
        edges_by_body[body_id].insert(e / 3, opp / 3);
    }

    let trace_cycles = |adj: &HashMap<usize, usize>| -> Vec<Vec<[f32; 2]>> {
        let mut starts: Vec<usize> = adj.keys().copied().collect();
        starts.sort_unstable();
        let mut visited: HashMap<usize, bool> = HashMap::with_capacity(adj.len());
        let mut loops = Vec::new();

        for start in starts {
            if visited.contains_key(&start) {
                continue;
            }
            let mut ring = Vec::new();
            let mut cur = start;
            while !visited.contains_key(&cur) {
                visited.insert(cur, true);
                let pt = vor.vertices[cur];
                ring.push([round1(pt[0]), round1(pt[1])]);
                let Some(&nxt) = adj.get(&cur) else { break };
                cur = nxt;
            }
            if ring.len() >= 3 {
                loops.push(ring);
            }
        }
        loops
    };

    let land_loops = trace_cycles(&edges_by_body[0]);
    let mut lake_loops = Vec::new();
    for adj in edges_by_body.iter().skip(1) {
        lake_loops.extend(trace_cycles(adj));
    }

    (land_loops, lake_loops)
}

#[derive(Copy, Clone, PartialEq)]
struct DijkstraEntry {
    cost: f32,
    tile: usize,
    region_id: u16,
}

impl Eq for DijkstraEntry {}

impl Ord for DijkstraEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.tile.cmp(&self.tile))
    }
}

impl PartialOrd for DijkstraEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn name_land_region(
    region_id: u16,
    center: [f32; 2],
    width: f32,
    height: f32,
    dominant: TerrainKind,
    coastal_share: f32,
    used_short_names: &mut HashMap<String, usize>,
) -> String {
    let nx = center[0] / width;
    let ny = center[1] / height;

    let ns = if ny < 0.28 {
        "极北"
    } else if ny < 0.44 {
        "北境"
    } else if ny <= 0.56 {
        "赤道"
    } else if ny <= 0.72 {
        "南境"
    } else {
        "极南"
    };

    let ew = if nx < 0.34 {
        "西陆"
    } else if nx <= 0.66 {
        "中陆"
    } else {
        "东陆"
    };

    let variants: &[&str] = if coastal_share > 0.58 {
        &["海湾区", "海峡区", "群岛区", "岬角区"]
    } else {
        match dominant {
            TerrainKind::Plains => &["平原区", "沃野区", "盆地区", "河谷区"],
            TerrainKind::Forest => &["林地区", "密林区", "林海区", "翠谷区"],
            TerrainKind::Hills => &["丘陵区", "台地区", "高地区", "冈峦区"],
            TerrainKind::Mountains => &["山岳区", "崇山区", "脊岭区", "雪峰区"],
            TerrainKind::Arid => &["旱原区", "荒漠区", "戈壁区", "沙海区"],
            _ => &["冻土区", "苔原区", "冰原区", "寒原区"],
        }
    };

    let base_key = format!("{ew}{ns}{}", variants[0]);
    let seq = used_short_names.entry(base_key).or_insert(0);
    let chosen_geo = variants[*seq % variants.len()];
    *seq += 1;

    let short = if *seq > variants.len() {
        format!("{ew}{ns}{chosen_geo}{}", *seq)
    } else {
        format!("{ew}{ns}{chosen_geo}")
    };

    format!("{:02} · {}", region_id + 1, short)
}

fn name_ocean_region(
    region_id: u16,
    center: [f32; 2],
    width: f32,
    height: f32,
    used: &mut HashMap<String, usize>,
) -> String {
    let nx = center[0] / width;
    let ny = center[1] / height;
    let basin = if nx < 0.32 {
        "西洋"
    } else if nx > 0.68 {
        "东洋"
    } else if ny < 0.5 {
        "北洋"
    } else {
        "南洋"
    };
    let variants = ["远洋海域", "深海盆地", "公海航区", "外海海岭"];
    let seq = used.entry(basin.to_string()).or_insert(0);
    let suffix = variants[*seq % variants.len()];
    *seq += 1;
    format!("{:02} · {}{}", region_id + 1, basin, suffix)
}

fn assign_all_regions(
    centers: &[[f32; 2]],
    neighbors: &[Vec<u16>],
    terrains: &[TerrainKind],
    coastal_flags: &[bool],
    width: f32,
    height: f32,
) -> (Vec<u16>, Vec<SurfaceRegion>) {
    let total_tiles = centers.len();
    let land_indices: Vec<usize> = (0..total_tiles)
        .filter(|&i| !terrains[i].is_water())
        .collect();
    let shelf_indices: Vec<usize> = (0..total_tiles)
        .filter(|&i| terrains[i] == TerrainKind::CoastalSea)
        .collect();
    let deep_indices: Vec<usize> = (0..total_tiles)
        .filter(|&i| terrains[i] == TerrainKind::DeepOcean)
        .collect();

    let mut region_of = vec![u16::MAX; total_tiles];
    let land_k = DEFAULT_LAND_REGIONS.min(land_indices.len().max(1));

    // 1. Connected land masses among land tiles.
    let mut comp_of = vec![usize::MAX; total_tiles];
    let mut comps: Vec<Vec<usize>> = Vec::new();
    for &start in &land_indices {
        if comp_of[start] != usize::MAX {
            continue;
        }
        let cid = comps.len();
        let mut members = Vec::new();
        let mut q = VecDeque::new();
        comp_of[start] = cid;
        q.push_back(start);
        while let Some(cur) = q.pop_front() {
            members.push(cur);
            for &nb_u16 in &neighbors[cur] {
                let nb = nb_u16 as usize;
                if !terrains[nb].is_water() && comp_of[nb] == usize::MAX {
                    comp_of[nb] = cid;
                    q.push_back(nb);
                }
            }
        }
        comps.push(members);
    }

    let major_comps: Vec<usize> = comps
        .iter()
        .enumerate()
        .filter(|(_, m)| m.len() >= 6)
        .map(|(idx, _)| idx)
        .collect();
    let active_comps = if major_comps.is_empty() {
        (0..comps.len()).collect::<Vec<_>>()
    } else {
        major_comps
    };

    let total_major_land: usize = active_comps.iter().map(|&c| comps[c].len()).sum();
    let mut quotas = vec![0usize; comps.len()];
    let mut assigned = 0usize;
    for &cid in &active_comps {
        if assigned < land_k {
            quotas[cid] = 1;
            assigned += 1;
        }
    }
    while assigned < land_k {
        let mut best_cid = active_comps[0];
        let mut best_gap = -1.0_f64;
        for &cid in &active_comps {
            let ideal = (comps[cid].len() as f64 / total_major_land.max(1) as f64) * (land_k as f64);
            let gap = ideal - quotas[cid] as f64;
            if gap > best_gap && quotas[cid] < comps[cid].len() {
                best_gap = gap;
                best_cid = cid;
            }
        }
        quotas[best_cid] += 1;
        assigned += 1;
    }

    let mut land_seeds: Vec<usize> = Vec::with_capacity(land_k);
    for (cid, &quota) in quotas.iter().enumerate() {
        if quota == 0 {
            continue;
        }
        let members = &comps[cid];
        let first = *members
            .iter()
            .max_by(|&&a, &&b| {
                let sa = terrains[a].habitability() + if coastal_flags[a] { 0.15 } else { 0.0 };
                let sb = terrains[b].habitability() + if coastal_flags[b] { 0.15 } else { 0.0 };
                sa.partial_cmp(&sb).unwrap_or(Ordering::Equal)
            })
            .unwrap_or(&members[0]);
        let mut chosen = vec![first];
        while chosen.len() < quota {
            let mut farthest = members[0];
            let mut best_score = -1.0_f32;
            for &cand in members {
                if chosen.contains(&cand) {
                    continue;
                }
                let min_d2 = chosen
                    .iter()
                    .map(|&s| {
                        (centers[cand][0] - centers[s][0]).powi(2)
                            + (centers[cand][1] - centers[s][1]).powi(2)
                    })
                    .fold(f32::INFINITY, f32::min);
                let score = min_d2 * (0.7 + 0.3 * terrains[cand].habitability());
                if score > best_score {
                    best_score = score;
                    farthest = cand;
                }
            }
            chosen.push(farthest);
        }
        land_seeds.extend(chosen);
    }

    // 2. Terrain-weighted Dijkstra across land tiles.
    let mut best_cost = vec![f32::INFINITY; total_tiles];
    let mut heap = BinaryHeap::new();
    for (rid, &seed_tile) in land_seeds.iter().enumerate() {
        best_cost[seed_tile] = 0.0;
        heap.push(DijkstraEntry {
            cost: 0.0,
            tile: seed_tile,
            region_id: rid as u16,
        });
    }
    while let Some(entry) = heap.pop() {
        if region_of[entry.tile] != u16::MAX {
            continue;
        }
        region_of[entry.tile] = entry.region_id;
        for &nb_u16 in &neighbors[entry.tile] {
            let nb = nb_u16 as usize;
            if terrains[nb].is_water() || region_of[nb] != u16::MAX {
                continue;
            }
            let dist = ((centers[nb][0] - centers[entry.tile][0]).powi(2)
                + (centers[nb][1] - centers[entry.tile][1]).powi(2))
            .sqrt();
            let next_cost = entry.cost + dist * terrains[nb].expansion_cost();
            if next_cost < best_cost[nb] {
                best_cost[nb] = next_cost;
                heap.push(DijkstraEntry {
                    cost: next_cost,
                    tile: nb,
                    region_id: entry.region_id,
                });
            }
        }
    }

    // Attach small offshore land islands (<6 tiles) to nearest assigned land region.
    let assigned_land: Vec<usize> = land_indices
        .iter()
        .copied()
        .filter(|&i| region_of[i] != u16::MAX)
        .collect();
    for &idx in &land_indices {
        if region_of[idx] == u16::MAX {
            if let Some(&nearest) = assigned_land.iter().min_by(|&&a, &&b| {
                let da = (centers[idx][0] - centers[a][0]).powi(2)
                    + (centers[idx][1] - centers[a][1]).powi(2);
                let db = (centers[idx][0] - centers[b][0]).powi(2)
                    + (centers[idx][1] - centers[b][1]).powi(2);
                da.partial_cmp(&db).unwrap_or(Ordering::Equal)
            }) {
                region_of[idx] = region_of[nearest];
            } else {
                region_of[idx] = 0;
            }
        }
    }

    // 3. Assign `CoastalSea` shelf tiles to their adjacent coastal land macro-region via Dijkstra.
    for &land_idx in &land_indices {
        if coastal_flags[land_idx] {
            let rid = region_of[land_idx];
            for &nb_u16 in &neighbors[land_idx] {
                let nb = nb_u16 as usize;
                if terrains[nb] == TerrainKind::CoastalSea {
                    let dist = ((centers[nb][0] - centers[land_idx][0]).powi(2)
                        + (centers[nb][1] - centers[land_idx][1]).powi(2))
                    .sqrt();
                    if dist < best_cost[nb] {
                        best_cost[nb] = dist;
                        heap.push(DijkstraEntry {
                            cost: dist,
                            tile: nb,
                            region_id: rid,
                        });
                    }
                }
            }
        }
    }
    while let Some(entry) = heap.pop() {
        if region_of[entry.tile] != u16::MAX {
            continue;
        }
        region_of[entry.tile] = entry.region_id;
        for &nb_u16 in &neighbors[entry.tile] {
            let nb = nb_u16 as usize;
            if terrains[nb] == TerrainKind::CoastalSea && region_of[nb] == u16::MAX {
                let dist = ((centers[nb][0] - centers[entry.tile][0]).powi(2)
                    + (centers[nb][1] - centers[entry.tile][1]).powi(2))
                .sqrt();
                let next_cost = entry.cost + dist;
                if next_cost < best_cost[nb] {
                    best_cost[nb] = next_cost;
                    heap.push(DijkstraEntry {
                        cost: next_cost,
                        tile: nb,
                        region_id: entry.region_id,
                    });
                }
            }
        }
    }
    for &idx in &shelf_indices {
        if region_of[idx] == u16::MAX {
            let nearest = land_indices
                .iter()
                .copied()
                .min_by(|&a, &b| {
                    let da = (centers[idx][0] - centers[a][0]).powi(2)
                        + (centers[idx][1] - centers[a][1]).powi(2);
                    let db = (centers[idx][0] - centers[b][0]).powi(2)
                        + (centers[idx][1] - centers[b][1]).powi(2);
                    da.partial_cmp(&db).unwrap_or(Ordering::Equal)
                })
                .unwrap_or(0);
            region_of[idx] = region_of[nearest].min((land_seeds.len().saturating_sub(1)) as u16);
        }
    }

    // 4. Cluster `DeepOcean` large sea tiles into 4 pelagic ocean zones (`id >= land_region_count`).
    let land_region_total = land_seeds.len();
    let ocean_k = if deep_indices.is_empty() {
        0
    } else {
        DEFAULT_OCEAN_REGIONS.min(deep_indices.len())
    };
    let mut ocean_seeds: Vec<usize> = Vec::with_capacity(ocean_k);
    if ocean_k > 0 {
        ocean_seeds.push(deep_indices[0]);
        while ocean_seeds.len() < ocean_k {
            let mut farthest = deep_indices[0];
            let mut best_d2 = -1.0_f32;
            for &cand in &deep_indices {
                if ocean_seeds.contains(&cand) {
                    continue;
                }
                let min_d2 = ocean_seeds
                    .iter()
                    .map(|&s| {
                        (centers[cand][0] - centers[s][0]).powi(2)
                            + (centers[cand][1] - centers[s][1]).powi(2)
                    })
                    .fold(f32::INFINITY, f32::min);
                if min_d2 > best_d2 {
                    best_d2 = min_d2;
                    farthest = cand;
                }
            }
            ocean_seeds.push(farthest);
        }

        for (oid, &seed_tile) in ocean_seeds.iter().enumerate() {
            let rid = (land_region_total + oid) as u16;
            best_cost[seed_tile] = 0.0;
            heap.push(DijkstraEntry {
                cost: 0.0,
                tile: seed_tile,
                region_id: rid,
            });
        }
        while let Some(entry) = heap.pop() {
            if region_of[entry.tile] != u16::MAX {
                continue;
            }
            region_of[entry.tile] = entry.region_id;
            for &nb_u16 in &neighbors[entry.tile] {
                let nb = nb_u16 as usize;
                if terrains[nb] == TerrainKind::DeepOcean && region_of[nb] == u16::MAX {
                    let dist = ((centers[nb][0] - centers[entry.tile][0]).powi(2)
                        + (centers[nb][1] - centers[entry.tile][1]).powi(2))
                    .sqrt();
                    let next_cost = entry.cost + dist;
                    if next_cost < best_cost[nb] {
                        best_cost[nb] = next_cost;
                        heap.push(DijkstraEntry {
                            cost: next_cost,
                            tile: nb,
                            region_id: entry.region_id,
                        });
                    }
                }
            }
        }
        for &idx in &deep_indices {
            if region_of[idx] == u16::MAX {
                let nearest_oid = (0..ocean_seeds.len())
                    .min_by(|&a, &b| {
                        let sa = ocean_seeds[a];
                        let sb = ocean_seeds[b];
                        let da = (centers[idx][0] - centers[sa][0]).powi(2)
                            + (centers[idx][1] - centers[sa][1]).powi(2);
                        let db = (centers[idx][0] - centers[sb][0]).powi(2)
                            + (centers[idx][1] - centers[sb][1]).powi(2);
                        da.partial_cmp(&db).unwrap_or(Ordering::Equal)
                    })
                    .unwrap_or(0);
                region_of[idx] = (land_region_total + nearest_oid) as u16;
            }
        }
    }

    // 5. Build `SurfaceRegion` metadata for both land macro-regions and pelagic ocean regions.
    let total_regions = land_region_total + ocean_k;
    let mut regions = Vec::with_capacity(total_regions);
    let mut used_land_names: HashMap<String, usize> = HashMap::new();
    let mut used_ocean_names: HashMap<String, usize> = HashMap::new();

    for rid in 0..total_regions {
        let rid_u16 = rid as u16;
        let is_ocean_region = rid >= land_region_total;
        let all_members: Vec<usize> = (0..total_tiles)
            .filter(|&i| region_of[i] == rid_u16)
            .collect();
        let land_members: Vec<usize> = all_members
            .iter()
            .copied()
            .filter(|&i| !terrains[i].is_water())
            .collect();
        let sea_count = all_members.len().saturating_sub(land_members.len());

        let anchor_pool = if !land_members.is_empty() {
            &land_members
        } else {
            &all_members
        };
        let center = if !anchor_pool.is_empty() {
            let (sx, sy) = anchor_pool.iter().fold((0.0_f32, 0.0_f32), |(ax, ay), &idx| {
                (ax + centers[idx][0], ay + centers[idx][1])
            });
            let avg_x = sx / anchor_pool.len() as f32;
            let avg_y = sy / anchor_pool.len() as f32;
            let anchor_idx = anchor_pool
                .iter()
                .copied()
                .min_by(|&a, &b| {
                    let da = ((centers[a][0] - avg_x).powi(2) + (centers[a][1] - avg_y).powi(2))
                        * if coastal_flags[a] { 1.35 } else { 1.0 };
                    let db = ((centers[b][0] - avg_x).powi(2) + (centers[b][1] - avg_y).powi(2))
                        * if coastal_flags[b] { 1.35 } else { 1.0 };
                    da.partial_cmp(&db).unwrap_or(Ordering::Equal)
                })
                .unwrap_or(anchor_pool[0]);
            centers[anchor_idx]
        } else {
            [width * 0.5, height * 0.5]
        };

        let (name, color_rgb) = if is_ocean_region {
            let oid = rid - land_region_total;
            (
                name_ocean_region(rid_u16, center, width, height, &mut used_ocean_names),
                OCEAN_REGION_PALETTE[oid % OCEAN_REGION_PALETTE.len()],
            )
        } else {
            let mut terrain_counts = [0usize; 6];
            let mut coastal_count = 0usize;
            for &idx in &land_members {
                if coastal_flags[idx] {
                    coastal_count += 1;
                }
                let b = match terrains[idx] {
                    TerrainKind::Plains => 0,
                    TerrainKind::Forest => 1,
                    TerrainKind::Hills => 2,
                    TerrainKind::Mountains => 3,
                    TerrainKind::Arid => 4,
                    _ => 5,
                };
                terrain_counts[b] += 1;
            }
            let best_b = terrain_counts
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| *c)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let dominant = match best_b {
                0 => TerrainKind::Plains,
                1 => TerrainKind::Forest,
                2 => TerrainKind::Hills,
                3 => TerrainKind::Mountains,
                4 => TerrainKind::Arid,
                _ => TerrainKind::Tundra,
            };
            let coastal_share = if !land_members.is_empty() {
                coastal_count as f32 / land_members.len() as f32
            } else {
                0.0
            };
            (
                name_land_region(
                    rid_u16,
                    center,
                    width,
                    height,
                    dominant,
                    coastal_share,
                    &mut used_land_names,
                ),
                LAND_REGION_PALETTE[rid % LAND_REGION_PALETTE.len()],
            )
        };

        regions.push(SurfaceRegion {
            id: rid_u16,
            name,
            is_ocean: is_ocean_region,
            color_rgb,
            center,
            tile_count: all_members.len() as u16,
            land_tile_count: land_members.len() as u16,
            sea_tile_count: sea_count as u16,
        });
    }

    (region_of, regions)
}

pub fn generate_planet_surface(planet_id: &str, seed: u32) -> PlanetSurfaceSnapshot {
    let width = SURFACE_WIDTH as f64;
    let height = SURFACE_HEIGHT as f64;
    let seed64 = (seed as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x4f1b_bcdd_a529_1073;
    let mut rng = SplitMix64::new(seed64);

    // Step 1: ~1,000-cell jittered hexagonal lattice (`Grid`) + 1-pass centroid relaxation.
    let spacing = ((width * height) / ((DEFAULT_GRID_CELLS as f64) * 0.866_025_4)).sqrt();
    let dy = spacing * 0.866_025_4;
    let margin = spacing * 0.5;
    let mut grid_points = Vec::with_capacity(DEFAULT_GRID_CELLS + 32);

    let mut row = 0usize;
    let mut y = margin;
    while y <= height - margin {
        let x_offset = if row % 2 == 1 { spacing * 0.5 } else { 0.0 };
        let mut x = margin + x_offset;
        while x <= width - margin {
            let jx = (rng.next_f64() - 0.5) * spacing * 0.70;
            let jy = (rng.next_f64() - 0.5) * spacing * 0.70;
            grid_points.push([
                (x + jx).clamp(margin * 0.55, width - margin * 0.55),
                (y + jy).clamp(margin * 0.55, height - margin * 0.55),
            ]);
            x += spacing;
        }
        y += dy;
        row += 1;
    }

    let boundary = build_boundary_points(width, height, spacing);
    let initial_vor = compute_voronoi(&grid_points, &boundary, width, height, spacing * 1.4);
    for i in 0..grid_points.len() {
        if !initial_vor.cell_border[i] {
            let raw_poly: Vec<[f64; 2]> = initial_vor.cell_vertices[i]
                .iter()
                .map(|&v| initial_vor.vertices[v])
                .collect();
            let c = polygon_centroid_pts(&raw_poly, grid_points[i]);
            grid_points[i] = [
                (grid_points[i][0] * 0.45 + c[0] * 0.55).clamp(margin * 0.55, width - margin * 0.55),
                (grid_points[i][1] * 0.45 + c[1] * 0.55).clamp(margin * 0.55, height - margin * 0.55),
            ];
        }
    }

    let grid_vor = compute_voronoi(&grid_points, &boundary, width, height, spacing * 1.4);
    let grid_cell_count = grid_vor.points_n;

    // Step 2: Continental heightmap & signed coastal distance field (`markupGrid`).
    let plates = build_continental_plates(&mut rng);
    let sea_threshold = calibrate_sea_threshold(
        seed64,
        &plates,
        &grid_points,
        &grid_vor.cell_border,
        width,
        height,
    );
    let mut grid_heights = compute_heights_for_points(
        seed64,
        &plates,
        &grid_points,
        &grid_vor.cell_border,
        width,
        height,
        sea_threshold,
    );
    let (grid_t, _) = markup_distance_field(&grid_vor, &mut grid_heights);

    // Step 3: Graded `Pack / regraph` — fine coastal cells (`t = ±1`), medium shelf (`t = -2`),
    // and coarse relaxed deep-ocean polygons (`t <= -3`).
    let split_dist2 = (spacing * 0.90).powi(2);
    let deep_min_dist2 = (spacing * 2.55).powi(2);

    let mut packed_points: Vec<[f64; 2]> = Vec::with_capacity(grid_cell_count);
    let mut packed_heights: Vec<u8> = Vec::with_capacity(grid_cell_count);
    let mut is_coarse_deep: Vec<bool> = Vec::with_capacity(grid_cell_count);
    let mut kept_deep_pts: Vec<[f64; 2]> = Vec::new();
    let mut ocean_merged_count = 0usize;

    for i in 0..grid_cell_count {
        let h = grid_heights[i];
        let tag = grid_t[i];

        if h < SEA_LEVEL && tag <= -3 {
            // Coarse minimum-distance retention in deep ocean so open seas merge into broad, balanced polygons.
            let [px, py] = grid_points[i];
            let too_close = kept_deep_pts
                .iter()
                .any(|&[qx, qy]| (px - qx).powi(2) + (py - qy).powi(2) < deep_min_dist2);
            if too_close {
                ocean_merged_count += 1;
                continue;
            }
            kept_deep_pts.push([px, py]);
            packed_points.push([px, py]);
            packed_heights.push(h);
            is_coarse_deep.push(true);
            continue;
        }

        let [px, py] = grid_points[i];
        packed_points.push([px, py]);
        packed_heights.push(h);
        is_coarse_deep.push(false);

        // Split wide coastal land (`t == 1`) and coastal water (`t == -1`) pairs for high-res shores.
        if (tag == 1 || tag == -1) && !grid_vor.cell_border[i] {
            for &nb in &grid_vor.cell_neighbors[i] {
                if nb <= i || grid_t[nb] != tag {
                    continue;
                }
                let [nx, ny] = grid_points[nb];
                let d2 = (px - nx).powi(2) + (py - ny).powi(2);
                if d2 < split_dist2 {
                    continue;
                }
                let mx = (px + nx) * 0.5 + (rng.next_f64() - 0.5) * spacing * 0.08;
                let my = (py + ny) * 0.5 + (rng.next_f64() - 0.5) * spacing * 0.08;
                let mid_h = ((h as u16 + grid_heights[nb] as u16) / 2) as u8;
                packed_points.push([mx, my]);
                packed_heights.push(mid_h);
                is_coarse_deep.push(false);
            }
        }
    }

    // Relax ONLY coarse deep-ocean centers once so large ocean polygons are well-proportioned without moving coastlines.
    let pre_pack_vor = compute_voronoi(&packed_points, &boundary, width, height, spacing * 2.2);
    for p in 0..packed_points.len() {
        if is_coarse_deep[p] {
            let raw_poly: Vec<[f64; 2]> = pre_pack_vor.cell_vertices[p]
                .iter()
                .map(|&v| pre_pack_vor.vertices[v])
                .collect();
            let clipped = clip_polygon_to_box(&raw_poly, width, height);
            let c = polygon_centroid_pts(&clipped, packed_points[p]);
            packed_points[p] = [
                (packed_points[p][0] * 0.35 + c[0] * 0.65).clamp(spacing * 0.8, width - spacing * 0.8),
                (packed_points[p][1] * 0.35 + c[1] * 0.65).clamp(spacing * 0.8, height - spacing * 0.8),
            ];
        }
    }

    let pack_vor = compute_voronoi(&packed_points, &boundary, width, height, spacing * 2.2);
    let (pack_t, pack_is_ocean) = markup_distance_field(&pack_vor, &mut packed_heights);

    // Step 4: Every packed cell (land, coastal shelf, and deep ocean) becomes an interactive `Tile`.
    let tile_count = pack_vor.points_n;
    let mut centers = Vec::with_capacity(tile_count);
    let mut polygons = Vec::with_capacity(tile_count);
    let mut neighbors = Vec::with_capacity(tile_count);
    let mut terrains = Vec::with_capacity(tile_count);
    let mut coastal_flags = Vec::with_capacity(tile_count);
    let mut elevations = Vec::with_capacity(tile_count);

    let mut land_tile_count = 0usize;
    let mut coastal_sea_count = 0usize;
    let mut deep_ocean_count = 0usize;

    for p in 0..tile_count {
        let raw_poly: Vec<[f64; 2]> = pack_vor.cell_vertices[p]
            .iter()
            .map(|&vid| pack_vor.vertices[vid])
            .collect();
        let clipped = clip_polygon_to_box(&raw_poly, width, height);
        let c = polygon_centroid_pts(&clipped, pack_vor.all_points[p]);
        centers.push([round1(c[0]), round1(c[1])]);

        let poly: Vec<[f32; 2]> = clipped
            .into_iter()
            .map(|pt| [round1(pt[0]), round1(pt[1])])
            .collect();
        polygons.push(poly);

        let mut nbs: Vec<u16> = pack_vor.cell_neighbors[p]
            .iter()
            .map(|&nb| nb as u16)
            .collect();
        nbs.sort_unstable();
        nbs.dedup();
        neighbors.push(nbs);

        let elev = packed_heights[p];
        elevations.push(elev);

        let terrain = if elev >= SEA_LEVEL {
            land_tile_count += 1;
            coastal_flags.push(pack_t[p] == 1);
            classify_land_terrain(seed64, c[0], c[1], width, height, elev)
        } else {
            coastal_flags.push(pack_t[p] == -1);
            // 1st-ring shore (`-1`), inland lakes (`!pack_is_ocean`), and 2nd-ring non-coarse shelf (`-2`) = CoastalSea.
            if !pack_is_ocean[p] || pack_t[p] == -1 || (pack_t[p] == -2 && !is_coarse_deep[p]) {
                coastal_sea_count += 1;
                TerrainKind::CoastalSea
            } else {
                deep_ocean_count += 1;
                TerrainKind::DeepOcean
            }
        };
        terrains.push(terrain);
    }

    // Step 5: Assign regions (18 land macro-regions absorbing adjacent `CoastalSea`, plus 4 pelagic `DeepOcean` zones).
    let (region_of, regions) = assign_all_regions(
        &centers,
        &neighbors,
        &terrains,
        &coastal_flags,
        SURFACE_WIDTH,
        SURFACE_HEIGHT,
    );

    let mut region_borders = Vec::new();
    let mut maritime_borders = Vec::new();
    for e in 0..pack_vor.triangles.len() {
        let opp = pack_vor.halfedges[e];
        if opp == EMPTY || e > opp {
            continue;
        }
        let p = pack_vor.triangles[e];
        let q = pack_vor.triangles[next_halfedge(e)];
        if p >= tile_count || q >= tile_count {
            continue;
        }
        if region_of[p] == region_of[q] {
            continue;
        }
        let va = pack_vor.vertices[e / 3];
        let vb = pack_vor.vertices[opp / 3];
        let seg = [
            [
                round1(va[0].clamp(0.0, width)),
                round1(va[1].clamp(0.0, height)),
            ],
            [
                round1(vb[0].clamp(0.0, width)),
                round1(vb[1].clamp(0.0, height)),
            ],
        ];
        if !terrains[p].is_water() && !terrains[q].is_water() {
            region_borders.push(seg);
        } else if terrains[p].is_water() && terrains[q].is_water() {
            maritime_borders.push(seg);
        }
    }

    // Step 6: Extract exact closed coastline & lake loops.
    let (land_loops, lake_loops) = extract_shore_loops(&pack_vor, &packed_heights, &pack_is_ocean);

    let tiles = (0..tile_count)
        .map(|idx| SurfacePolygonTile {
            index: idx as u16,
            region_id: region_of[idx],
            terrain: terrains[idx],
            elevation: elevations[idx],
            coastal: coastal_flags[idx],
            center: centers[idx],
            polygon: polygons[idx].clone(),
            neighbors: neighbors[idx].clone(),
        })
        .collect();

    PlanetSurfaceSnapshot {
        planet_id: planet_id.to_string(),
        seed,
        width: SURFACE_WIDTH,
        height: SURFACE_HEIGHT,
        grid_cell_count,
        land_tile_count,
        coastal_sea_count,
        deep_ocean_count,
        ocean_merged_count,
        regions,
        tiles,
        land_loops,
        lake_loops,
        region_borders,
        maritime_borders,
    }
}

/// Generates only the lightweight simulation geography stored permanently in ECS.
pub fn generate_sim_surface(planet_id: &str, seed: u32) -> GeneratedSimSurface {
    let snapshot = generate_planet_surface(planet_id, seed);
    let starter_tile_index = snapshot
        .tiles
        .iter()
        .position(|t| t.terrain == TerrainKind::Plains && t.coastal)
        .or_else(|| snapshot.tiles.iter().position(|t| t.terrain == TerrainKind::Plains))
        .or_else(|| snapshot.tiles.iter().position(|t| !t.terrain.is_water()))
        .unwrap_or(0);

    let land_tile_count = snapshot.land_tile_count;
    let sea_tile_count = snapshot.coastal_sea_count + snapshot.deep_ocean_count;

    let tiles = snapshot
        .tiles
        .into_iter()
        .map(|t| TileGeography {
            region_id: t.region_id,
            terrain: t.terrain,
            elevation: t.elevation,
            coastal: t.coastal,
            center: t.center,
        })
        .collect();

    GeneratedSimSurface {
        regions: snapshot.regions,
        tiles,
        land_tile_count,
        sea_tile_count,
        starter_tile_index,
    }
}
