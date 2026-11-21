// Clean 2-tier sea & smooth/fractal coastline compiler:
// - Land coastline: Azgaar hybrid quadratic B-spline + fractal roughness
// - Shallow/Deep sea boundary: single `curveBasisClosed` contour matching the exact `CoastalSea` outer edge
// - Sea grid: gently curved symmetric shared edges so deep-ocean cells look organic without visual clutter
import { isWaterTags } from './game';
import type { PlanetSurfaceSnapshot, SurfacePolygonTile } from './game';

export type Point2D = [number, number];

interface CoastSettings {
  maxDepth: number;
  baseAmplitude: number;
  amplitudeDecay: number;
  minEdge: number;
  smoothThreshold: number;
  roughnessContrast: number;
  roughnessScale: number;
}

const OCEAN_COAST_SETTINGS: CoastSettings = {
  maxDepth: 4,
  baseAmplitude: 1.45,
  amplitudeDecay: 0.88,
  minEdge: 1.2,
  smoothThreshold: 0.26,
  roughnessContrast: 1.5,
  roughnessScale: 68,
};

const LAKE_COAST_SETTINGS: CoastSettings = {
  ...OCEAN_COAST_SETTINGS,
  baseAmplitude: 1.05,
  smoothThreshold: 0.52,
};

const QUANTUM = 64;
const OCTAVE_WEIGHT = 0.35;
const FIELD_STRETCH = 1.9;
const SIMPLIFY_MIN_DIST2 = 0.35 * 0.35;

interface FractalizedLoop {
  points: Point2D[];
  origIndices: number[];
}

export interface CompiledSurfaceGeometry {
  landMaskSvgPath: string;
  oceanCoastSvgPath: string;
  lakeCoastSvgPath: string;
  landMaskPath: Path2D;
  waterMaskPath: Path2D;
  oceanCoastPath: Path2D;
  lakeCoastPath: Path2D;
  shelfContourPath: Path2D;
  regionBorderPath: Path2D;
  landInternalGridPath: Path2D;
  seaInternalGridPath: Path2D;
  tilePaths: Path2D[];
  regionSolidPaths: Path2D[];
  regionPoliticalBorderPaths: Path2D[];
  hitTestTile: (
    ctx: CanvasRenderingContext2D,
    mapX: number,
    mapY: number,
  ) => SurfacePolygonTile | null;
}

function murmurNoise(seed: number, ...values: number[]): number {
  let h = seed | 0;
  for (const value of values) {
    h = Math.imul(h ^ (value | 0), 0x5bd1e995);
    h ^= h >>> 13;
  }
  h = Math.imul(h ^ (h >>> 16), 0x85ebca6b);
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

function fieldAt(seed: number, x: number, y: number, scale: number): number {
  const fx = x / scale;
  const fy = y / scale;
  const ix = Math.floor(fx);
  const iy = Math.floor(fy);
  const tx = fx - ix;
  const ty = fy - iy;
  const sx = tx * tx * (3 - 2 * tx);
  const sy = ty * ty * (3 - 2 * ty);

  const v00 = murmurNoise(seed, ix, iy);
  const v10 = murmurNoise(seed, ix + 1, iy);
  const v01 = murmurNoise(seed, ix, iy + 1);
  const v11 = murmurNoise(seed, ix + 1, iy + 1);
  const top = v00 * (1 - sx) + v10 * sx;
  const bottom = v01 * (1 - sx) + v11 * sx;
  return top * (1 - sy) + bottom * sy;
}

function roughnessAt(seed: number, x: number, y: number, settings: CoastSettings): number {
  const scale = Math.max(settings.roughnessScale, 1);
  const base = fieldAt(seed, x, y, scale);
  const detail = fieldAt(seed ^ 0x9e3779b9, x, y, scale / 2);
  const combined = base * (1 - OCTAVE_WEIGHT) + detail * OCTAVE_WEIGHT;
  const spread = Math.min(1, Math.max(0, (combined - 0.5) * FIELD_STRETCH + 0.5));
  return spread ** settings.roughnessContrast;
}

function simplifyRing(ring: Point2D[]): Point2D[] {
  if (ring.length <= 3) return ring;
  const out: Point2D[] = [];
  for (let i = 0; i < ring.length; i++) {
    const pt = ring[i]!;
    if (out.length === 0) {
      out.push(pt);
      continue;
    }
    const prev = out[out.length - 1]!;
    const dx = pt[0] - prev[0];
    const dy = pt[1] - prev[1];
    if (dx * dx + dy * dy >= SIMPLIFY_MIN_DIST2) {
      out.push(pt);
    }
  }
  if (out.length > 3) {
    const first = out[0]!;
    const last = out[out.length - 1]!;
    const dx = first[0] - last[0];
    const dy = first[1] - last[1];
    if (dx * dx + dy * dy < SIMPLIFY_MIN_DIST2) {
      out.pop();
    }
  }
  return out.length >= 3 ? out : ring;
}

function subdivideEdge(
  a: Point2D,
  b: Point2D,
  depth: number,
  amplitude: number,
  seed: number,
  resultPts: Point2D[],
  settings: CoastSettings,
): void {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len = Math.sqrt(dx * dx + dy * dy);
  if (depth === 0 || len < settings.minEdge || amplitude === 0) return;

  const mx = (a[0] + b[0]) * 0.5;
  const my = (a[1] + b[1]) * 0.5;
  const roughness = roughnessAt(seed, mx, my, settings);
  if (roughness < settings.smoothThreshold) return;

  const qx0 = Math.round(a[0] * QUANTUM);
  const qy0 = Math.round(a[1] * QUANTUM);
  const qx1 = Math.round(b[0] * QUANTUM);
  const qy1 = Math.round(b[1] * QUANTUM);
  const segRand = murmurNoise(seed, qx0, qy0, qx1, qy1, depth);
  const disp = (segRand - 0.5) * Math.sqrt(len) * amplitude * roughness;
  const mid: Point2D = [mx + (-dy / len) * disp, my + (dx / len) * disp];

  const nextAmp = amplitude * settings.amplitudeDecay;
  subdivideEdge(a, mid, depth - 1, nextAmp, seed, resultPts, settings);
  resultPts.push(mid);
  subdivideEdge(mid, b, depth - 1, nextAmp, seed, resultPts, settings);
}

function fractalizeLoop(rawRing: Point2D[], seed: number, settings: CoastSettings): FractalizedLoop {
  const points = simplifyRing(rawRing);
  const n = points.length;
  const resultPts: Point2D[] = [];
  const origIndices: number[] = [];

  for (let i = 0; i < n; i++) {
    const a = points[i]!;
    const b = points[(i + 1) % n]!;
    origIndices.push(resultPts.length);
    resultPts.push(a);
    subdivideEdge(a, b, settings.maxDepth, settings.baseAmplitude, seed, resultPts, settings);
  }
  return { points: resultPts, origIndices };
}

function fmt(n: number): string {
  return (Math.round(n * 100) / 100).toString();
}

function buildHybridClosedSvgPath({ points, origIndices }: FractalizedLoop): string {
  const N = points.length;
  const M = origIndices.length;
  if (N < 3 || M < 3) return '';

  const smooth: boolean[] = new Array<boolean>(M);
  for (let i = 0; i < M; i++) {
    const a = origIndices[i]!;
    const b = origIndices[(i + 1) % M]!;
    smooth[i] = (b > a ? b - a : b + N - a) === 1;
  }

  const p0 = points[origIndices[0]!]!;
  const pL = points[origIndices[M - 1]!]!;
  let atMid = smooth[M - 1]!;
  const sx = atMid ? (pL[0] + p0[0]) * 0.5 : p0[0];
  const sy = atMid ? (pL[1] + p0[1]) * 0.5 : p0[1];
  const d: string[] = [`M${fmt(sx)},${fmt(sy)}`];

  for (let i = 0; i < M; i++) {
    const ci = origIndices[i]!;
    const ni = origIndices[(i + 1) % M]!;
    const [cpx, cpy] = points[ci]!;

    if (smooth[i]) {
      const [npx, npy] = points[ni]!;
      const mx = (cpx + npx) * 0.5;
      const my = (cpy + npy) * 0.5;
      d.push(atMid ? `Q${fmt(cpx)},${fmt(cpy)} ${fmt(mx)},${fmt(my)}` : `L${fmt(mx)},${fmt(my)}`);
      atMid = true;
    } else {
      if (atMid) d.push(`L${fmt(cpx)},${fmt(cpy)}`);
      const end = ni > ci ? ni : ni + N;
      for (let j = ci; j < end; j++) {
        const a = points[j % N]!;
        const b = points[(j + 1) % N]!;
        const prev = points[(j - 1 + N) % N]!;
        const nnext = points[(j + 2) % N]!;
        const cp1x = a[0] + (b[0] - prev[0]) / 8;
        const cp1y = a[1] + (b[1] - prev[1]) / 8;
        const cp2x = b[0] - (nnext[0] - a[0]) / 8;
        const cp2y = b[1] - (nnext[1] - a[1]) / 8;
        d.push(
          `C${fmt(cp1x)},${fmt(cp1y)} ${fmt(cp2x)},${fmt(cp2y)} ${fmt(b[0])},${fmt(b[1])}`,
        );
      }
      atMid = false;
    }
  }
  d.push('Z');
  return d.join('');
}

function buildSmoothBasisClosedSvgPath(rawRing: Point2D[]): string {
  const pts = simplifyRing(rawRing);
  const M = pts.length;
  if (M < 3) return '';
  const p0 = pts[0]!;
  const pL = pts[M - 1]!;
  const sx = (pL[0] + p0[0]) * 0.5;
  const sy = (pL[1] + p0[1]) * 0.5;
  const d: string[] = [`M${fmt(sx)},${fmt(sy)}`];
  for (let i = 0; i < M; i++) {
    const [cpx, cpy] = pts[i]!;
    const [npx, npy] = pts[(i + 1) % M]!;
    const mx = (cpx + npx) * 0.5;
    const my = (cpy + npy) * 0.5;
    d.push(`Q${fmt(cpx)},${fmt(cpy)} ${fmt(mx)},${fmt(my)}`);
  }
  d.push('Z');
  return d.join('');
}

function isPolarBorderEdge(a: Point2D, b: Point2D, h: number): boolean {
  const eps = 0.25;
  return (a[1] <= eps && b[1] <= eps) || (a[1] >= h - eps && b[1] >= h - eps);
}

function periodicFieldAt(seed: number, x: number, y: number, width: number, scale: number): number {
  const cols = Math.max(1, Math.round(width / scale));
  const effScale = width / cols;
  const fx = x / effScale;
  const fy = y / scale;
  const ix = Math.floor(fx);
  const iy = Math.floor(fy);
  const tx = fx - ix;
  const ty = fy - iy;
  const sx = tx * tx * (3 - 2 * tx);
  const sy = ty * ty * (3 - 2 * ty);
  const ix0 = ((ix % cols) + cols) % cols;
  const ix1 = (ix0 + 1) % cols;

  const v00 = murmurNoise(seed, ix0, iy);
  const v10 = murmurNoise(seed, ix1, iy);
  const v01 = murmurNoise(seed, ix0, iy + 1);
  const v11 = murmurNoise(seed, ix1, iy + 1);
  const top = v00 * (1 - sx) + v10 * sx;
  const bottom = v01 * (1 - sx) + v11 * sx;
  return top * (1 - sy) + bottom * sy;
}

/**
 * Subtle, calm symmetric curve on shared ocean cell edges (max 4.5px gentle arc),
 * periodic modulo `width` so edges straddling `x = 0 <-> width` match identically across copies!
 */
function seaEdgeControlPoint(
  seed: number,
  a: Point2D,
  b: Point2D,
  width: number,
  height: number,
): Point2D | null {
  if (isPolarBorderEdge(a, b, height)) return null;
  const wa = ((a[0] % width) + width) % width;
  const wb = ((b[0] % width) + width) % width;
  const firstIsA = Math.abs(wa - wb) > 1e-3 ? wa < wb : a[1] <= b[1];
  const p0 = firstIsA ? a : b;
  const p1 = firstIsA ? b : a;
  const dx = p1[0] - p0[0];
  const dy = p1[1] - p0[1];
  const len = Math.hypot(dx, dy);
  if (len < 6) return null;

  const mx = (p0[0] + p1[0]) * 0.5;
  const my = (p0[1] + p1[1]) * 0.5;
  const wave = (periodicFieldAt(seed ^ 0x51a7_3d91, mx, my, width, 120) - 0.5) * 2;
  const offset = wave * Math.min(len * 0.12, 4.5);
  const cx = mx + (-dy / len) * offset;
  const cy = Math.max(0.5, Math.min(height - 0.5, my + (dx / len) * offset));
  return [cx, cy];
}

export function compileSurfaceGeometry(surface: PlanetSurfaceSnapshot): CompiledSurfaceGeometry {
  const oceanLoopsSvg: string[] = [];
  for (let i = 0; i < surface.land_loops.length; i++) {
    const loopSeed = murmurNoise(surface.seed, 101, i);
    const frac = fractalizeLoop(surface.land_loops[i]!, (loopSeed * 0x7fffffff) | 0, OCEAN_COAST_SETTINGS);
    const svg = buildHybridClosedSvgPath(frac);
    if (svg) oceanLoopsSvg.push(svg);
  }

  const lakeLoopsSvg: string[] = [];
  for (let i = 0; i < surface.lake_loops.length; i++) {
    const loopSeed = murmurNoise(surface.seed, 202, i);
    const frac = fractalizeLoop(surface.lake_loops[i]!, (loopSeed * 0x7fffffff) | 0, LAKE_COAST_SETTINGS);
    const svg = buildHybridClosedSvgPath(frac);
    if (svg) lakeLoopsSvg.push(svg);
  }

  // Apply 2 passes of Chaikin midpoint smoothing before `curveBasisClosed` so the single
  // Shallow/Deep boundary curve glides smoothly along the outer ring of `CoastalSea` cells.
  const smoothRingOnce = (ring: Point2D[]): Point2D[] => {
    if (ring.length < 4) return ring;
    const out: Point2D[] = [];
    for (let i = 0; i < ring.length; i++) {
      const a = ring[i]!;
      const b = ring[(i + 1) % ring.length]!;
      out.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
      out.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
    }
    return out;
  };

  const shelfContourSvg = surface.shelf_loops
    .map((ring) => buildSmoothBasisClosedSvgPath(smoothRingOnce(ring)))
    .filter(Boolean)
    .join(' ');

  const oceanCoastSvgPath = oceanLoopsSvg.join(' ');
  const lakeCoastSvgPath = lakeLoopsSvg.join(' ');
  const landMaskSvgPath = `${oceanCoastSvgPath} ${lakeCoastSvgPath}`.trim();
  const waterMaskSvgPath = `M0,0 H${surface.width} V${surface.height} H0 Z ${landMaskSvgPath}`.trim();

  const landMaskPath = new Path2D(landMaskSvgPath);
  const waterMaskPath = new Path2D(waterMaskSvgPath);
  const oceanCoastPath = new Path2D(oceanCoastSvgPath);
  const lakeCoastPath = new Path2D(lakeCoastSvgPath);
  const shelfContourPath = new Path2D(shelfContourSvg);

  const regionBorderPath = new Path2D();
  for (const [a, b] of surface.region_borders) {
    regionBorderPath.moveTo(a[0], a[1]);
    regionBorderPath.lineTo(b[0], b[1]);
  }

  const tilePaths: Path2D[] = new Array<Path2D>(surface.tiles.length);
  const landInternalGridPath = new Path2D();
  const seaInternalGridPath = new Path2D();
  const regionSolidPaths: Path2D[] = Array.from(
    { length: surface.regions.length },
    () => new Path2D(),
  );
  const regionPoliticalBorderPaths: Path2D[] = Array.from(
    { length: surface.regions.length },
    () => new Path2D(),
  );

  const vKey = (pt: Point2D): string => `${Math.round(pt[0] * 10)},${Math.round(pt[1] * 10)}`;
  const ensureClockwise = (poly: ReadonlyArray<Point2D>): Point2D[] => {
    let area2 = 0;
    for (let i = 0; i < poly.length; i++) {
      const a = poly[i]!;
      const b = poly[(i + 1) % poly.length]!;
      area2 += a[0] * b[1] - b[0] * a[1];
    }
    return area2 >= 0 ? [...poly] : [...poly].reverse();
  };

  interface DirectedLandEdge {
    ka: string;
    kb: string;
    a: Point2D;
    b: Point2D;
    regionId: number;
  }
  const landEdgeMap = new Map<string, DirectedLandEdge>();
  const seenSeaEdges = new Set<string>();

  const cols = 24;
  const rows = 14;
  const buckets: SurfacePolygonTile[][] = Array.from({ length: cols * rows }, () => []);
  const landTiles: SurfacePolygonTile[] = [];
  const waterTiles: SurfacePolygonTile[] = [];

  for (const tile of surface.tiles) {
    const isWater = isWaterTags(tile.tags);
    if (isWater) waterTiles.push(tile);
    else landTiles.push(tile);

    const tp = new Path2D();
    const poly = tile.polygon;
    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;

    if (poly.length > 0) {
      const [sx, sy] = poly[0]!;
      tp.moveTo(sx, sy);

      for (let i = 0; i < poly.length; i++) {
        const cur = poly[i]!;
        const nxt = poly[(i + 1) % poly.length]!;
        if (cur[0] < minX) minX = cur[0];
        if (cur[0] > maxX) maxX = cur[0];
        if (cur[1] < minY) minY = cur[1];
        if (cur[1] > maxY) maxY = cur[1];

        if (isWater) {
          const ctrl = seaEdgeControlPoint(surface.seed, cur, nxt, surface.width, surface.height);
          const wa = (((Math.round(cur[0] * 10) % Math.round(surface.width * 10)) + Math.round(surface.width * 10)) % Math.round(surface.width * 10));
          const wb = (((Math.round(nxt[0] * 10) % Math.round(surface.width * 10)) + Math.round(surface.width * 10)) % Math.round(surface.width * 10));
          const k1 = `${wa},${Math.round(cur[1] * 10)}`;
          const k2 = `${wb},${Math.round(nxt[1] * 10)}`;
          const seaEdgeKey = k1 < k2 ? `${k1}|${k2}` : `${k2}|${k1}`;
          const shouldRecordSeaEdge =
            !seenSeaEdges.has(seaEdgeKey) && !isPolarBorderEdge(cur, nxt, surface.height);
          if (shouldRecordSeaEdge) {
            seenSeaEdges.add(seaEdgeKey);
            seaInternalGridPath.moveTo(cur[0], cur[1]);
            if (ctrl) {
              seaInternalGridPath.quadraticCurveTo(ctrl[0], ctrl[1], nxt[0], nxt[1]);
            } else {
              seaInternalGridPath.lineTo(nxt[0], nxt[1]);
            }
          }

          if (ctrl) {
            tp.quadraticCurveTo(ctrl[0], ctrl[1], nxt[0], nxt[1]);
            if (ctrl[0] < minX) minX = ctrl[0];
            if (ctrl[0] > maxX) maxX = ctrl[0];
            if (ctrl[1] < minY) minY = ctrl[1];
            if (ctrl[1] > maxY) maxY = ctrl[1];
            continue;
          }
        }
        tp.lineTo(nxt[0], nxt[1]);
      }
      tp.closePath();

      if (!isWater && poly.length >= 3) {
        const cwPoly = ensureClockwise(poly);
        for (let i = 0; i < cwPoly.length; i++) {
          const a = cwPoly[i]!;
          const b = cwPoly[(i + 1) % cwPoly.length]!;
          const ka = vKey(a);
          const kb = vKey(b);
          if (ka === kb) continue;
          landEdgeMap.set(`${ka}->${kb}`, {
            ka,
            kb,
            a,
            b,
            regionId: tile.region_id,
          });
        }
      }
    }
    tilePaths[tile.index] = tp;

    const pad = isWater ? 5 : 1;
    const rawC0 = Math.floor(((minX - pad) / surface.width) * cols);
    const rawC1 = Math.floor(((maxX + pad) / surface.width) * cols);
    const r0 = Math.max(0, Math.min(rows - 1, Math.floor(((minY - pad) / surface.height) * rows)));
    const r1 = Math.max(0, Math.min(rows - 1, Math.floor(((maxY + pad) / surface.height) * rows)));
    for (let r = r0; r <= r1; r++) {
      const seenCols = new Set<number>();
      for (let rc = rawC0; rc <= rawC1; rc++) {
        const wc = ((rc % cols) + cols) % cols;
        if (seenCols.has(wc)) continue;
        seenCols.add(wc);
        buckets[r * cols + wc]!.push(tile);
      }
    }
  }

  const hitTestTile: CompiledSurfaceGeometry['hitTestTile'] = (ctx, rawMapX, mapY) => {
    if (mapY < 0 || mapY > surface.height) {
      return null;
    }
    const w = surface.width;
    const mapX = ((rawMapX % w) + w) % w;
    ctx.save();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    const inLand =
      ctx.isPointInPath(landMaskPath, mapX, mapY, 'evenodd')
      || (mapX < 80 && ctx.isPointInPath(landMaskPath, mapX + w, mapY, 'evenodd'))
      || (mapX > w - 80 && ctx.isPointInPath(landMaskPath, mapX - w, mapY, 'evenodd'));

    const wantWater = !inLand;
    const col = Math.max(0, Math.min(cols - 1, Math.floor((mapX / w) * cols)));
    const row = Math.max(0, Math.min(rows - 1, Math.floor((mapY / surface.height) * rows)));
    const candidates = buckets[row * cols + col]!;

    for (const cand of candidates) {
      if (isWaterTags(cand.tags) !== wantWater) continue;
      const tPath = tilePaths[cand.index];
      if (!tPath) continue;
      if (
        ctx.isPointInPath(tPath, mapX, mapY)
        || (mapX < 96 && ctx.isPointInPath(tPath, mapX + w, mapY))
        || (mapX > w - 96 && ctx.isPointInPath(tPath, mapX - w, mapY))
      ) {
        ctx.restore();
        return cand;
      }
    }
    ctx.restore();

    const sameDomainBucket = candidates.filter((c) => isWaterTags(c.tags) === wantWater);
    const pool = sameDomainBucket.length > 0 ? sameDomainBucket : wantWater ? waterTiles : landTiles;
    let nearest: SurfacePolygonTile | null = null;
    let bestD2 = Infinity;
    for (const cand of pool) {
      const rawDx = Math.abs(cand.center[0] - mapX);
      const dx = Math.min(rawDx, w - rawDx);
      const dy = cand.center[1] - mapY;
      const d2 = dx * dx + dy * dy;
      if (d2 < bestD2) {
        bestD2 = d2;
        nearest = cand;
      }
    }
    return nearest;
  };

  // Dissolve internal edges within each macro-region so Level 1 has ZERO single-tile edges,
  // and record each internal same-region edge ONCE in `landInternalGridPath` for Level 2's thin mesh!
  const exteriorByRegion: Array<Map<string, DirectedLandEdge[]>> = Array.from(
    { length: surface.regions.length },
    () => new Map<string, DirectedLandEdge[]>(),
  );

  for (const edge of landEdgeMap.values()) {
    const rev = landEdgeMap.get(`${edge.kb}->${edge.ka}`);
    if (rev && rev.regionId === edge.regionId) {
      if (edge.ka < edge.kb) {
        landInternalGridPath.moveTo(edge.a[0], edge.a[1]);
        landInternalGridPath.lineTo(edge.b[0], edge.b[1]);
      }
      continue;
    }

    const bucketMap = exteriorByRegion[edge.regionId];
    if (bucketMap) {
      let arr = bucketMap.get(edge.ka);
      if (!arr) {
        arr = [];
        bucketMap.set(edge.ka, arr);
      }
      arr.push(edge);
    }

    if (rev && rev.regionId !== edge.regionId) {
      const pBorder = regionPoliticalBorderPaths[edge.regionId];
      if (pBorder) {
        pBorder.moveTo(edge.a[0], edge.a[1]);
        pBorder.lineTo(edge.b[0], edge.b[1]);
      }
    } else {
      // Coastal chord `a -> b` (clockwise around land -> outward normal into water is `[dy, -dx]`):
      // append a clockwise 7px coastal fringe quad so `regionSolidPaths` reaches every fractal cape!
      const dx = edge.b[0] - edge.a[0];
      const dy = edge.b[1] - edge.a[1];
      const len = Math.hypot(dx, dy);
      const rSolid = regionSolidPaths[edge.regionId];
      if (len > 1e-3 && rSolid) {
        const nx = (dy / len) * 7.5;
        const ny = (-dx / len) * 7.5;
        rSolid.moveTo(edge.a[0], edge.a[1]);
        rSolid.lineTo(edge.a[0] + nx, edge.a[1] + ny);
        rSolid.lineTo(edge.b[0] + nx, edge.b[1] + ny);
        rSolid.lineTo(edge.b[0], edge.b[1]);
        rSolid.closePath();
      }
    }
  }

  for (let regId = 0; regId < exteriorByRegion.length; regId++) {
    const adj = exteriorByRegion[regId]!;
    const rSolid = regionSolidPaths[regId]!;
    for (const [startKey, list] of adj.entries()) {
      while (list.length > 0) {
        const first = list.pop()!;
        rSolid.moveTo(first.a[0], first.a[1]);
        rSolid.lineTo(first.b[0], first.b[1]);
        let curKey = first.kb;
        let guard = 0;
        while (curKey !== startKey && guard++ < 4096) {
          const nextArr = adj.get(curKey);
          if (!nextArr || nextArr.length === 0) break;
          const nextEdge = nextArr.pop()!;
          rSolid.lineTo(nextEdge.b[0], nextEdge.b[1]);
          curKey = nextEdge.kb;
        }
        rSolid.closePath();
      }
    }
  }

  return {
    landMaskSvgPath,
    oceanCoastSvgPath,
    lakeCoastSvgPath,
    landMaskPath,
    waterMaskPath,
    oceanCoastPath,
    lakeCoastPath,
    shelfContourPath,
    regionBorderPath,
    landInternalGridPath,
    seaInternalGridPath,
    tilePaths,
    regionSolidPaths,
    regionPoliticalBorderPaths,
    hitTestTile,
  };
}
