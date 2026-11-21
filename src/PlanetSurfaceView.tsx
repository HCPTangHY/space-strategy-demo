import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { compileSurfaceGeometry } from './coastline';
import { isWaterTags, TILE_TAG_LABELS } from './game';
import type {
  OccupiedTileSnapshot,
  PlanetSnapshot,
  PlanetSurfaceSnapshot,
  SectorSnapshot,
  SurfacePolygonTile,
  TileTagId,
} from './game';

export type MapInteractionLevel = 'region' | 'tile';

export const DETAIL_ZOOM_RATIO = 1.65;

export interface MapFocusTarget {
  seq: number;
  mapX: number;
  mapY: number;
  targetRatio?: number;
}

function deriveTagTerrainRgb(tags: readonly TileTagId[]): [number, number, number] {
  if (tags.includes('mountains')) return [132, 138, 146];
  if (tags.includes('hills')) {
    return tags.includes('forest') ? [104, 132, 98] : [138, 134, 102];
  }
  if (tags.includes('tundra')) {
    return tags.includes('forest') ? [116, 146, 138] : [152, 166, 174];
  }
  if (tags.includes('arid')) return [174, 148, 106];
  if (tags.includes('forest')) return [78, 122, 92];
  return [118, 152, 112];
}

function macroRegionFillStyle(regionRgb: [number, number, number]): string {
  return `rgb(${regionRgb[0]}, ${regionRgb[1]}, ${regionRgb[2]})`;
}

function detailTerrainFillStyle(
  tile: SurfacePolygonTile,
  regionRgb: [number, number, number],
): string {
  const tBase = deriveTagTerrainRgb(tile.tags);
  const elevFactor = (tile.elevation - 20) / 80;
  const shade = 0.88 + elevFactor * 0.24;
  const r = Math.min(255, Math.round((tBase[0] * 0.86 + regionRgb[0] * 0.14) * shade));
  const g = Math.min(255, Math.round((tBase[1] * 0.86 + regionRgb[1] * 0.14) * shade));
  const b = Math.min(255, Math.round((tBase[2] * 0.86 + regionRgb[2] * 0.14) * shade));
  return `rgb(${r}, ${g}, ${b})`;
}

function formatTagSummary(tags: readonly TileTagId[]): string {
  return tags
    .filter((t) => t !== 'land' && t !== 'sea')
    .map((t) => TILE_TAG_LABELS[t])
    .join(' · ');
}

function buildPeriodicClipPath(baseSvg: string, width: number): Path2D {
  const path = new Path2D();
  if (!baseSvg) return path;
  const base = new Path2D(baseSvg);
  for (const shiftX of [-width, 0, width]) {
    const m = new DOMMatrix().translate(shiftX, 0);
    path.addPath(base, m);
  }
  return path;
}

/**
 * Renders a horizontally tileable `surface.width × surface.height` offscreen base map
 * by drawing periodic geometry at `[-W, 0, +W]` with zero left/right frame lines.
 * - `level === 'region'`: flat macro-region colors + coastlines + bold region borders (no terrain icons / cell grid).
 * - `level === 'tile'`: natural terrain colors + mountain/hill/forest icons + fine Voronoi grid + region borders.
 */
function renderOffscreenBaseMap(
  surface: PlanetSurfaceSnapshot,
  geo: ReturnType<typeof compileSurfaceGeometry>,
  level: MapInteractionLevel,
): HTMLCanvasElement | null {
  if (typeof document === 'undefined') return null;
  const scale = 2;
  const off = document.createElement('canvas');
  off.width = Math.round(surface.width * scale);
  off.height = Math.round(surface.height * scale);
  const ctx = off.getContext('2d');
  if (!ctx) return null;

  ctx.setTransform(scale, 0, 0, scale, 0, 0);

  const w = surface.width;
  const h = surface.height;
  const periodicLandMask = buildPeriodicClipPath(geo.landMaskSvgPath, w);
  const periodicWaterMask = new Path2D();
  periodicWaterMask.rect(-w, 0, w * 3, h);
  if (geo.landMaskSvgPath) {
    periodicWaterMask.addPath(periodicLandMask);
  }

  // 1. Deep ocean background + periodic shallow continental shelf contour (no grid baked here).
  ctx.fillStyle = '#12283f';
  ctx.fillRect(0, 0, w, h);

  ctx.save();
  ctx.clip(periodicWaterMask, 'evenodd');

  ctx.save();
  ctx.filter = 'blur(5px)';
  ctx.fillStyle = level === 'tile' ? '#25557a' : '#1f4666';
  for (const shiftX of [-w, 0, w]) {
    ctx.save();
    ctx.translate(shiftX, 0);
    ctx.fill(geo.shelfContourPath, 'evenodd');
    ctx.restore();
  }
  ctx.restore();

  for (const shiftX of [-w, 0, w]) {
    ctx.save();
    ctx.translate(shiftX, 0);

    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.strokeStyle = 'rgba(132, 194, 224, 0.22)';
    ctx.lineWidth = 5;
    ctx.stroke(geo.oceanCoastPath);

    if (geo.lakeCoastSvgPath.length > 0) {
      ctx.fillStyle = '#224c6e';
      ctx.fill(geo.lakeCoastPath, 'evenodd');
    }

    ctx.restore();
  }
  ctx.restore();

  // 2. Land domain:
  // - Level 1 (`region`): dissolved solid macro-region fills ONLY (zero single-tile polygons/edges!) + bold political borders.
  // - Level 2 (`tile`): natural terrain biome fills + relief icons (grid & political borders drawn crisp in screen space).
  ctx.save();
  ctx.clip(periodicLandMask, 'evenodd');
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';

  for (const shiftX of [-w, 0, w]) {
    ctx.save();
    ctx.translate(shiftX, 0);

    if (level === 'region') {
      for (const region of surface.regions) {
        if (region.is_ocean) continue;
        const solidPath = geo.regionSolidPaths[region.id];
        if (!solidPath) continue;
        ctx.fillStyle = macroRegionFillStyle(region.color_rgb);
        ctx.fill(solidPath, 'nonzero');
      }

      // Prominent double-stroke macro-region political network in Level 1
      ctx.strokeStyle = 'rgba(238, 244, 250, 0.30)';
      ctx.lineWidth = 3.4;
      ctx.stroke(geo.regionBorderPath);

      ctx.strokeStyle = '#0a1320';
      ctx.lineWidth = 2.0;
      ctx.stroke(geo.regionBorderPath);
    } else {
      for (const tile of surface.tiles) {
        if (isWaterTags(tile.tags) || !tile.tags.includes('coastal')) continue;
        const regionRgb = surface.regions[tile.region_id]?.color_rgb ?? [115, 145, 125];
        const color = detailTerrainFillStyle(tile, regionRgb);
        const path = geo.tilePaths[tile.index];
        if (!path) continue;
        ctx.strokeStyle = color;
        ctx.lineWidth = 9;
        ctx.stroke(path);
      }

      for (const tile of surface.tiles) {
        if (isWaterTags(tile.tags)) continue;
        const regionRgb = surface.regions[tile.region_id]?.color_rgb ?? [115, 145, 125];
        const color = detailTerrainFillStyle(tile, regionRgb);
        const path = geo.tilePaths[tile.index];
        if (!path) continue;
        ctx.fillStyle = color;
        ctx.fill(path);
        ctx.strokeStyle = color;
        ctx.lineWidth = 0.55;
        ctx.stroke(path);
      }

      for (const tile of surface.tiles) {
        if (isWaterTags(tile.tags)) continue;
        const [cx, cy] = tile.center;
        const hasMountains = tile.tags.includes('mountains');
        const hasHills = tile.tags.includes('hills');
        const hasForest = tile.tags.includes('forest');

        if (hasMountains) {
          ctx.strokeStyle = 'rgba(20, 28, 40, 0.48)';
          ctx.lineWidth = 1.25;
          ctx.beginPath();
          ctx.moveTo(cx - 4.8, cy + 2.6);
          ctx.lineTo(cx, cy - 3.8);
          ctx.lineTo(cx + 4.8, cy + 2.6);
          ctx.stroke();
        } else if (hasHills) {
          const ox = hasForest ? -2.6 : 0;
          ctx.strokeStyle = 'rgba(24, 34, 46, 0.34)';
          ctx.lineWidth = 1.05;
          ctx.beginPath();
          ctx.arc(cx + ox, cy + 1.6, 3.4, Math.PI * 1.12, Math.PI * 1.88);
          ctx.stroke();
        }

        if (hasForest) {
          const ox = hasHills ? 3.2 : 0;
          ctx.fillStyle = 'rgba(22, 46, 34, 0.36)';
          ctx.beginPath();
          ctx.moveTo(cx + ox, cy - 3.3);
          ctx.lineTo(cx + ox - 2.7, cy + 2.1);
          ctx.lineTo(cx + ox + 2.7, cy + 2.1);
          ctx.closePath();
          ctx.fill();
        }
      }
    }

    ctx.restore();
  }

  ctx.restore();

  // 3. Crisp shore outlines across `[-W, 0, +W]` (no vertical border lines!).
  for (const shiftX of [-w, 0, w]) {
    ctx.save();
    ctx.translate(shiftX, 0);
    if (geo.lakeCoastSvgPath.length > 0) {
      ctx.strokeStyle = '#102030';
      ctx.lineWidth = 1.2;
      ctx.stroke(geo.lakeCoastPath);
    }
    ctx.strokeStyle = '#0c1826';
    ctx.lineWidth = 1.55;
    ctx.lineJoin = 'round';
    ctx.stroke(geo.oceanCoastPath);
    ctx.restore();
  }

  return off;
}

interface Camera {
  zoom: number;
  panX: number;
  panY: number;
}

function getCoverZoom(viewportW: number, viewportH: number, mapW: number, mapH: number): number {
  return Math.max(viewportW / mapW, viewportH / mapH);
}

/**
 * Strictly clamps zoom >= `coverZoom`, locks vertical `panY` inside `[viewportH - mapH*zoom, 0]`,
 * and wraps horizontal `panX` into `(-mapW*zoom, 0]` so ZERO blank dark space can ever appear!
 */
function clampCamera(
  raw: Camera,
  viewportW: number,
  viewportH: number,
  mapW: number,
  mapH: number,
): Camera {
  const minZoom = getCoverZoom(viewportW, viewportH, mapW, mapH);
  const maxZoom = minZoom * 6.0;
  const zoom = Math.min(maxZoom, Math.max(minZoom, raw.zoom));
  const scaledW = mapW * zoom;
  const scaledH = mapH * zoom;
  const minPanY = Math.min(0, viewportH - scaledH);
  const panY = Math.min(0, Math.max(minPanY, raw.panY));
  let panX = raw.panX % scaledW;
  if (panX > 0) panX -= scaledW;
  if (Object.is(panX, -0)) panX = 0;
  return { zoom, panX, panY };
}

function computeDefaultCoverCamera(
  viewportW: number,
  viewportH: number,
  mapW: number,
  mapH: number,
): Camera {
  const zoom = getCoverZoom(viewportW, viewportH, mapW, mapH);
  const scaledW = mapW * zoom;
  const scaledH = mapH * zoom;
  return clampCamera(
    {
      zoom,
      panX: (viewportW - scaledW) * 0.5,
      panY: (viewportH - scaledH) * 0.5,
    },
    viewportW,
    viewportH,
    mapW,
    mapH,
  );
}

export interface PlanetSurfaceViewProps {
  planet: PlanetSnapshot;
  surface: PlanetSurfaceSnapshot;
  selectedRegionId: number | null;
  selectedTileIndex: number | null;
  onSelectRegion: (regionId: number) => void;
  onSelectTile: (tileIndex: number) => void;
  showGrid: boolean;
  focusTarget?: MapFocusTarget | null;
  onLevelChange?: (level: MapInteractionLevel, relativePercent: number) => void;
}

export function PlanetSurfaceView({
  planet,
  surface,
  selectedRegionId,
  selectedTileIndex,
  onSelectRegion,
  onSelectTile,
  showGrid,
  focusTarget = null,
  onLevelChange,
}: PlanetSurfaceViewProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const cameraRef = useRef<Camera | null>(null);
  const appliedFocusSeqRef = useRef<number | null>(null);
  const lastLevelNoteRef = useRef<{ level: MapInteractionLevel; pct: number } | null>(null);
  const dragRef = useRef<{
    active: boolean;
    moved: boolean;
    startX: number;
    startY: number;
    origPanX: number;
    origPanY: number;
  } | null>(null);

  const [hoveredInfo, setHoveredInfo] = useState<string | null>(null);
  const hoveredTileRef = useRef<number | null>(null);
  const hoveredRegionRef = useRef<number | null>(null);
  const [zoomPercent, setZoomPercent] = useState<number>(100);
  const [activeLevel, setActiveLevel] = useState<MapInteractionLevel>('region');

  const geo = useMemo(() => compileSurfaceGeometry(surface), [surface]);

  const periodicLandMask = useMemo(
    () => buildPeriodicClipPath(geo.landMaskSvgPath, surface.width),
    [geo.landMaskSvgPath, surface.width],
  );
  const periodicWaterMask = useMemo(() => {
    const p = new Path2D();
    p.rect(-surface.width * 2, 0, surface.width * 5, surface.height);
    if (geo.landMaskSvgPath) {
      p.addPath(periodicLandMask);
    }
    return p;
  }, [geo.landMaskSvgPath, periodicLandMask, surface.height, surface.width]);

  const macroBaseCanvas = useMemo(
    () => renderOffscreenBaseMap(surface, geo, 'region'),
    [surface, geo],
  );
  const detailBaseCanvas = useMemo(
    () => renderOffscreenBaseMap(surface, geo, 'tile'),
    [surface, geo],
  );

  const occupiedByIndex = useMemo(() => {
    const map = new Map<number, OccupiedTileSnapshot>();
    for (const occ of planet.occupied_tiles) {
      map.set(occ.index, occ);
    }
    return map;
  }, [planet.occupied_tiles]);

  const sectorByRegionId = useMemo(() => {
    const map = new Map<number, SectorSnapshot>();
    for (const sec of planet.sectors) {
      if (!map.has(sec.geo_region_id)) {
        map.set(sec.geo_region_id, sec);
      }
    }
    return map;
  }, [planet.sectors]);

  const sectorById = useMemo(() => {
    const map = new Map<number, SectorSnapshot>();
    for (const sec of planet.sectors) {
      map.set(sec.id, sec);
    }
    return map;
  }, [planet.sectors]);

  const unclaimedLandMask = useMemo(() => {
    const p = new Path2D();
    for (const tile of surface.tiles) {
      if (isWaterTags(tile.tags)) continue;
      if ((planet.tile_sectors[tile.index] ?? null) === null) {
        const tp = geo.tilePaths[tile.index];
        if (tp) p.addPath(tp);
      }
    }
    return p;
  }, [geo.tilePaths, planet.tile_sectors, surface.tiles]);

  const syncZoomReadout = useCallback(
    (cam: Camera, vw: number, vh: number) => {
      const cover = getCoverZoom(vw, vh, surface.width, surface.height);
      const ratio = cam.zoom / Math.max(cover, 1e-6);
      const pct = Math.max(100, Math.round(ratio * 100));
      const lvl: MapInteractionLevel = ratio >= DETAIL_ZOOM_RATIO ? 'tile' : 'region';
      setZoomPercent(pct);
      setActiveLevel(lvl);
      const prev = lastLevelNoteRef.current;
      if (!prev || prev.level !== lvl || prev.pct !== pct) {
        lastLevelNoteRef.current = { level: lvl, pct };
        onLevelChange?.(lvl, pct);
      }
    },
    [onLevelChange, surface.height, surface.width],
  );

  const drawScene = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas || !macroBaseCanvas || !detailBaseCanvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const { width, height } = canvas.getBoundingClientRect();
    if (width === 0 || height === 0) return;

    if (!cameraRef.current) {
      cameraRef.current = computeDefaultCoverCamera(width, height, surface.width, surface.height);
    } else {
      cameraRef.current = clampCamera(
        cameraRef.current,
        width,
        height,
        surface.width,
        surface.height,
      );
    }
    const cam = cameraRef.current;
    const cover = getCoverZoom(width, height, surface.width, surface.height);
    const ratio = cam.zoom / Math.max(cover, 1e-6);
    const level: MapInteractionLevel = ratio >= DETAIL_ZOOM_RATIO ? 'tile' : 'region';
    const baseCanvas = level === 'tile' ? detailBaseCanvas : macroBaseCanvas;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const targetW = Math.round(width * dpr);
    const targetH = Math.round(height * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = '#12283f';
    ctx.fillRect(0, 0, width, height);

    const scaledW = surface.width * cam.zoom;

    // 1. Tile seamless offscreen base map horizontally across the viewport (zero seams, zero blank edges).
    for (let ox = cam.panX; ox < width + 2; ox += scaledW) {
      ctx.drawImage(baseCanvas, ox, cam.panY, scaledW, surface.height * cam.zoom);
    }

    // 2. Dynamic vector overlays, building icons, and labels repeated across visible horizontal copies
    //    (including ±1 neighbor copy so cells straddling `x = 0 <-> width` render seamlessly).
    for (let ox = cam.panX - scaledW; ox < width + scaledW; ox += scaledW) {
      if (ox + scaledW < -120 * cam.zoom || ox > width + 120 * cam.zoom) continue;

      ctx.save();
      ctx.translate(ox, cam.panY);
      ctx.scale(cam.zoom, cam.zoom);

      if (level === 'region') {
        // Level 1: ZERO single-tile edges! Highlight uses dissolved region fill + ONLY outer political/coast borders.
        const drawRegionHighlight = (
          regId: number,
          fill: string,
          stroke: string,
          widthPx: number,
        ) => {
          const reg = surface.regions[regId];
          if (!reg || reg.is_ocean) return;
          const solidPath = geo.regionSolidPaths[regId];
          const borderPath = geo.regionPoliticalBorderPaths[regId];
          if (!solidPath) return;

          ctx.save();
          ctx.clip(periodicLandMask, 'evenodd');
          ctx.fillStyle = fill;
          ctx.fill(solidPath, 'nonzero');
          if (borderPath) {
            ctx.strokeStyle = stroke;
            ctx.lineWidth = widthPx / cam.zoom;
            ctx.lineJoin = 'round';
            ctx.lineCap = 'round';
            ctx.stroke(borderPath);
          }
          ctx.restore();

          ctx.save();
          ctx.clip(solidPath, 'nonzero');
          ctx.strokeStyle = stroke;
          ctx.lineWidth = widthPx / cam.zoom;
          ctx.lineJoin = 'round';
          ctx.lineCap = 'round';
          ctx.stroke(geo.oceanCoastPath);
          if (geo.lakeCoastSvgPath.length > 0) {
            ctx.stroke(geo.lakeCoastPath);
          }
          ctx.restore();
        };

        // Dim completely unclaimed frontier regions in Level 1 so colonized regions stand out clearly
        ctx.save();
        ctx.clip(periodicLandMask, 'evenodd');
        for (const region of surface.regions) {
          if (region.is_ocean) continue;
          const regSummary = planet.region_summaries[region.id];
          if (regSummary && regSummary.claimed_land_tiles === 0) {
            const solidPath = geo.regionSolidPaths[region.id];
            if (solidPath) {
              ctx.fillStyle = 'rgba(7, 13, 22, 0.44)';
              ctx.fill(solidPath, 'nonzero');
            }
          } else if (
            regSummary
            && regSummary.claimed_land_tiles > 0
            && regSummary.claimed_land_tiles < region.land_tile_count
          ) {
            ctx.fillStyle = 'rgba(7, 13, 22, 0.38)';
            ctx.fill(unclaimedLandMask);
          }
        }
        ctx.restore();

        const hovReg = hoveredRegionRef.current;
        if (hovReg !== null && hovReg !== selectedRegionId) {
          drawRegionHighlight(
            hovReg,
            'rgba(237, 241, 245, 0.16)',
            'rgba(237, 241, 245, 0.78)',
            2.0,
          );
        }
        if (selectedRegionId !== null) {
          drawRegionHighlight(
            selectedRegionId,
            'rgba(147, 184, 203, 0.22)',
            '#edf1f5',
            2.5,
          );
        }

        // Major landmarks (Capital, Regional Cores & unbuilt Core Candidate Sites) + Macro Region Nameplates
        const markerR = Math.max(4.6, 6.2 / Math.sqrt(cam.zoom));
        for (const tile of surface.tiles) {
          if (!tile.tags.includes('core_site')) continue;
          const occ = occupiedByIndex.get(tile.index);
          if (occ?.primary) continue;
          const [cx, cy] = tile.center;
          const r = markerR * 0.85;
          ctx.beginPath();
          ctx.moveTo(cx, cy - r);
          ctx.lineTo(cx + r, cy);
          ctx.lineTo(cx, cy + r);
          ctx.lineTo(cx - r, cy);
          ctx.closePath();
          ctx.fillStyle = 'rgba(14, 22, 34, 0.65)';
          ctx.fill();
          ctx.lineWidth = 1.35 / cam.zoom;
          ctx.strokeStyle = 'rgba(230, 193, 122, 0.78)';
          ctx.stroke();
        }
        for (const occ of occupiedByIndex.values()) {
          const tile = surface.tiles[occ.index];
          if (!tile) continue;
          const pTags = occ.primary?.tags ?? [];
          const isCapital = pTags.includes('planetary_capital');
          const isRegionalCore = !isCapital && pTags.includes('regional_core');
          if (!isCapital && !isRegionalCore) continue;
          const [cx, cy] = tile.center;
          if (isCapital) {
            const r = markerR * 1.25;
            ctx.beginPath();
            ctx.moveTo(cx, cy - r);
            ctx.lineTo(cx + r, cy);
            ctx.lineTo(cx, cy + r);
            ctx.lineTo(cx - r, cy);
            ctx.closePath();
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.8 / cam.zoom;
            ctx.strokeStyle = '#e6c17a';
            ctx.stroke();
          } else {
            const half = markerR * 0.9;
            ctx.beginPath();
            ctx.rect(cx - half, cy - half, half * 2, half * 2);
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.6 / cam.zoom;
            ctx.strokeStyle = '#edf1f5';
            ctx.stroke();
          }
        }

        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        ctx.lineJoin = 'round';
        for (const region of surface.regions) {
          if (region.is_ocean || region.land_tile_count < 3) continue;
          const shortLabel = region.name.split(' · ')[1] ?? region.name;
          const [rx, rawRy] = region.center;
          const sec = sectorByRegionId.get(region.id);
          const regSummary = planet.region_summaries[region.id];
          const bCount = regSummary
            ? regSummary.primary_used + regSummary.secondary_used
            : 0;

          ctx.font = `600 ${Math.round(12 / Math.sqrt(cam.zoom) * Math.sqrt(cover))}px Inter, "Segoe UI", "PingFang SC", sans-serif`;
          ctx.lineWidth = 3 / cam.zoom;
          ctx.strokeStyle = 'rgba(9, 14, 24, 0.85)';
          ctx.strokeText(shortLabel, rx, rawRy - 5);
          ctx.fillStyle = 'rgba(237, 241, 245, 0.96)';
          ctx.fillText(shortLabel, rx, rawRy - 5);

          if (sec) {
            const partial =
              regSummary && regSummary.claimed_land_tiles < region.land_tile_count
                ? ` · 辖${regSummary.claimed_land_tiles}/${region.land_tile_count}格`
                : '';
            const sub = `${sec.population}人 · ${bCount}建筑${partial}`;
            ctx.font = `500 ${Math.round(9.5 / Math.sqrt(cam.zoom) * Math.sqrt(cover))}px Inter, "Segoe UI", "PingFang SC", sans-serif`;
            ctx.strokeText(sub, rx, rawRy + 8);
            ctx.fillStyle = 'rgba(186, 206, 222, 0.9)';
            ctx.fillText(sub, rx, rawRy + 8);
          } else {
            const sub = `未拓荒荒野 · ${region.land_tile_count}格`;
            ctx.font = `500 ${Math.round(9.0 / Math.sqrt(cam.zoom) * Math.sqrt(cover))}px Inter, "Segoe UI", "PingFang SC", sans-serif`;
            ctx.strokeText(sub, rx, rawRy + 8);
            ctx.fillStyle = 'rgba(148, 166, 184, 0.76)';
            ctx.fillText(sub, rx, rawRy + 8);
          }
        }
      } else {
        // Level 2 (`tile`):
        // 0) Frontier fog tint over unclaimed land tiles (`tile_sectors[idx] === null`) so polity territory pops!
        ctx.save();
        ctx.clip(periodicLandMask, 'evenodd');
        ctx.fillStyle = 'rgba(7, 12, 20, 0.44)';
        ctx.fill(unclaimedLandMask);
        ctx.restore();

        // 1) Whisper-thin single-tile Voronoi subdivision mesh (`showGrid`).
        if (showGrid) {
          ctx.save();
          ctx.clip(periodicWaterMask, 'evenodd');
          ctx.strokeStyle = 'rgba(148, 206, 234, 0.08)';
          ctx.lineWidth = 0.6 / cam.zoom;
          ctx.lineJoin = 'round';
          ctx.stroke(geo.seaInternalGridPath);
          ctx.restore();

          ctx.save();
          ctx.clip(periodicLandMask, 'evenodd');
          ctx.strokeStyle = 'rgba(12, 20, 32, 0.14)';
          ctx.lineWidth = 0.65 / cam.zoom;
          ctx.lineJoin = 'round';
          ctx.stroke(geo.landInternalGridPath);
          ctx.restore();
        }

        // 2) Dominant macro-region / political border network ALWAYS drawn prominently over the thin cell mesh!
        ctx.save();
        ctx.clip(periodicLandMask, 'evenodd');
        ctx.lineJoin = 'round';
        ctx.lineCap = 'round';

        if (selectedRegionId !== null) {
          const selBorder = geo.regionPoliticalBorderPaths[selectedRegionId];
          if (selBorder) {
            ctx.strokeStyle = 'rgba(168, 208, 230, 0.55)';
            ctx.lineWidth = 5.6 / cam.zoom;
            ctx.stroke(selBorder);
          }
        }

        ctx.strokeStyle = 'rgba(240, 246, 252, 0.38)';
        ctx.lineWidth = 4.6 / cam.zoom;
        ctx.stroke(geo.regionBorderPath);

        ctx.strokeStyle = '#08101c';
        ctx.lineWidth = 2.5 / cam.zoom;
        ctx.stroke(geo.regionBorderPath);
        ctx.restore();

        // Crisp coast outline in screen space
        ctx.lineJoin = 'round';
        ctx.strokeStyle = '#0b1624';
        ctx.lineWidth = 1.8 / cam.zoom;
        ctx.stroke(geo.oceanCoastPath);

        // 3) Per-tile hover/selection highlight
        const drawTileOverlay = (
          tileIdx: number,
          fill: string,
          stroke: string,
          widthPx: number,
        ) => {
          const tile = surface.tiles[tileIdx];
          const tPath = geo.tilePaths[tileIdx];
          if (!tile || !tPath) return;
          ctx.save();
          ctx.clip(isWaterTags(tile.tags) ? periodicWaterMask : periodicLandMask, 'evenodd');
          ctx.fillStyle = fill;
          ctx.fill(tPath);
          ctx.strokeStyle = stroke;
          ctx.lineWidth = widthPx / cam.zoom;
          ctx.stroke(tPath);
          ctx.restore();
        };

        const hoveredIdx = hoveredTileRef.current;
        if (hoveredIdx !== null && hoveredIdx !== selectedTileIndex) {
          drawTileOverlay(
            hoveredIdx,
            'rgba(237, 241, 245, 0.18)',
            'rgba(237, 241, 245, 0.82)',
            1.6,
          );
        }
        if (selectedTileIndex !== null) {
          drawTileOverlay(selectedTileIndex, 'rgba(147, 184, 203, 0.28)', '#edf1f5', 2.2);
        }

        const markerR = Math.max(3.6, 5.8 / Math.sqrt(cam.zoom));
        // Candidate hub sites (`core_site`) without a primary building yet: hollow gold diamond beacon
        for (const tile of surface.tiles) {
          if (!tile.tags.includes('core_site')) continue;
          const occ = occupiedByIndex.get(tile.index);
          if (occ?.primary) continue;
          const [cx, cy] = tile.center;
          const r = markerR * 0.96;
          ctx.beginPath();
          ctx.moveTo(cx, cy - r);
          ctx.lineTo(cx + r, cy);
          ctx.lineTo(cx, cy + r);
          ctx.lineTo(cx - r, cy);
          ctx.closePath();
          ctx.fillStyle = 'rgba(14, 22, 34, 0.72)';
          ctx.fill();
          ctx.lineWidth = 1.45 / cam.zoom;
          ctx.strokeStyle = 'rgba(230, 193, 122, 0.88)';
          ctx.stroke();

          ctx.beginPath();
          ctx.arc(cx, cy, r * 0.26, 0, Math.PI * 2);
          ctx.fillStyle = 'rgba(230, 193, 122, 0.88)';
          ctx.fill();
        }
        for (const occ of occupiedByIndex.values()) {
          const tile = surface.tiles[occ.index];
          if (!tile) continue;
          const [cx, cy] = tile.center;
          const pTags = occ.primary?.tags ?? [];
          const isCapital = pTags.includes('planetary_capital');
          const isRegionalCore = !isCapital && pTags.includes('regional_core');
          const isCampus = pTags.includes('corporate_campus');
          const isInfra =
            pTags.includes('infrastructure')
            || occ.secondary.some((b) => b !== null && b.tags.includes('infrastructure'));
          const secBuildings = occ.secondary.filter(
            (b): b is NonNullable<typeof b> => b !== null,
          );
          const primaryUnderstaffed =
            occ.primary !== null && occ.primary.jobs_filled < occ.primary.jobs_required;
          const anyUnderstaffed =
            primaryUnderstaffed
            || secBuildings.some((b) => b.jobs_filled < b.jobs_required);

          if (isCapital) {
            const r = markerR * 1.25;
            ctx.beginPath();
            ctx.arc(cx, cy, r * 1.42, 0, Math.PI * 2);
            ctx.strokeStyle = 'rgba(230, 193, 122, 0.45)';
            ctx.lineWidth = 1.2 / cam.zoom;
            ctx.stroke();

            ctx.beginPath();
            ctx.moveTo(cx, cy - r);
            ctx.lineTo(cx + r, cy);
            ctx.lineTo(cx, cy + r);
            ctx.lineTo(cx - r, cy);
            ctx.closePath();
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.8 / cam.zoom;
            ctx.strokeStyle = primaryUnderstaffed ? '#e09f67' : '#e6c17a';
            ctx.stroke();

            ctx.beginPath();
            ctx.arc(cx, cy, r * 0.36, 0, Math.PI * 2);
            ctx.fillStyle = primaryUnderstaffed ? '#e09f67' : '#e6c17a';
            ctx.fill();
          } else if (isRegionalCore) {
            const half = markerR * 0.96;
            ctx.beginPath();
            ctx.rect(cx - half, cy - half, half * 2, half * 2);
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.65 / cam.zoom;
            ctx.strokeStyle = primaryUnderstaffed ? '#e09f67' : '#edf1f5';
            ctx.stroke();

            const inner = half * 0.42;
            ctx.fillStyle = primaryUnderstaffed ? '#e09f67' : '#edf1f5';
            ctx.fillRect(cx - inner, cy - inner, inner * 2, inner * 2);
          } else if (isCampus) {
            const r = markerR * 1.1;
            ctx.beginPath();
            for (let k = 0; k < 6; k++) {
              const ang = -Math.PI / 2 + (k * Math.PI) / 3;
              const vx = cx + Math.cos(ang) * r;
              const vy = cy + Math.sin(ang) * r;
              if (k === 0) ctx.moveTo(vx, vy);
              else ctx.lineTo(vx, vy);
            }
            ctx.closePath();
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.65 / cam.zoom;
            ctx.strokeStyle = primaryUnderstaffed ? '#e09f67' : '#7ac7a9';
            ctx.stroke();

            ctx.beginPath();
            ctx.arc(cx, cy, r * 0.36, 0, Math.PI * 2);
            ctx.fillStyle = primaryUnderstaffed ? '#e09f67' : '#7ac7a9';
            ctx.fill();
          } else if (isInfra) {
            const r = markerR * 1.1;
            ctx.beginPath();
            ctx.moveTo(cx, cy - r);
            ctx.lineTo(cx + r * 0.95, cy + r * 0.78);
            ctx.lineTo(cx - r * 0.95, cy + r * 0.78);
            ctx.closePath();
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.6 / cam.zoom;
            ctx.strokeStyle = anyUnderstaffed ? '#e09f67' : '#6ec6ff';
            ctx.stroke();
          } else if (occ.primary !== null) {
            const isPrivateOwner = occ.primary.owner.kind !== 'state';
            const strokeTone = anyUnderstaffed
              ? '#e09f67'
              : isPrivateOwner
                ? '#7ac7a9'
                : '#93b8cb';
            ctx.beginPath();
            ctx.arc(cx, cy, markerR, 0, Math.PI * 2);
            ctx.fillStyle = '#111a29';
            ctx.fill();
            ctx.lineWidth = 1.5 / cam.zoom;
            ctx.strokeStyle = strokeTone;
            ctx.stroke();

            ctx.beginPath();
            ctx.arc(cx, cy, markerR * 0.38, 0, Math.PI * 2);
            ctx.fillStyle = strokeTone;
            ctx.fill();
          } else {
            // Secondary-only tile (`primary === null`): compact ring distinguishing 1x workshops from 2x primary hubs
            const anyPrivateSec = secBuildings.some((b) => b.owner.kind !== 'state');
            const subR = markerR * 0.72;
            const strokeTone = anyUnderstaffed
              ? '#e09f67'
              : anyPrivateSec
                ? '#7ac7a9'
                : '#93b8cb';
            ctx.beginPath();
            ctx.arc(cx, cy, subR, 0, Math.PI * 2);
            ctx.fillStyle = 'rgba(17, 26, 41, 0.88)';
            ctx.fill();
            ctx.lineWidth = 1.25 / cam.zoom;
            ctx.strokeStyle = strokeTone;
            ctx.stroke();
          }

          const orbitR = (isCapital ? markerR * 1.45 : markerR) + 2.4;
          for (let s = 0; s < secBuildings.length; s++) {
            const secB = secBuildings[s]!;
            const secShort = secB.jobs_filled < secB.jobs_required;
            const isCbd = secB.tags.includes('commercial');
            const angle = -Math.PI / 2 + s * ((Math.PI * 2) / 3);
            const px = cx + Math.cos(angle) * orbitR;
            const py = cy + Math.sin(angle) * orbitR;
            ctx.beginPath();
            ctx.arc(px, py, Math.max(1.3, 2.0 / Math.sqrt(cam.zoom)), 0, Math.PI * 2);
            ctx.fillStyle = secShort
              ? '#e09f67'
              : isCbd
                ? '#e6c17a'
                : secB.owner.kind !== 'state'
                  ? '#7ac7a9'
                  : '#93b8cb';
            ctx.fill();
          }
        }

        // Subtle macro-region jurisdiction labels in Level 2 so political territories remain anchored
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        ctx.lineJoin = 'round';
        const l2FontPx = Math.max(7, (11.5 / cam.zoom) * Math.sqrt(cam.zoom / Math.max(cover, 1e-6)));
        ctx.font = `600 ${l2FontPx.toFixed(1)}px Inter, "Segoe UI", "PingFang SC", sans-serif`;
        ctx.lineWidth = 2.6 / cam.zoom;
        for (const region of surface.regions) {
          if (region.is_ocean || region.land_tile_count < 3) continue;
          const shortLabel = region.name.split(' · ')[1] ?? region.name;
          const [rx, ry] = region.center;
          ctx.strokeStyle = 'rgba(9, 14, 24, 0.72)';
          ctx.strokeText(shortLabel, rx, ry);
          ctx.fillStyle = 'rgba(237, 241, 245, 0.74)';
          ctx.fillText(shortLabel, rx, ry);
        }
      }

      ctx.restore();
    }

    syncZoomReadout(cam, width, height);
  }, [
    detailBaseCanvas,
    geo,
    macroBaseCanvas,
    occupiedByIndex,
    periodicLandMask,
    periodicWaterMask,
    planet.region_summaries,
    planet.tile_sectors,
    sectorByRegionId,
    selectedRegionId,
    selectedTileIndex,
    surface,
    syncZoomReadout,
    unclaimedLandMask,
  ]);

  const drawSceneRef = useRef(drawScene);
  drawSceneRef.current = drawScene;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onResize = () => drawScene();
    const observer = new ResizeObserver(() => drawScene());
    observer.observe(canvas);
    window.addEventListener('resize', onResize);
    drawScene();
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', onResize);
    };
  }, [drawScene]);

  useEffect(() => {
    if (!focusTarget || appliedFocusSeqRef.current === focusTarget.seq) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { width, height } = canvas.getBoundingClientRect();
    if (width === 0 || height === 0) return;
    appliedFocusSeqRef.current = focusTarget.seq;
    const cover = getCoverZoom(width, height, surface.width, surface.height);
    const nextZoom = focusTarget.targetRatio
      ? cover * focusTarget.targetRatio
      : Math.max(cameraRef.current?.zoom ?? cover, cover);
    cameraRef.current = clampCamera(
      {
        zoom: nextZoom,
        panX: width * 0.5 - focusTarget.mapX * nextZoom,
        panY: height * 0.5 - focusTarget.mapY * nextZoom,
      },
      width,
      height,
      surface.width,
      surface.height,
    );
    drawSceneRef.current();
  }, [focusTarget, surface.height, surface.width]);

  const zoomBy = useCallback(
    (factor: number, anchorX?: number, anchorY?: number) => {
      const canvas = canvasRef.current;
      if (!canvas || !cameraRef.current) return;
      const rect = canvas.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      const ax = anchorX ?? rect.width * 0.5;
      const ay = anchorY ?? rect.height * 0.5;
      const cam = cameraRef.current;
      const minZ = getCoverZoom(rect.width, rect.height, surface.width, surface.height);
      const nextZoom = Math.min(minZ * 6.0, Math.max(minZ, cam.zoom * factor));
      const mapX = (ax - cam.panX) / cam.zoom;
      const mapY = (ay - cam.panY) / cam.zoom;
      cameraRef.current = clampCamera(
        {
          zoom: nextZoom,
          panX: ax - mapX * nextZoom,
          panY: ay - mapY * nextZoom,
        },
        rect.width,
        rect.height,
        surface.width,
        surface.height,
      );
      drawScene();
    },
    [drawScene, surface.height, surface.width],
  );

  const resetCamera = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { width, height } = canvas.getBoundingClientRect();
    if (width === 0 || height === 0) return;
    cameraRef.current = computeDefaultCoverCamera(width, height, surface.width, surface.height);
    drawScene();
  }, [drawScene, surface.height, surface.width]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (ev: WheelEvent) => {
      ev.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const factor = ev.deltaY < 0 ? 1.18 : 1 / 1.18;
      zoomBy(factor, ev.clientX - rect.left, ev.clientY - rect.top);
    };
    canvas.addEventListener('wheel', onWheel, { passive: false });
    return () => canvas.removeEventListener('wheel', onWheel);
  }, [zoomBy]);

  const screenToMap = (clientX: number, clientY: number): [number, number] | null => {
    const canvas = canvasRef.current;
    const cam = cameraRef.current;
    if (!canvas || !cam) return null;
    const rect = canvas.getBoundingClientRect();
    const sx = clientX - rect.left;
    const sy = clientY - rect.top;
    return [(sx - cam.panX) / cam.zoom, (sy - cam.panY) / cam.zoom];
  };

  const getCurrentLevel = (): MapInteractionLevel => {
    const canvas = canvasRef.current;
    const cam = cameraRef.current;
    if (!canvas || !cam) return 'region';
    const { width, height } = canvas.getBoundingClientRect();
    if (width === 0 || height === 0) return 'region';
    const cover = getCoverZoom(width, height, surface.width, surface.height);
    return cam.zoom / Math.max(cover, 1e-6) >= DETAIL_ZOOM_RATIO ? 'tile' : 'region';
  };

  const handlePointerDown = (ev: React.PointerEvent<HTMLCanvasElement>) => {
    if (ev.button !== 0 || !cameraRef.current) return;
    dragRef.current = {
      active: true,
      moved: false,
      startX: ev.clientX,
      startY: ev.clientY,
      origPanX: cameraRef.current.panX,
      origPanY: cameraRef.current.panY,
    };
    try {
      ev.currentTarget.setPointerCapture(ev.pointerId);
    } catch {
      // Synthetic pointer events without an active hardware pointer id continue cleanly.
    }
  };

  const handlePointerMove = (ev: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    if (dragRef.current?.active && cameraRef.current) {
      const dx = ev.clientX - dragRef.current.startX;
      const dy = ev.clientY - dragRef.current.startY;
      if (Math.abs(dx) > 4 || Math.abs(dy) > 4) {
        dragRef.current.moved = true;
      }
      if (dragRef.current.moved) {
        const { width, height } = canvas.getBoundingClientRect();
        cameraRef.current = clampCamera(
          {
            zoom: cameraRef.current.zoom,
            panX: dragRef.current.origPanX + dx,
            panY: dragRef.current.origPanY + dy,
          },
          width,
          height,
          surface.width,
          surface.height,
        );
        drawScene();
        return;
      }
    }

    const pt = screenToMap(ev.clientX, ev.clientY);
    const ctx = canvas.getContext('2d');
    if (!pt || !ctx) return;

    const tile = geo.hitTestTile(ctx, pt[0], pt[1]);
    const level = getCurrentLevel();

    if (tile) {
      if (level === 'region') {
        const reg = surface.regions[tile.region_id];
        if (!reg || reg.is_ocean) {
          if (hoveredRegionRef.current !== null || hoveredTileRef.current !== null) {
            hoveredRegionRef.current = null;
            hoveredTileRef.current = null;
            drawScene();
          }
          setHoveredInfo(null);
          return;
        }
        const sec = sectorByRegionId.get(tile.region_id);
        const regSummary = planet.region_summaries[tile.region_id];
        const bCount = regSummary ? regSummary.primary_used + regSummary.secondary_used : 0;
        if (hoveredRegionRef.current !== tile.region_id || hoveredTileRef.current !== null) {
          hoveredRegionRef.current = tile.region_id;
          hoveredTileRef.current = null;
          drawScene();
        }
        const meta = sec
          ? `已占 ${regSummary?.claimed_land_tiles ?? reg.land_tile_count}/${reg.land_tile_count} 陆格 · 人口 ${sec.population} · 在岗 ${sec.jobs_filled}/${sec.population} · 建筑 ${bCount} 座 · 点击打开大区窗口`
          : `未拓荒荒野（${reg.land_tile_count} 陆格） · 点击打开大区窗口框入边疆`;
        setHoveredInfo(`${reg.name} · ${meta}`);
      } else {
        const rName = surface.regions[tile.region_id]?.name ?? `大区 ${tile.region_id + 1}`;
        const occ = occupiedByIndex.get(tile.index);
        const tileSecId = planet.tile_sectors[tile.index] ?? null;
        const tileSec = tileSecId !== null ? sectorById.get(tileSecId) ?? null : null;
        const claimTag = isWaterTags(tile.tags)
          ? ''
          : tileSec
            ? ` · [${tileSec.name.split(' · ')[1] ?? tileSec.name}辖区]`
            : ' · [无主荒野]';
        const allSlots = occ
          ? [occ.primary, ...occ.secondary].filter((b): b is NonNullable<typeof b> => b !== null)
          : [];
        const slotUsed = allSlots.length;
        const tileFilled = allSlots.reduce((acc, b) => acc + b.jobs_filled, 0);
        const tileReq = allSlots.reduce((acc, b) => acc + b.jobs_required, 0);
        const bText =
          slotUsed > 0 ? ` · 在岗 ${tileFilled}/${tileReq}（${slotUsed}/4 槽）` : '';
        const elevStr = isWaterTags(tile.tags)
          ? `水深 ${(20 - tile.elevation) * 220}m`
          : `宜居 ${tile.habitability}%`;
        if (hoveredTileRef.current !== tile.index || hoveredRegionRef.current !== null) {
          hoveredTileRef.current = tile.index;
          hoveredRegionRef.current = null;
          drawScene();
        }
        setHoveredInfo(
          `#${String(tile.index + 1).padStart(4, '0')} · ${rName}${claimTag} · ${formatTagSummary(tile.tags)}（${elevStr}）${bText} · 点击打开地块窗口`,
        );
      }
    } else {
      if (hoveredTileRef.current !== null || hoveredRegionRef.current !== null) {
        hoveredTileRef.current = null;
        hoveredRegionRef.current = null;
        drawScene();
      }
      setHoveredInfo(null);
    }
  };

  const handlePointerUp = (ev: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current;
    dragRef.current = null;
    if (!drag || drag.moved) return;

    const canvas = canvasRef.current;
    const ctx = canvas?.getContext('2d');
    const pt = screenToMap(ev.clientX, ev.clientY);
    if (!ctx || !pt) return;

    const tile = geo.hitTestTile(ctx, pt[0], pt[1]);
    if (!tile) return;

    if (getCurrentLevel() === 'region') {
      const reg = surface.regions[tile.region_id];
      if (reg && !reg.is_ocean) {
        onSelectRegion(tile.region_id);
      }
    } else {
      onSelectTile(tile.index);
    }
  };

  const handleDoubleClick = (ev: React.MouseEvent<HTMLCanvasElement>) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    if (getCurrentLevel() === 'region') {
      zoomBy(1.9, ev.clientX - rect.left, ev.clientY - rect.top);
    } else {
      zoomBy(1.35, ev.clientX - rect.left, ev.clientY - rect.top);
    }
  };

  const handlePointerLeave = () => {
    if (hoveredTileRef.current !== null || hoveredRegionRef.current !== null) {
      hoveredTileRef.current = null;
      hoveredRegionRef.current = null;
      drawScene();
    }
    setHoveredInfo(null);
  };

  return (
    <>
      <canvas
        ref={canvasRef}
        className="planet-canvas surface-map-canvas"
        data-testid="surface-canvas"
        data-map-level={activeLevel}
        role="img"
        aria-label={`${planet.name}的维诺地表与海洋版图，当前为${activeLevel === 'region' ? '第一层大区视图' : '第二层地形与地块视图'}。`}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onDoubleClick={handleDoubleClick}
        onPointerLeave={handlePointerLeave}
      />
      <div className="map-zoom-dock" aria-label="地图缩放控制">
        <button
          type="button"
          className="map-tool-btn"
          data-testid="map-zoom-in"
          onClick={() => zoomBy(1.3)}
          title="放大（超过 165% 进入地形与地块层）"
        >
          +
        </button>
        <button
          type="button"
          className="map-tool-btn zoom-readout"
          data-testid="map-zoom-reset"
          onClick={resetCamera}
          title="回到第一层铺满大区视图（100%）"
        >
          {zoomPercent}%
        </button>
        <button
          type="button"
          className="map-tool-btn"
          data-testid="map-zoom-out"
          onClick={() => zoomBy(1 / 1.3)}
          title="缩小（不低于铺满视窗 100%）"
        >
          −
        </button>
      </div>
      {hoveredInfo && (
        <div className="map-hover-hud" aria-live="off">
          {hoveredInfo}
        </div>
      )}
    </>
  );
}
