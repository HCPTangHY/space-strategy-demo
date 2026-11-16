import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { compileSurfaceGeometry } from './coastline';
import { isWaterTags, TILE_TAG_LABELS } from './game';
import type {
  OccupiedTileSnapshot,
  PlanetSnapshot,
  PlanetSurfaceSnapshot,
  SurfacePolygonTile,
  TileTagId,
} from './game';

export type SurfaceLayerMode = 'region' | 'terrain';

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

function landFillStyle(
  tile: SurfacePolygonTile,
  regionRgb: [number, number, number],
  layerMode: SurfaceLayerMode,
): string {
  const tBase = deriveTagTerrainRgb(tile.tags);
  const elevFactor = (tile.elevation - 20) / 80;
  if (layerMode === 'terrain') {
    const shade = 0.9 + elevFactor * 0.22;
    return `rgb(${Math.min(255, Math.round(tBase[0] * shade))}, ${Math.min(255, Math.round(tBase[1] * shade))}, ${Math.min(255, Math.round(tBase[2] * shade))})`;
  }
  const lift = 0.94 + elevFactor * 0.14;
  const r = Math.min(255, Math.round((regionRgb[0] * 0.8 + tBase[0] * 0.2) * lift));
  const g = Math.min(255, Math.round((regionRgb[1] * 0.8 + tBase[1] * 0.2) * lift));
  const b = Math.min(255, Math.round((regionRgb[2] * 0.8 + tBase[2] * 0.2) * lift));
  return `rgb(${r}, ${g}, ${b})`;
}

function formatTagSummary(tags: readonly TileTagId[]): string {
  return tags
    .filter((t) => t !== 'land' && t !== 'sea')
    .map((t) => TILE_TAG_LABELS[t])
    .join(' · ');
}

function renderOffscreenBaseMap(
  surface: PlanetSurfaceSnapshot,
  geo: ReturnType<typeof compileSurfaceGeometry>,
  layerMode: SurfaceLayerMode,
  showGrid: boolean,
): HTMLCanvasElement | null {
  if (typeof document === 'undefined') return null;
  const scale = 2;
  const off = document.createElement('canvas');
  off.width = Math.round(surface.width * scale);
  off.height = Math.round(surface.height * scale);
  const ctx = off.getContext('2d');
  if (!ctx) return null;

  ctx.setTransform(scale, 0, 0, scale, 0, 0);

  // 1. Clean 2-tier ocean (`深海` base + softly feathered `近海大陆架` contour).
  ctx.fillStyle = '#12283f';
  ctx.fillRect(0, 0, surface.width, surface.height);

  ctx.save();
  ctx.clip(geo.waterMaskPath, 'evenodd');

  ctx.save();
  ctx.filter = 'blur(5px)';
  ctx.fillStyle = layerMode === 'terrain' ? '#25557a' : '#204868';
  ctx.fill(geo.shelfContourPath, 'evenodd');
  ctx.restore();

  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  ctx.strokeStyle = 'rgba(132, 194, 224, 0.22)';
  ctx.lineWidth = 5;
  ctx.stroke(geo.oceanCoastPath);

  if (geo.lakeCoastSvgPath.length > 0) {
    ctx.fillStyle = '#224c6e';
    ctx.fill(geo.lakeCoastPath, 'evenodd');
  }

  if (showGrid) {
    ctx.strokeStyle = 'rgba(142, 202, 230, 0.18)';
    ctx.lineWidth = 0.55;
    ctx.stroke(geo.shelfGridBorderPath);

    ctx.strokeStyle = 'rgba(116, 174, 208, 0.20)';
    ctx.lineWidth = 0.72;
    ctx.stroke(geo.deepGridBorderPath);
  }

  ctx.restore();

  // 2. Land domain (`landMaskPath` even-odd): colored & symbol-stamped purely from `tile.tags`!
  ctx.save();
  ctx.clip(geo.landMaskPath, 'evenodd');

  ctx.lineJoin = 'round';
  for (const tile of surface.tiles) {
    if (isWaterTags(tile.tags) || !tile.tags.includes('coastal')) continue;
    const regionRgb = surface.regions[tile.region_id]?.color_rgb ?? [115, 145, 125];
    const color = landFillStyle(tile, regionRgb, layerMode);
    const path = geo.tilePaths[tile.index];
    if (!path) continue;
    ctx.strokeStyle = color;
    ctx.lineWidth = 9;
    ctx.stroke(path);
  }

  for (const tile of surface.tiles) {
    if (isWaterTags(tile.tags)) continue;
    const regionRgb = surface.regions[tile.region_id]?.color_rgb ?? [115, 145, 125];
    const color = landFillStyle(tile, regionRgb, layerMode);
    const path = geo.tilePaths[tile.index];
    if (!path) continue;
    ctx.fillStyle = color;
    ctx.fill(path);
    ctx.strokeStyle = color;
    ctx.lineWidth = 0.55;
    ctx.stroke(path);
  }

  // Composable tag-driven map symbols (`mountains`, `hills`, `forest` can co-exist on the same tile!).
  ctx.lineCap = 'round';
  for (const tile of surface.tiles) {
    if (isWaterTags(tile.tags)) continue;
    const [cx, cy] = tile.center;
    const hasMountains = tile.tags.includes('mountains');
    const hasHills = tile.tags.includes('hills');
    const hasForest = tile.tags.includes('forest');

    if (hasMountains) {
      ctx.strokeStyle = 'rgba(20, 28, 40, 0.46)';
      ctx.lineWidth = 1.25;
      ctx.beginPath();
      ctx.moveTo(cx - 4.8, cy + 2.6);
      ctx.lineTo(cx, cy - 3.8);
      ctx.lineTo(cx + 4.8, cy + 2.6);
      ctx.stroke();
    } else if (hasHills) {
      const ox = hasForest ? -2.6 : 0;
      ctx.strokeStyle = 'rgba(24, 34, 46, 0.32)';
      ctx.lineWidth = 1.05;
      ctx.beginPath();
      ctx.arc(cx + ox, cy + 1.6, 3.4, Math.PI * 1.12, Math.PI * 1.88);
      ctx.stroke();
    }

    if (hasForest) {
      const ox = hasHills ? 3.2 : 0;
      ctx.fillStyle = 'rgba(22, 46, 34, 0.34)';
      ctx.beginPath();
      ctx.moveTo(cx + ox, cy - 3.3);
      ctx.lineTo(cx + ox - 2.7, cy + 2.1);
      ctx.lineTo(cx + ox + 2.7, cy + 2.1);
      ctx.closePath();
      ctx.fill();
    }
  }

  if (showGrid) {
    ctx.strokeStyle = 'rgba(14, 22, 34, 0.25)';
    ctx.lineWidth = 0.65;
    ctx.stroke(geo.landGridBorderPath);
  }

  ctx.strokeStyle = layerMode === 'region' ? 'rgba(15, 23, 36, 0.76)' : 'rgba(15, 23, 36, 0.38)';
  ctx.lineWidth = layerMode === 'region' ? 1.7 : 1.05;
  ctx.stroke(geo.regionBorderPath);

  ctx.restore();

  if (geo.lakeCoastSvgPath.length > 0) {
    ctx.strokeStyle = '#102030';
    ctx.lineWidth = 1.15;
    ctx.stroke(geo.lakeCoastPath);
  }
  ctx.strokeStyle = '#0e1c2c';
  ctx.lineWidth = 1.4;
  ctx.lineJoin = 'round';
  ctx.stroke(geo.oceanCoastPath);

  ctx.strokeStyle = 'rgba(147, 184, 203, 0.24)';
  ctx.lineWidth = 1.2;
  ctx.strokeRect(0.6, 0.6, surface.width - 1.2, surface.height - 1.2);

  return off;
}

interface Camera {
  zoom: number;
  panX: number;
  panY: number;
}

function computeDefaultCamera(
  viewportW: number,
  viewportH: number,
  mapW: number,
  mapH: number,
): Camera {
  const compact = viewportW < 760;
  if (compact) {
    const availW = viewportW - 32;
    const availH = 166;
    const zoom = Math.min(availW / mapW, availH / mapH) * 0.96;
    return {
      zoom,
      panX: (viewportW - mapW * zoom) * 0.5,
      panY: 116 + (availH - mapH * zoom) * 0.5,
    };
  }
  const leftInset = Math.min(392, viewportW * 0.4) + 56;
  const rightInset = 36;
  const topInset = 88;
  const bottomInset = 56;
  const availW = Math.max(320, viewportW - leftInset - rightInset);
  const availH = Math.max(260, viewportH - topInset - bottomInset);
  const zoom = Math.min(availW / mapW, availH / mapH) * 0.96;
  return {
    zoom,
    panX: leftInset + (availW - mapW * zoom) * 0.5,
    panY: topInset + (availH - mapH * zoom) * 0.5,
  };
}

export interface PlanetSurfaceViewProps {
  planet: PlanetSnapshot;
  surface: PlanetSurfaceSnapshot;
  selectedTileIndex: number;
  onSelectTile: (tileIndex: number) => void;
  layerMode: SurfaceLayerMode;
  showGrid: boolean;
}

export function PlanetSurfaceView({
  planet,
  surface,
  selectedTileIndex,
  onSelectTile,
  layerMode,
  showGrid,
}: PlanetSurfaceViewProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const cameraRef = useRef<Camera | null>(null);
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
  const [zoomPercent, setZoomPercent] = useState<number>(100);

  const geo = useMemo(() => compileSurfaceGeometry(surface), [surface]);

  const baseMapCanvas = useMemo(
    () => renderOffscreenBaseMap(surface, geo, layerMode, showGrid),
    [surface, geo, layerMode, showGrid],
  );

  const occupiedByIndex = useMemo(() => {
    const map = new Map<number, OccupiedTileSnapshot>();
    for (const occ of planet.occupied_tiles) {
      map.set(occ.index, occ);
    }
    return map;
  }, [planet.occupied_tiles]);

  const drawScene = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas || !baseMapCanvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const { width, height } = canvas.getBoundingClientRect();
    if (width === 0 || height === 0) return;

    if (!cameraRef.current) {
      cameraRef.current = computeDefaultCamera(width, height, surface.width, surface.height);
      setZoomPercent(Math.round(cameraRef.current.zoom * 100));
    }
    const cam = cameraRef.current;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const targetW = Math.round(width * dpr);
    const targetH = Math.round(height * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = '#090e18';
    ctx.fillRect(0, 0, width, height);

    ctx.save();
    ctx.translate(cam.panX, cam.panY);
    ctx.scale(cam.zoom, cam.zoom);
    ctx.drawImage(baseMapCanvas, 0, 0, surface.width, surface.height);

    const drawTileOverlay = (tileIdx: number, fill: string, stroke: string, widthPx: number) => {
      const tile = surface.tiles[tileIdx];
      const tPath = geo.tilePaths[tileIdx];
      if (!tile || !tPath) return;
      ctx.save();
      ctx.clip(isWaterTags(tile.tags) ? geo.waterMaskPath : geo.landMaskPath, 'evenodd');
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
        'rgba(237, 241, 245, 0.16)',
        'rgba(237, 241, 245, 0.76)',
        1.5,
      );
    }

    drawTileOverlay(selectedTileIndex, 'rgba(147, 184, 203, 0.26)', '#edf1f5', 2.2);

    const markerR = Math.max(4.4, 6.2 / Math.sqrt(cam.zoom));
    for (const occ of occupiedByIndex.values()) {
      const tile = surface.tiles[occ.index];
      if (!tile) continue;
      const [cx, cy] = tile.center;
      const pTags = occ.primary?.tags ?? [];
      const isCapital = pTags.includes('planetary_capital');
      const isRegionalCore = !isCapital && pTags.includes('regional_core');
      const isCampus = pTags.includes('corporate_campus');
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
      } else {
        const isPrivateOwner =
          occ.primary !== null && occ.primary.owner.kind !== 'state';
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
      }

      // Tiny satellite pips around the tile marker indicating occupied secondary slots (0..3);
      // gold = CBD commercial street, teal = private/enterprise facility, amber = understaffed!
      const orbitR = (isCapital ? markerR * 1.45 : markerR) + 2.8;
      for (let s = 0; s < secBuildings.length; s++) {
        const secB = secBuildings[s];
        const secShort = secB.jobs_filled < secB.jobs_required;
        const isCbd = secB.tags.includes('commercial');
        const angle = -Math.PI / 2 + s * ((Math.PI * 2) / 3);
        const px = cx + Math.cos(angle) * orbitR;
        const py = cy + Math.sin(angle) * orbitR;
        ctx.beginPath();
        ctx.arc(px, py, Math.max(1.5, 2.1 / Math.sqrt(cam.zoom)), 0, Math.PI * 2);
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

    if (layerMode === 'region') {
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.font = '600 11px Inter, "Segoe UI", "PingFang SC", sans-serif';
      ctx.lineJoin = 'round';
      for (const region of surface.regions) {
        if (region.is_ocean || region.land_tile_count < 3) continue;
        const shortLabel = region.name.split(' · ')[1] ?? region.name;
        const [rx, rawRy] = region.center;
        let collidesWithMarker = false;
        for (const occ of occupiedByIndex.values()) {
          const t = surface.tiles[occ.index];
          if (!t) continue;
          const dx = t.center[0] - rx;
          const dy = t.center[1] - rawRy;
          if (Math.abs(dx) < 38 && Math.abs(dy) < 13) {
            collidesWithMarker = true;
            break;
          }
        }
        const ry = collidesWithMarker ? rawRy + markerR * 1.9 + 9 : rawRy;
        ctx.lineWidth = 3;
        ctx.strokeStyle = 'rgba(9, 14, 24, 0.78)';
        ctx.strokeText(shortLabel, rx, ry);
        ctx.fillStyle = 'rgba(237, 241, 245, 0.93)';
        ctx.fillText(shortLabel, rx, ry);
      }
    }

    ctx.restore();
  }, [baseMapCanvas, geo, layerMode, occupiedByIndex, selectedTileIndex, surface]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onResize = () => {
      const { width, height } = canvas.getBoundingClientRect();
      if (width > 0 && height > 0) {
        cameraRef.current = computeDefaultCamera(width, height, surface.width, surface.height);
        setZoomPercent(Math.round(cameraRef.current.zoom * 100));
      }
      drawScene();
    };
    const observer = new ResizeObserver(() => drawScene());
    observer.observe(canvas);
    window.addEventListener('resize', onResize);
    drawScene();
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', onResize);
    };
  }, [drawScene, surface.height, surface.width]);

  const zoomBy = useCallback(
    (factor: number, anchorX?: number, anchorY?: number) => {
      const canvas = canvasRef.current;
      if (!canvas || !cameraRef.current) return;
      const rect = canvas.getBoundingClientRect();
      const ax = anchorX ?? rect.width * 0.65;
      const ay = anchorY ?? rect.height * 0.5;
      const cam = cameraRef.current;
      const nextZoom = Math.min(5.0, Math.max(0.45, cam.zoom * factor));
      const mapX = (ax - cam.panX) / cam.zoom;
      const mapY = (ay - cam.panY) / cam.zoom;
      cameraRef.current = {
        zoom: nextZoom,
        panX: ax - mapX * nextZoom,
        panY: ay - mapY * nextZoom,
      };
      setZoomPercent(Math.round(nextZoom * 100));
      drawScene();
    },
    [drawScene],
  );

  const resetCamera = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { width, height } = canvas.getBoundingClientRect();
    cameraRef.current = computeDefaultCamera(width, height, surface.width, surface.height);
    setZoomPercent(Math.round(cameraRef.current.zoom * 100));
    drawScene();
  }, [drawScene, surface.height, surface.width]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (ev: WheelEvent) => {
      ev.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const factor = ev.deltaY < 0 ? 1.16 : 1 / 1.16;
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

  const handlePointerDown = (ev: React.PointerEvent<HTMLCanvasElement>) => {
    if (ev.button !== 0 || !cameraRef.current) return;
    ev.currentTarget.setPointerCapture(ev.pointerId);
    dragRef.current = {
      active: true,
      moved: false,
      startX: ev.clientX,
      startY: ev.clientY,
      origPanX: cameraRef.current.panX,
      origPanY: cameraRef.current.panY,
    };
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
        cameraRef.current = {
          ...cameraRef.current,
          panX: dragRef.current.origPanX + dx,
          panY: dragRef.current.origPanY + dy,
        };
        drawScene();
        return;
      }
    }

    const pt = screenToMap(ev.clientX, ev.clientY);
    const ctx = canvas.getContext('2d');
    if (!pt || !ctx) return;

    const tile = geo.hitTestTile(ctx, pt[0], pt[1]);
    if (tile) {
      const rName = surface.regions[tile.region_id]?.name ?? `大区 ${tile.region_id + 1}`;
      const occ = occupiedByIndex.get(tile.index);
      const allSlots = occ
        ? [occ.primary, ...occ.secondary].filter((b): b is NonNullable<typeof b> => b !== null)
        : [];
      const slotUsed = allSlots.length;
      const tileFilled = allSlots.reduce((acc, b) => acc + b.jobs_filled, 0);
      const tileReq = allSlots.reduce((acc, b) => acc + b.jobs_required, 0);
      const pTags = occ?.primary?.tags ?? [];
      const coreLabel = pTags.includes('planetary_capital')
        ? ' · ★ 行星首府'
        : pTags.includes('regional_core')
          ? ' · ◆ 大区核心'
          : pTags.includes('corporate_campus')
            ? ` · ▲ ${occ?.primary?.owner.label ?? '企业园区'}`
            : '';
      const bText =
        slotUsed > 0 ? `${coreLabel} · 在岗 ${tileFilled}/${tileReq}（${slotUsed}/4 槽）` : '';
      const elevStr = isWaterTags(tile.tags)
        ? `水深 ${(20 - tile.elevation) * 220}m`
        : `宜居 ${tile.habitability}%`;
      if (hoveredTileRef.current !== tile.index) {
        hoveredTileRef.current = tile.index;
        drawScene();
      }
      setHoveredInfo(
        `#${String(tile.index + 1).padStart(4, '0')} · ${rName} · ${formatTagSummary(tile.tags)}（${elevStr}）${bText}`,
      );
    } else {
      if (hoveredTileRef.current !== null) {
        hoveredTileRef.current = null;
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
    if (tile) {
      onSelectTile(tile.index);
    }
  };

  const handlePointerLeave = () => {
    if (hoveredTileRef.current !== null) {
      hoveredTileRef.current = null;
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
        role="img"
        aria-label={`${planet.name}的维诺地表与海洋版图，含 ${surface.land_tile_count} 个陆地格、${surface.coastal_sea_count} 个近海细格与 ${surface.deep_ocean_count} 个远洋大格。`}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerLeave={handlePointerLeave}
      />
      <div className="map-zoom-dock" aria-label="地图缩放控制">
        <button type="button" className="map-tool-btn" onClick={() => zoomBy(1.25)} title="放大版图">
          +
        </button>
        <button
          type="button"
          className="map-tool-btn zoom-readout"
          onClick={resetCamera}
          title="重置版图视角"
        >
          {zoomPercent}%
        </button>
        <button type="button" className="map-tool-btn" onClick={() => zoomBy(0.8)} title="缩小版图">
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
