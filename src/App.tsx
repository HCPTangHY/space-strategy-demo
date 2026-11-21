import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { FloatingWindow } from './FloatingWindow';
import {
  BUILDING_COLONIAL_OUTPOST,
  BUILDING_REGIONAL_CORE,
  connectGameStream,
  connectionLabel,
  DEPOT_POLICY_NEXT,
  formatCents,
  isWaterTags,
  MONETARY_POLICY_NEXT,
  TILE_TAG_LABELS,
} from './game';
import type {
  BuildingSnapshot,
  BuildingTagId,
  DepotPolicy,
  GameSpeed,
  GameStreamHandle,
  MonetaryPolicy,
  OccupiedTileSnapshot,
  PlacedBuildingSnapshot,
  PlanetSnapshot,
  PlanetSurfaceSnapshot,
  RegionSummarySnapshot,
  ScenarioPreset,
  SectorSnapshot,
  SurfacePolygonTile,
  TileSlotKey,
  TimeSnapshot,
} from './game';
import { PlanetSurfaceView } from './PlanetSurfaceView';
import type { MapFocusTarget, MapInteractionLevel } from './PlanetSurfaceView';
import { PlanetView } from './PlanetView';

const SPEED_OPTIONS: ReadonlyArray<{ value: GameSpeed; label: string; hint: string }> = [
  { value: 0, label: '暂停', hint: '暂停推进（空格键切换）' },
  { value: 1, label: '1x', hint: '慢速：2.0 秒/天（60 秒/月）' },
  { value: 2, label: '2x', hint: '常速：1.0 秒/天（30 秒/月）' },
  { value: 3, label: '3x', hint: '快速：0.25 秒/天（7.5 秒/月）' },
  { value: 4, label: '4x', hint: '极速：无人工等待推进' },
];

const SECONDARY_KEYS: ReadonlyArray<{ key: TileSlotKey; label: string; idx: 0 | 1 | 2 }> = [
  { key: 'secondary0', label: '次槽 1', idx: 0 },
  { key: 'secondary1', label: '次槽 2', idx: 1 },
  { key: 'secondary2', label: '次槽 3', idx: 2 },
];

type SubWinKey = 'polity' | 'region' | 'tile';

function formatPlacedOutput(
  building: PlacedBuildingSnapshot,
  goodNames: ReadonlyMap<string, string>,
): string {
  const outParts = Object.entries(building.effective_monthly_output).map(([gid, eff]) => {
    const base = building.base_monthly_output[gid] ?? eff;
    const gName = goodNames.get(gid) ?? gid;
    return eff < base ? `+${eff}/${base} ${gName}` : `+${eff} ${gName}`;
  });
  const inParts = Object.entries(building.monthly_inputs).map(
    ([gid, amt]) => `-${amt} ${goodNames.get(gid) ?? gid}`,
  );
  if (outParts.length > 0) {
    const prefix = inParts.length > 0 ? `${inParts.join(' ')} → ` : '';
    const hostSuffix =
      building.hosting_capacity > 0 ? ` · 托管 ${building.hosting_capacity}` : '';
    return `${prefix}${outParts.join(' · ')}/月${hostSuffix}`;
  }
  if (building.tags.includes('corporate_campus')) {
    return `园区托管 ${building.hosting_capacity} 座 · +15%产出`;
  }
  if (building.hosting_capacity > 0) {
    return `托管上限 ${building.hosting_capacity} 座`;
  }
  if (building.tags.includes('planetary_capital') || building.tags.includes('regional_core')) {
    return '行政与驻军中枢';
  }
  return '公共设施';
}

function formatCatalogRecipe(
  def: BuildingSnapshot,
  scale: number,
  goodNames: ReadonlyMap<string, string>,
): string {
  const outs = Object.entries(def.monthly_output).map(
    ([gid, amt]) => `+${amt * scale}${goodNames.get(gid) ?? gid}`,
  );
  const ins = Object.entries(def.monthly_inputs).map(
    ([gid, amt]) => `-${amt * scale}${goodNames.get(gid) ?? gid}`,
  );
  if (outs.length > 0) {
    return ins.length > 0 ? `${ins.join(' ')}→${outs.join(' ')}` : outs.join(' ');
  }
  if (def.hosting_capacity > 0) {
    return `托管+${def.hosting_capacity}`;
  }
  return `${def.jobs_required * scale}岗`;
}

function formatStaffingBadge(building: PlacedBuildingSnapshot): { text: string; className: string } {
  if (building.jobs_required === 0 || building.jobs_filled >= building.jobs_required) {
    return {
      text: `在岗 ${building.jobs_filled}/${building.jobs_required}`,
      className: 'tile-tag',
    };
  }
  if (building.jobs_filled === 0) {
    return {
      text: `停工 0/${building.jobs_required}`,
      className: 'tile-tag shortage-tag',
    };
  }
  return {
    text: `缺工 ${building.jobs_filled}/${building.jobs_required}`,
    className: 'tile-tag shortage-tag',
  };
}

function formatSigned(val: number): string {
  return val >= 0 ? `+${val.toLocaleString('zh-CN')}` : val.toLocaleString('zh-CN');
}

function hasAnyTagOverlap(a: readonly BuildingTagId[], b: readonly BuildingTagId[]): boolean {
  return a.some((tag) => b.includes(tag));
}

function canBuildOnTileSlot(
  def: BuildingSnapshot,
  tile: SurfacePolygonTile | null,
  slot: TileSlotKey,
  occupied: OccupiedTileSnapshot | null,
  planet: PlanetSnapshot,
  sector: SectorSnapshot | null,
): boolean {
  if (!tile) return false;
  if ((planet.tile_sectors[tile.index] ?? null) === null) return false;
  const isPrimary = slot === 'primary';
  if (def.slot_rule === 'primary_only' && !isPrimary) return false;
  if (def.slot_rule === 'secondary_only' && isPrimary) return false;

  if (!def.required_tile_tags.every((req) => tile.tags.includes(req))) return false;
  if (def.forbidden_tile_tags.some((forb) => tile.tags.includes(forb))) return false;

  if (def.tags.includes('corporate_campus') && !sector?.can_found_enterprise) {
    return false;
  }

  if (!isPrimary && def.required_primary_tags.length > 0) {
    const primaryB = occupied?.primary ?? null;
    if (!primaryB || !hasAnyTagOverlap(primaryB.tags, def.required_primary_tags)) {
      return false;
    }
  }

  if (
    def.unique_planet_tags.length > 0
    || def.unique_region_tags.length > 0
    || def.unique_tile_tags.length > 0
  ) {
    for (const occ of planet.occupied_tiles) {
      const entries: Array<[TileSlotKey, PlacedBuildingSnapshot | null]> = [
        ['primary', occ.primary],
        ['secondary0', occ.secondary[0]],
        ['secondary1', occ.secondary[1]],
        ['secondary2', occ.secondary[2]],
      ];
      for (const [sk, placed] of entries) {
        if (!placed) continue;
        if (occ.index === tile.index && sk === slot) continue;
        if (hasAnyTagOverlap(placed.tags, def.unique_planet_tags)) return false;
        if (occ.region_id === tile.region_id && hasAnyTagOverlap(placed.tags, def.unique_region_tags)) {
          return false;
        }
        if (occ.index === tile.index && hasAnyTagOverlap(placed.tags, def.unique_tile_tags)) {
          return false;
        }
      }
    }
  }

  return true;
}

function canBuildAnywhereInRegion(
  def: BuildingSnapshot,
  regionId: number,
  surface: PlanetSurfaceSnapshot,
  planet: PlanetSnapshot,
  occupiedMap: ReadonlyMap<number, OccupiedTileSnapshot>,
  sector: SectorSnapshot | null,
  regionSummary: RegionSummarySnapshot,
): boolean {
  if (def.tags.includes('planetary_capital')) return false;
  if (def.tags.includes('corporate_campus') && !sector?.can_found_enterprise) return false;
  if (def.tags.includes('regional_core') && regionSummary.core_tile_index !== null) return false;

  for (const tile of surface.tiles) {
    if (tile.region_id !== regionId) continue;
    const tSecId = planet.tile_sectors[tile.index] ?? null;
    if (tSecId === null) continue;
    if (sector && tSecId !== sector.id) continue;
    const occ = occupiedMap.get(tile.index) ?? null;
    if (!occ?.primary && canBuildOnTileSlot(def, tile, 'primary', occ, planet, sector)) {
      return true;
    }
    for (const { key, idx } of SECONDARY_KEYS) {
      if (!occ?.secondary[idx] && canBuildOnTileSlot(def, tile, key, occ, planet, sector)) {
        return true;
      }
    }
  }
  return false;
}

export function App() {
  const [time, setTime] = useState<TimeSnapshot | null>(null);
  const [planet, setPlanet] = useState<PlanetSnapshot | null>(null);
  const [surface, setSurface] = useState<PlanetSurfaceSnapshot | null>(null);

  // Windows ONLY pop up when clicking Earth (and sub-windows when clicking a region or tile)!
  const [planetWindowOpen, setPlanetWindowOpen] = useState<boolean>(false);
  const [polityWindowOpen, setPolityWindowOpen] = useState<boolean>(false);
  const [regionWindowOpen, setRegionWindowOpen] = useState<boolean>(false);
  const [tileWindowOpen, setTileWindowOpen] = useState<boolean>(false);
  const [subWinOrder, setSubWinOrder] = useState<SubWinKey[]>(['polity', 'region', 'tile']);

  const [mapLevel, setMapLevel] = useState<MapInteractionLevel>('region');
  const [mapZoomPct, setMapZoomPct] = useState<number>(100);
  const [mapFocus, setMapFocus] = useState<MapFocusTarget | null>(null);
  const focusSeqRef = useRef<number>(1);

  const [showGrid, setShowGrid] = useState<boolean>(true);
  const [showAllResources, setShowAllResources] = useState<boolean>(false);
  const [showPopsDetail, setShowPopsDetail] = useState<boolean>(false);
  const [selectedGoodId, setSelectedGoodId] = useState<string | null>(null);
  const [regionBuildId, setRegionBuildId] = useState<string>('');
  const [slotBuildIds, setSlotBuildIds] = useState<Record<TileSlotKey, string>>({
    primary: '',
    secondary0: '',
    secondary1: '',
    secondary2: '',
  });
  const [selectedRegionId, setSelectedRegionId] = useState<number | null>(null);
  const [selectedTileIndex, setSelectedTileIndex] = useState<number | null>(null);
  const [streamError, setStreamError] = useState<string | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  const [controlNotice, setControlNotice] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  const streamRef = useRef<GameStreamHandle | null>(null);
  const lastRunningSpeedRef = useRef<Exclude<GameSpeed, 0>>(2);

  const bringSubWinToFront = useCallback((key: SubWinKey) => {
    setSubWinOrder((prev) => [...prev.filter((k) => k !== key), key]);
  }, []);

  const subZIndex = useCallback(
    (key: SubWinKey) => 14 + subWinOrder.indexOf(key),
    [subWinOrder],
  );

  const focusMapAt = useCallback((mapX: number, mapY: number, targetRatio?: number) => {
    const seq = focusSeqRef.current++;
    setMapFocus({ seq, mapX, mapY, targetRatio });
  }, []);

  useEffect(() => {
    setTime(null);
    setPlanet(null);
    setSurface(null);
    setSelectedRegionId(null);
    setSelectedTileIndex(null);
    setStreamError(null);
    setControlError(null);
    setControlNotice(null);

    const handle = connectGameStream({
      onEvent: (event) => {
        if (event.kind === 'init' || event.kind === 'tick') {
          if (event.time.speed !== 0) {
            lastRunningSpeedRef.current = event.time.speed;
          }
          setTime(event.time);
        } else if (event.kind === 'planet_surface') {
          if (!event.surface) {
            setStreamError('Rust 内核未返回当前星球的地表网格数据。');
          } else {
            const nextSurf = event.surface;
            setSurface((prev) => {
              if (prev && prev.grid_cell_count !== nextSurf.grid_cell_count) {
                setTileWindowOpen(false);
                setSelectedTileIndex(null);
              }
              return nextSurf;
            });
            setSelectedRegionId((prev) =>
              prev !== null && prev >= nextSurf.regions.length ? null : prev,
            );
          }
        } else if (event.kind === 'planet_view') {
          if (!event.planet) {
            setStreamError('Rust 内核未返回当前订阅的星球数据。');
          } else {
            const nextPlanet = event.planet;
            setPlanet(nextPlanet);
            setSelectedTileIndex((prev) =>
              prev === null || prev >= nextPlanet.tile_count ? nextPlanet.starter_tile_index : prev,
            );
          }
        } else if (event.kind === 'command_notice') {
          setControlError(null);
          setControlNotice(event.message);
        } else if (event.kind === 'command_error') {
          setControlNotice(null);
          setControlError(event.message);
        }
      },
      onDisconnect: (reason) => {
        setStreamError(reason);
      },
    });
    streamRef.current = handle;

    return () => {
      streamRef.current = null;
      handle.close();
    };
  }, [attempt]);

  useEffect(() => {
    if (surface && planet && selectedRegionId === null) {
      const capTile = surface.tiles[planet.starter_tile_index];
      if (capTile) {
        setSelectedRegionId(capTile.region_id);
      }
    }
  }, [planet, selectedRegionId, surface]);

  useEffect(() => {
    if (!controlNotice) return;
    const t = window.setTimeout(() => setControlNotice(null), 4200);
    return () => window.clearTimeout(t);
  }, [controlNotice]);

  const handleReloadData = useCallback(async (restartWorld: boolean) => {
    const handle = streamRef.current;
    if (!handle) return;
    setControlError(null);
    try {
      await handle.reloadData(restartWorld);
    } catch (error: unknown) {
      setControlError(String(error));
    }
  }, []);

  const handleSetScenarioPreset = useCallback(async (scenario: ScenarioPreset) => {
    const handle = streamRef.current;
    if (!handle) return;
    setControlError(null);
    setSelectedRegionId(null);
    setSelectedTileIndex(null);
    setRegionWindowOpen(false);
    setTileWindowOpen(false);
    try {
      await handle.setScenarioPreset(scenario);
    } catch (error: unknown) {
      setControlError(String(error));
    }
  }, []);

  const handleClaimTile = useCallback(async (planetId: string, tileIndex: number) => {
    const handle = streamRef.current;
    if (!handle) return;
    setControlError(null);
    try {
      await handle.claimTile(planetId, tileIndex);
    } catch (error: unknown) {
      setControlError(String(error));
    }
  }, []);

  const handleClaimRegionFrontier = useCallback(
    async (planetId: string, regionId: number, maxCount = 5) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.claimRegionFrontier(planetId, regionId, maxCount);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const applySpeed = useCallback(async (nextSpeed: GameSpeed) => {
    const handle = streamRef.current;
    if (!handle) return;
    if (nextSpeed !== 0) lastRunningSpeedRef.current = nextSpeed;
    setControlError(null);
    try {
      await handle.setSpeed(nextSpeed);
    } catch (error: unknown) {
      setControlError(String(error));
    }
  }, []);

  const handleBuildOnTile = useCallback(
    async (planetId: string, tileIndex: number, buildingId: string, slot: TileSlotKey) => {
      const handle = streamRef.current;
      if (!handle || !buildingId) return;
      setControlError(null);
      try {
        await handle.buildOnTile(planetId, tileIndex, buildingId, slot);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handleBuildInRegion = useCallback(
    async (planetId: string, regionId: number, buildingId: string) => {
      const handle = streamRef.current;
      if (!handle || !buildingId) return;
      setControlError(null);
      try {
        await handle.buildInRegion(planetId, regionId, buildingId);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handleDemolishSlot = useCallback(
    async (planetId: string, tileIndex: number, slot: TileSlotKey) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.demolishOnTile(planetId, tileIndex, slot);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handlePrivatizeSlot = useCallback(
    async (planetId: string, tileIndex: number, slot: TileSlotKey) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.privatizeSlot(planetId, tileIndex, slot);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handleNationalizeSlot = useCallback(
    async (planetId: string, tileIndex: number, slot: TileSlotKey) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.nationalizeSlot(planetId, tileIndex, slot);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handleTogglePrivateLandUse = useCallback(
    async (planetId: string, sectorId: number, allow: boolean) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.togglePrivateLandUse(planetId, sectorId, allow);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handleSetMonetaryPolicy = useCallback(
    async (planetId: string, policy: MonetaryPolicy) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.setMonetaryPolicy(planetId, policy);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const handleSetDepotPolicy = useCallback(
    async (planetId: string, sectorId: number, policy: DepotPolicy) => {
      const handle = streamRef.current;
      if (!handle) return;
      setControlError(null);
      try {
        await handle.setDepotPolicy(planetId, sectorId, policy);
      } catch (error: unknown) {
        setControlError(String(error));
      }
    },
    [],
  );

  const isConnected =
    streamError === null && time !== null && planet !== null && surface !== null;
  const currentSpeed: GameSpeed = time ? time.speed : 0;

  useEffect(() => {
    if (!isConnected) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.repeat || event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (
        target
        && (target.tagName === 'INPUT'
          || target.tagName === 'TEXTAREA'
          || target.tagName === 'SELECT'
          || target.isContentEditable)
      ) {
        return;
      }
      if (event.code === 'Space') {
        event.preventDefault();
        const next: GameSpeed = currentSpeed === 0 ? lastRunningSpeedRef.current : 0;
        void applySpeed(next);
      } else if (event.key === '1' || event.key === '2' || event.key === '3' || event.key === '4') {
        void applySpeed(Number(event.key) as GameSpeed);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isConnected, currentSpeed, applySpeed]);

  const goodNameById = useMemo(() => {
    const map = new Map<string, string>();
    if (!planet) return map;
    for (const g of planet.good_catalog) {
      map.set(g.id, g.name);
    }
    for (const r of planet.resources) {
      if (!map.has(r.id)) map.set(r.id, r.name);
    }
    return map;
  }, [planet]);

  const flowGoodSet = useMemo(() => {
    const set = new Set<string>();
    if (!planet) return set;
    for (const g of planet.good_catalog) {
      if (g.is_flow) set.add(g.id);
    }
    return set;
  }, [planet]);

  const occupiedMap = useMemo(() => {
    const map = new Map<number, OccupiedTileSnapshot>();
    if (!planet) return map;
    for (const item of planet.occupied_tiles) {
      map.set(item.index, item);
    }
    return map;
  }, [planet]);

  const sectorById = useMemo(() => {
    const map = new Map<number, SectorSnapshot>();
    if (!planet) return map;
    for (const sec of planet.sectors) {
      map.set(sec.id, sec);
    }
    return map;
  }, [planet]);

  const activeTileIndex = selectedTileIndex ?? planet?.starter_tile_index ?? 0;
  const selectedTileGeo = surface?.tiles[activeTileIndex] ?? null;

  const activeRegionId =
    selectedRegionId ?? selectedTileGeo?.region_id ?? 0;
  const selectedRegion = surface?.regions[activeRegionId] ?? null;
  const selectedRegionSummary =
    selectedRegion && planet ? planet.region_summaries[selectedRegion.id] ?? null : null;
  const selectedSector =
    selectedRegionSummary?.sector_id !== null && selectedRegionSummary?.sector_id !== undefined
      ? sectorById.get(selectedRegionSummary.sector_id) ?? null
      : null;

  const tileParentRegion =
    selectedTileGeo && surface ? surface.regions[selectedTileGeo.region_id] ?? null : null;
  const tileParentRegionSummary =
    tileParentRegion && planet ? planet.region_summaries[tileParentRegion.id] ?? null : null;
  const selectedTileSectorId = planet ? (planet.tile_sectors[activeTileIndex] ?? null) : null;
  const tileParentSector =
    selectedTileSectorId !== null
      ? sectorById.get(selectedTileSectorId) ?? null
      : tileParentRegionSummary?.sector_id !== null && tileParentRegionSummary?.sector_id !== undefined
        ? sectorById.get(tileParentRegionSummary.sector_id) ?? null
        : null;

  const isTileClaimableByContiguity = useCallback(
    (tileIdx: number): boolean => {
      if (!planet || !surface) return false;
      const t = surface.tiles[tileIdx];
      if (!t || isWaterTags(t.tags)) return false;
      if ((planet.tile_sectors[tileIdx] ?? null) !== null) return false;
      const anyOwned = surface.tiles.some(
        (cand) => !isWaterTags(cand.tags) && (planet.tile_sectors[cand.index] ?? null) !== null,
      );
      if (!anyOwned) return true;
      for (const nb of t.neighbors) {
        const nbTile = surface.tiles[nb];
        if (nbTile && !isWaterTags(nbTile.tags) && (planet.tile_sectors[nb] ?? null) !== null) {
          return true;
        }
      }
      if (t.tags.includes('coastal')) {
        for (const sea1 of t.neighbors) {
          const seaTile1 = surface.tiles[sea1];
          if (!seaTile1 || !seaTile1.tags.includes('coastal_sea')) continue;
          for (const nb2 of seaTile1.neighbors) {
            const tile2 = surface.tiles[nb2];
            if (!tile2) continue;
            if (!isWaterTags(tile2.tags) && (planet.tile_sectors[nb2] ?? null) !== null) {
              return true;
            }
            if (tile2.tags.includes('coastal_sea')) {
              for (const nb3 of tile2.neighbors) {
                const tile3 = surface.tiles[nb3];
                if (tile3 && !isWaterTags(tile3.tags) && (planet.tile_sectors[nb3] ?? null) !== null) {
                  return true;
                }
              }
            }
          }
        }
      }
      return false;
    },
    [planet, surface],
  );

  const selectedTileCanClaim = useMemo(
    () => isTileClaimableByContiguity(activeTileIndex),
    [activeTileIndex, isTileClaimableByContiguity],
  );

  const regionFrontierClaimableCount = useMemo(() => {
    if (!surface || !selectedRegion || selectedRegion.is_ocean) return 0;
    let cnt = 0;
    for (const t of surface.tiles) {
      if (t.region_id === selectedRegion.id && isTileClaimableByContiguity(t.index)) {
        cnt += 1;
      }
    }
    return cnt;
  }, [isTileClaimableByContiguity, selectedRegion, surface]);

  const regionHasClaimedCoreSite = useMemo(() => {
    if (!surface || !planet || !selectedRegion) return false;
    return surface.tiles.some(
      (t) =>
        t.region_id === selectedRegion.id
        && t.tags.includes('core_site')
        && (planet.tile_sectors[t.index] ?? null) !== null,
    );
  }, [planet, selectedRegion, surface]);

  const adminFree = planet ? Math.max(0, planet.polity.admin_capacity - planet.polity.admin_used) : 0;

  const selectedOccupied = occupiedMap.get(activeTileIndex) ?? null;
  const selectedIsWater = selectedTileGeo ? isWaterTags(selectedTileGeo.tags) : false;

  const sectorEnterprises = useMemo(() => {
    if (!planet || !selectedSector) return [];
    return planet.enterprises.filter((ent) => ent.hq_sector_id === selectedSector.id);
  }, [planet, selectedSector]);

  const landRegionCount = useMemo(
    () => (surface ? surface.regions.filter((r) => !r.is_ocean).length : 0),
    [surface],
  );
  const oceanRegionCount = useMemo(
    () => (surface ? surface.regions.filter((r) => r.is_ocean).length : 0),
    [surface],
  );

  const activeGoodBook = useMemo(() => {
    if (!selectedSector || selectedSector.market_books.length === 0) return null;
    const targetId = selectedGoodId ?? planet?.primary_staple_id ?? '';
    return (
      selectedSector.market_books.find((b) => b.good_id === targetId)
      ?? selectedSector.market_books[0]
      ?? null
    );
  }, [planet?.primary_staple_id, selectedGoodId, selectedSector]);

  const quickGoodBooks = useMemo(() => {
    if (!selectedSector) return [];
    const preferred = [
      planet?.primary_staple_id ?? '',
      'base:power',
      'base:water',
      'base:service',
      'base:cap_sea',
      'base:cap_land',
    ];
    const picked = preferred
      .map((id) => selectedSector.market_books.find((b) => b.good_id === id))
      .filter((b): b is NonNullable<typeof b> => Boolean(b));
    return picked.length > 0 ? picked : selectedSector.market_books.slice(0, 6);
  }, [planet?.primary_staple_id, selectedSector]);

  const coreBuildingDef = useMemo(
    () =>
      planet?.building_catalog.find((b) => b.id === BUILDING_REGIONAL_CORE)
      ?? planet?.building_catalog.find(
        (b) =>
          b.tags.includes('regional_core')
          && !b.tags.includes('planetary_capital')
          && b.id !== BUILDING_COLONIAL_OUTPOST,
      )
      ?? null,
    [planet],
  );
  const outpostBuildingDef = useMemo(
    () => planet?.building_catalog.find((b) => b.id === BUILDING_COLONIAL_OUTPOST) ?? null,
    [planet],
  );
  const commercialBuildingDef = useMemo(
    () => planet?.building_catalog.find((b) => b.tags.includes('commercial')) ?? null,
    [planet],
  );
  const campusBuildingDef = useMemo(
    () => planet?.building_catalog.find((b) => b.tags.includes('corporate_campus')) ?? null,
    [planet],
  );

  const regionBuildableCatalog = useMemo(() => {
    if (!planet || !surface || !selectedRegion || !selectedRegionSummary) return [];
    return planet.building_catalog.filter(
      (b) =>
        !b.tags.includes('planetary_capital')
        && !b.tags.includes('regional_core')
        && !b.tags.includes('corporate_campus')
        && canBuildAnywhereInRegion(
          b,
          selectedRegion.id,
          surface,
          planet,
          occupiedMap,
          selectedSector,
          selectedRegionSummary,
        ),
    );
  }, [occupiedMap, planet, selectedRegion, selectedRegionSummary, selectedSector, surface]);

  const effectiveRegionBuildDef = useMemo(() => {
    if (regionBuildableCatalog.length === 0) return null;
    return (
      regionBuildableCatalog.find((b) => b.id === regionBuildId)
      ?? regionBuildableCatalog.find((b) => b.tags.includes('agriculture'))
      ?? regionBuildableCatalog[0]
      ?? null
    );
  }, [regionBuildId, regionBuildableCatalog]);

  const slotBuildOptions = useMemo(() => {
    const empty: Record<TileSlotKey, BuildingSnapshot[]> = {
      primary: [],
      secondary0: [],
      secondary1: [],
      secondary2: [],
    };
    if (!planet || !selectedTileGeo) return empty;
    for (const sk of ['primary', 'secondary0', 'secondary1', 'secondary2'] as TileSlotKey[]) {
      empty[sk] = planet.building_catalog.filter((def) =>
        canBuildOnTileSlot(
          def,
          selectedTileGeo,
          sk,
          selectedOccupied,
          planet,
          tileParentSector,
        ),
      );
    }
    return empty;
  }, [planet, selectedOccupied, selectedTileGeo, tileParentSector]);

  const getEffectiveSlotBuildDef = useCallback(
    (slot: TileSlotKey): BuildingSnapshot | null => {
      const list = slotBuildOptions[slot];
      if (list.length === 0) return null;
      const chosen = slotBuildIds[slot];
      return (
        list.find((b) => b.id === chosen)
        ?? list.find((b) => b.tags.includes('agriculture'))
        ?? list.find((b) => !b.tags.includes('regional_core') && !b.tags.includes('corporate_campus'))
        ?? list[0]
        ?? null
      );
    },
    [slotBuildIds, slotBuildOptions],
  );

  // Level 1 click -> open/update Region Window!
  const handleMapSelectRegion = useCallback(
    (regionId: number) => {
      setSelectedRegionId(regionId);
      setRegionWindowOpen(true);
      bringSubWinToFront('region');
    },
    [bringSubWinToFront],
  );

  // Level 2 click -> open/update Tile Window!
  const handleMapSelectTile = useCallback(
    (tileIndex: number) => {
      setSelectedTileIndex(tileIndex);
      const t = surface?.tiles[tileIndex];
      if (t) setSelectedRegionId(t.region_id);
      setTileWindowOpen(true);
      bringSubWinToFront('tile');
    },
    [bringSubWinToFront, surface],
  );

  const jumpToTileInDetail = useCallback(
    (tileIndex: number) => {
      if (!surface) return;
      const t = surface.tiles[tileIndex];
      if (!t) return;
      setSelectedTileIndex(tileIndex);
      setSelectedRegionId(t.region_id);
      focusMapAt(t.center[0], t.center[1], 2.15);
      setTileWindowOpen(true);
      bringSubWinToFront('tile');
    },
    [bringSubWinToFront, focusMapAt, surface],
  );

  const selectNextMatchingEmptyTile = useCallback(
    (predicate: (t: SurfacePolygonTile) => boolean) => {
      if (!surface) return;
      const total = surface.tiles.length;
      for (let offset = 1; offset <= total; offset++) {
        const idx = (activeTileIndex + offset) % total;
        const cand = surface.tiles[idx];
        if (cand && !occupiedMap.has(idx) && predicate(cand)) {
          jumpToTileInDetail(idx);
          return;
        }
      }
    },
    [activeTileIndex, jumpToTileInDetail, occupiedMap, surface],
  );

  const handleOpenEarthWindow = useCallback(() => {
    setPlanetWindowOpen(true);
  }, []);

  const handleCloseEarthWindow = useCallback(() => {
    setPlanetWindowOpen(false);
    setPolityWindowOpen(false);
    setRegionWindowOpen(false);
    setTileWindowOpen(false);
  }, []);

  const handleLevelChange = useCallback((lvl: MapInteractionLevel, pct: number) => {
    setMapLevel(lvl);
    setMapZoomPct(pct);
  }, []);

  const toggleMapLevel = useCallback(() => {
    if (!surface) return;
    if (mapLevel === 'region') {
      const anchor =
        selectedRegion?.center
        ?? selectedTileGeo?.center
        ?? [surface.width * 0.5, surface.height * 0.5];
      focusMapAt(anchor[0], anchor[1], 2.05);
    } else {
      focusMapAt(surface.width * 0.5, surface.height * 0.5, 1.0);
    }
  }, [focusMapAt, mapLevel, selectedRegion, selectedTileGeo, surface]);

  const primaryBuilding = selectedOccupied?.primary ?? null;
  const hasDependentSecondaryOccupied = useMemo(() => {
    if (!selectedOccupied || !planet) return false;
    return selectedOccupied.secondary.some((secB) => {
      if (!secB) return false;
      const catDef = planet.building_catalog.find((d) => d.id === secB.id);
      return Boolean(catDef && catDef.required_primary_tags.length > 0);
    });
  }, [planet, selectedOccupied]);
  const canRedevelopPrimary = Boolean(
    primaryBuilding
      && !primaryBuilding.tags.includes('planetary_capital')
      && (!primaryBuilding.tags.includes('regional_core')
        || primaryBuilding.id === BUILDING_COLONIAL_OUTPOST)
      && !primaryBuilding.tags.includes('corporate_campus'),
  );

  const visibleResources = useMemo(() => {
    if (!planet) return [];
    if (showAllResources) return planet.resources;
    const compact = planet.resources.filter(
      (r) => r.id === planet.primary_staple_id || (!flowGoodSet.has(r.id) && r.amount > 0),
    );
    return compact.length > 0 ? compact : planet.resources.slice(0, 4);
  }, [flowGoodSet, planet, showAllResources]);

  const primaryChoice = getEffectiveSlotBuildDef('primary');

  return (
    <main className="observatory">
      {/* Persistent cosmic backdrop: clicking Earth opens the surface window! */}
      {planet && (
        <PlanetView
          planet={planet}
          isWindowOpen={planetWindowOpen}
          onSelectPlanet={handleOpenEarthWindow}
        />
      )}

      <header className="topbar">
        <div className="brand-cluster">
          <div className="brand">
            <span className="brand-mark" aria-hidden="true">✦</span>
            <span className="brand-title">SPACE STRATEGY</span>
          </div>

          {isConnected && planet && planetWindowOpen && (
            <button
              type="button"
              className="speed-btn active planet-dock-btn"
              data-testid="toggle-planet-window"
              onClick={handleCloseEarthWindow}
            >
              ◎ 返回宇宙视图
            </button>
          )}
        </div>

        {isConnected && time ? (
          <div className="time-bar" role="region" aria-label="时间与游戏速度">
            <div className="clock-readout">
              <span className="clock-epoch">{time.epoch_name}</span>
              <time className="clock-date" aria-live="off">{time.formatted}</time>
              <span className={time.speed === 0 ? 'clock-badge paused' : 'clock-badge running'}>
                {time.speed === 0 ? '已暂停' : `${time.speed}x`}
              </span>
            </div>
            <div className="speed-group" role="group" aria-label="游戏速度档位">
              {SPEED_OPTIONS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className={time.speed === option.value ? 'speed-btn active' : 'speed-btn'}
                  aria-pressed={time.speed === option.value}
                  title={option.hint}
                  onClick={() => void applySpeed(option.value)}
                >
                  {option.label}
                </button>
              ))}
            </div>
            <div className="speed-group" role="group" aria-label="剧本与数据控制">
              <select
                className="region-select scenario-select"
                aria-label="切换开局剧本"
                data-testid="scenario-select"
                value={time.scenario}
                onChange={(ev) => void handleSetScenarioPreset(ev.target.value as ScenarioPreset)}
              >
                <option value="earth">剧本：地球开局</option>
                <option value="colony">剧本：殖民地开局</option>
              </select>
              <button
                type="button"
                className="speed-btn"
                data-testid="btn-hot-reload"
                title="不丢失当前对局进度，原位热重载 gamedata/ 下的商品、建筑、职业、AI 参数与事件规则"
                onClick={() => void handleReloadData(false)}
              >
                ↻ 热重载
              </button>
              <button
                type="button"
                className="speed-btn"
                data-testid="btn-reload-restart"
                title="重新读取 gamedata/ 全部配置并按当前剧本重开对局"
                onClick={() => void handleReloadData(true)}
              >
                ⟲ 重载重开
              </button>
            </div>
          </div>
        ) : (
          <span className="section-label">宇宙视窗 <span className="muted">/ 001</span></span>
        )}
      </header>

      {controlError && (
        <p className="control-toast error" role="alert">{controlError}</p>
      )}
      {controlNotice && !controlError && (
        <p className="control-toast notice" role="status" data-testid="reload-notice">{controlNotice}</p>
      )}

      {isConnected && planet && surface ? (
        <>
          {/* 1. Primary Earth Map Window (only mounts after clicking Earth; full-bleed map, zero blank edges!) */}
          {planetWindowOpen && (
            <FloatingWindow
              testId="planet-window"
              zIndex={10}
              minWidth={640}
              minHeight={420}
              topSafeInset={76}
              bottomSafeInset={44}
              onClose={handleCloseEarthWindow}
              title={
                <div className="win-planet-title">
                  <span className="win-planet-dot" aria-hidden="true" />
                  <span>{planet.name}</span>
                  <span className="win-planet-sub" data-testid="planet-header-stats">
                    国库{' '}
                    <strong data-testid="polity-treasury">
                      {planet.polity.treasury.toLocaleString('zh-CN')}
                    </strong>{' '}
                    CR ({formatSigned(planet.polity.last_net_cashflow)}/月) · 直辖{' '}
                    <strong data-testid="polity-admin-quota">
                      {planet.polity.admin_used}/{planet.polity.admin_capacity}
                    </strong>{' '}
                    · 人口{' '}
                    <strong data-testid="planet-population">
                      {planet.total_population.toLocaleString('zh-CN')}
                    </strong>{' '}
                    · 建筑{' '}
                    <span data-testid="total-buildings">{planet.total_building_count}</span> 座 · 在岗{' '}
                    <span data-testid="planet-jobs">
                      {planet.total_jobs_filled}/{planet.total_jobs_required}
                    </span>
                  </span>
                </div>
              }
              headerActions={
                <div className="win-toolbar">
                  <button
                    type="button"
                    className={mapLevel === 'tile' ? 'speed-btn active' : 'speed-btn'}
                    data-testid="map-level-badge"
                    onClick={toggleMapLevel}
                    title="滚轮缩放自动切换层级；点击可直接在两层之间切换"
                  >
                    {mapLevel === 'region'
                      ? `第一层 · 大区 (${mapZoomPct}%)`
                      : `第二层 · 地形与格子 (${mapZoomPct}%)`}
                  </button>
                  <button
                    type="button"
                    className={polityWindowOpen ? 'speed-btn active' : 'speed-btn'}
                    data-testid="open-polity-window"
                    onClick={() => {
                      setPolityWindowOpen((prev) => !prev);
                      bringSubWinToFront('polity');
                    }}
                  >
                    国家与库存
                  </button>
                  {planet.capital_tile_index !== null && (
                    <button
                      type="button"
                      className="capital-jump-btn"
                      data-testid="jump-capital-tile"
                      onClick={() => jumpToTileInDetail(planet.capital_tile_index!)}
                    >
                      ★ 首府
                    </button>
                  )}
                  <a
                    className="speed-btn"
                    href="./map-editor.html"
                    target="_blank"
                    rel="noreferrer"
                    title="打开极简地球网格与大区编辑器（支持增删移动点、涂抹海陆与划分大区）"
                  >
                    地图编辑器
                  </a>
                  {mapLevel === 'tile' && (
                    <button
                      type="button"
                      className={showGrid ? 'speed-btn active' : 'speed-btn'}
                      aria-pressed={showGrid}
                      onClick={() => setShowGrid((prev) => !prev)}
                    >
                      细格线
                    </button>
                  )}
                </div>
              }
            >
              <div className="planet-window-stage">
                <div className="planet-window-map-pane">
                  <PlanetSurfaceView
                    planet={planet}
                    surface={surface}
                    selectedRegionId={regionWindowOpen ? activeRegionId : null}
                    selectedTileIndex={tileWindowOpen ? activeTileIndex : null}
                    onSelectRegion={handleMapSelectRegion}
                    onSelectTile={handleMapSelectTile}
                    showGrid={showGrid}
                    focusTarget={mapFocus}
                    onLevelChange={handleLevelChange}
                  />
                </div>
              </div>
            </FloatingWindow>
          )}

          {/* 2. Optional National Treasury & Planetary Stockpile Sub-Window */}
          {planetWindowOpen && polityWindowOpen && (
            <FloatingWindow
              testId="polity-window"
              zIndex={subZIndex('polity')}
              onFocus={() => bringSubWinToFront('polity')}
              onClose={() => setPolityWindowOpen(false)}
              minWidth={360}
              minHeight={280}
              topSafeInset={84}
              bottomSafeInset={48}
              defaultRect={(vw) => ({
                x: Math.max(24, Math.round(vw * 0.5 - 210)),
                y: 96,
                width: 420,
                height: 390,
              })}
              title={
                <div className="win-planet-title">
                  <span>{planet.polity.name} · 国家财政与总库存</span>
                </div>
              }
            >
              <div className="inspector-scroll">
                <div className="surface-card">
                  <div className="card-header">
                    <span className="card-title">中央国库与货币政策</span>
                    <span className="card-caption" data-testid="surface-summary">
                      直辖 {planet.polity.admin_used}/{planet.polity.admin_capacity} 格 · {landRegionCount} 陆区 + {oceanRegionCount} 海域
                    </span>
                  </div>
                  <ul className="stockpile-list">
                    <li className="stockpile-row">
                      <div className="row-label-col">
                        <span className="resource-name">中央国库</span>
                        <small className="row-subhint">
                          税率 {planet.polity.tax_rate_percent}% · 税入 {planet.polity.last_tax_income} · 国企 {formatSigned(planet.polity.last_state_enterprise_net)} · 运税 {planet.polity.last_freight_revenue} · 铸币 {planet.polity.last_seigniorage} · 行政 -{planet.polity.last_admin_upkeep}
                        </small>
                        <button
                          type="button"
                          className="inline-link-btn"
                          data-testid="cycle-monetary-policy"
                          onClick={() =>
                            void handleSetMonetaryPolicy(
                              planet.id,
                              MONETARY_POLICY_NEXT[planet.polity.monetary_policy],
                            )
                          }
                        >
                          央行：{planet.polity.monetary_policy_label}
                        </button>
                      </div>
                      <div className="resource-values">
                        <span className="resource-amount">
                          {planet.polity.treasury.toLocaleString('zh-CN')}
                        </span>
                        <span
                          className={planet.polity.last_net_cashflow >= 0 ? 'delta-badge positive' : 'delta-badge shortage-tag'}
                          data-testid="polity-cashflow"
                        >
                          {formatSigned(planet.polity.last_net_cashflow)}/月
                        </span>
                      </div>
                    </li>
                  </ul>

                  <div className="stock-compact-head">
                    <span className="row-subhint">全星实物库存（工场 + 官仓 + 居民）</span>
                    <button
                      type="button"
                      className="inline-link-btn"
                      data-testid="toggle-all-resources"
                      onClick={() => setShowAllResources((prev) => !prev)}
                    >
                      {showAllResources
                        ? '仅看有库存项'
                        : `展开全部商品（${planet.resources.length}）`}
                    </button>
                  </div>
                  <div className="resource-chip-grid">
                    {visibleResources.map((item) => (
                      <div key={item.id} className="resource-chip-cell">
                        <span className="econ-label">
                          {item.name}
                          {flowGoodSet.has(item.id) ? '（流量）' : ''}
                        </span>
                        <strong className="econ-val" data-testid={`stock-${item.id}`}>
                          {item.amount.toLocaleString('zh-CN')}
                        </strong>
                        {item.monthly_delta !== 0 && (
                          <small className="econ-sub" data-testid={`delta-${item.id}`}>
                            {item.monthly_delta > 0 ? `+${item.monthly_delta}` : item.monthly_delta}/月
                          </small>
                        )}
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </FloatingWindow>
          )}

          {/* 3. Level-1 Click Target: Region Inspector Window */}
          {planetWindowOpen && regionWindowOpen && selectedRegion && selectedRegionSummary && (
            <FloatingWindow
              testId="region-window"
              zIndex={subZIndex('region')}
              onFocus={() => bringSubWinToFront('region')}
              onClose={() => setRegionWindowOpen(false)}
              minWidth={390}
              minHeight={340}
              topSafeInset={84}
              bottomSafeInset={48}
              defaultRect={(_vw, vh) => ({
                x: 32,
                y: 96,
                width: 440,
                height: Math.min(640, vh - 150),
              })}
              title={
                <div className="win-planet-title">
                  <span
                    className="region-swatch"
                    style={{
                      backgroundColor: `rgb(${selectedRegion.color_rgb[0]}, ${selectedRegion.color_rgb[1]}, ${selectedRegion.color_rgb[2]})`,
                    }}
                  />
                  <span>{selectedRegion.name}</span>
                </div>
              }
              headerActions={
                <div className="win-toolbar">
                  <select
                    className="region-select"
                    aria-label="切换当前大区"
                    value={selectedRegion.id}
                    onChange={(ev) => setSelectedRegionId(Number(ev.target.value))}
                  >
                    {surface.regions.map((r) => (
                      <option key={r.id} value={r.id}>
                        {r.name}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    className="speed-btn"
                    data-testid="zoom-into-region"
                    onClick={() =>
                      focusMapAt(selectedRegion.center[0], selectedRegion.center[1], 2.05)
                    }
                  >
                    放大看地块
                  </button>
                </div>
              }
            >
              <div className="inspector-scroll">
                <div className="surface-card" aria-label="实控辖区政经与建设">
                  {selectedSector ? (
                    <>
                      <div className="good-picker-bar">
                        <select
                          className="region-select full-select"
                          aria-label="切换查看商品盘口"
                          data-testid="sector-good-select"
                          value={activeGoodBook?.good_id ?? ''}
                          onChange={(ev) => setSelectedGoodId(ev.target.value)}
                        >
                          {selectedSector.market_books.map((mb) => (
                            <option key={mb.good_id} value={mb.good_id}>
                              {mb.good_name} · 现价 {formatCents(mb.order_book.last_price_cents)} CR
                              {mb.order_book.last_net_import !== 0
                                ? `（净${mb.order_book.last_net_import > 0 ? '入' : '出'} ${Math.abs(mb.order_book.last_net_import)}）`
                                : ''}
                            </option>
                          ))}
                        </select>
                        <div className="quick-good-chips">
                          {quickGoodBooks.map((mb) => (
                            <button
                              key={mb.good_id}
                              type="button"
                              className={
                                activeGoodBook?.good_id === mb.good_id
                                  ? 'chip-btn active'
                                  : 'chip-btn'
                              }
                              onClick={() => setSelectedGoodId(mb.good_id)}
                            >
                              {mb.good_name} {formatCents(mb.order_book.last_price_cents)}
                            </button>
                          ))}
                        </div>
                      </div>

                      <div className="sector-econ-grid" data-testid="sector-market-bar">
                        <div className="econ-cell">
                          <span className="econ-label">
                            {activeGoodBook?.good_name ?? planet.primary_staple_name}最新成交价
                          </span>
                          <strong className="econ-val" data-testid="sector-good-price">
                            {formatCents(
                              activeGoodBook?.order_book.last_price_cents
                                ?? selectedSector.apple_price_cents,
                            )}
                          </strong>
                          <small className="econ-sub">
                            MA{' '}
                            {formatCents(
                              activeGoodBook?.order_book.ma_price_cents
                                ?? selectedSector.ma_apple_price_cents,
                            )}
                            {activeGoodBook && activeGoodBook.order_book.last_net_import !== 0
                              ? ` · 跨区${activeGoodBook.order_book.last_net_import > 0 ? `净入 ${activeGoodBook.order_book.last_net_import}` : `净出 ${-activeGoodBook.order_book.last_net_import}`}`
                              : activeGoodBook?.good_id === planet.primary_staple_id
                                ? ` · 主粮产 ${selectedSector.apple_supply} / 需 ${selectedSector.apple_demand}`
                                : ''}
                          </small>
                        </div>
                        <div className="econ-cell">
                          <span className="econ-label">民间投资池</span>
                          <strong className="econ-val" data-testid="sector-investment-pool">
                            {selectedSector.investment_pool.toLocaleString('zh-CN')}
                          </strong>
                          <small className="econ-sub" data-testid="sector-cbd-hosting">
                            商业街托管 {selectedSector.cbd_hosted}/{selectedSector.cbd_capacity} · 分红 {selectedSector.last_dividends}
                          </small>
                        </div>
                      </div>

                      {activeGoodBook && (
                        <div className="order-book" data-testid="sector-order-book">
                          <div className="order-book-head">
                            <span>{activeGoodBook.good_name}盘口（上月收盘）</span>
                            <small>
                              长协 {activeGoodBook.order_book.last_contract_volume} · 现货 {activeGoodBook.order_book.last_spot_volume}
                            </small>
                          </div>
                          <div className="order-book-grid">
                            <div className="order-side asks" data-testid="order-book-asks">
                              {activeGoodBook.order_book.asks.length === 0 ? (
                                <span className="order-empty">无卖单</span>
                              ) : (
                                [...activeGoodBook.order_book.asks].reverse().map((lv) => (
                                  <div key={`a${lv.price_cents}`} className="order-level ask">
                                    <span>卖 {formatCents(lv.price_cents)}</span>
                                    <span>{lv.qty}</span>
                                  </div>
                                ))
                              )}
                            </div>
                            <div className="order-side bids" data-testid="order-book-bids">
                              {activeGoodBook.order_book.bids.length === 0 ? (
                                <span className="order-empty">无买单</span>
                              ) : (
                                activeGoodBook.order_book.bids.map((lv) => (
                                  <div key={`b${lv.price_cents}`} className="order-level bid">
                                    <span>买 {formatCents(lv.price_cents)}</span>
                                    <span>{lv.qty}</span>
                                  </div>
                                ))
                              )}
                            </div>
                          </div>
                          {activeGoodBook.order_book.recent_trades.length > 0 && (
                            <div className="order-trades">
                              {activeGoodBook.order_book.recent_trades.map((t, i) => (
                                <span key={i}>
                                  {t.from_sector === t.to_sector ? '本地' : `#${t.from_sector + 1}→#${t.to_sector + 1}`} {t.qty} @ {formatCents(t.price_cents)}
                                  {t.freight_cents > 0 ? `（含运税 ${formatCents(t.freight_cents)}）` : ''}
                                </span>
                              ))}
                            </div>
                          )}
                          <div className="order-book-head">
                            <span data-testid="sector-depot">
                              本区官仓 {activeGoodBook.good_name} {activeGoodBook.depot_stock}（总容 {selectedSector.depot_capacity}）
                            </span>
                            <button
                              type="button"
                              className="inline-link-btn"
                              data-testid="cycle-depot-policy"
                              onClick={() =>
                                void handleSetDepotPolicy(
                                  planet.id,
                                  selectedSector.id,
                                  DEPOT_POLICY_NEXT[selectedSector.depot_policy],
                                )
                              }
                            >
                              官仓：{selectedSector.depot_policy_label}
                            </button>
                          </div>
                        </div>
                      )}

                      <div className="region-pop-box" data-testid="region-pop-stats">
                        <div className="region-pop-main">
                          <span className="pop-label">辖区人口</span>
                          <strong className="pop-number" data-testid="region-population">
                            {selectedSector.population.toLocaleString('zh-CN')}
                          </strong>
                          <span className="delta-badge positive">
                            +{selectedSector.monthly_growth}/月
                          </span>
                          <span className="tile-tag">满意度 {selectedSector.average_satisfaction}%</span>
                        </div>
                        <div className="region-pop-sub">
                          <span data-testid="region-employment">
                            在岗 {selectedSector.jobs_filled}/{selectedSector.population}
                          </span>
                          <span
                            className={selectedSector.unemployed_count > 0 ? 'tile-tag' : 'tile-tag empty-tag'}
                            data-testid="region-unemployed"
                          >
                            待业 {selectedSector.unemployed_count - selectedSector.vagrant_count}
                            {selectedSector.vagrant_count > 0 ? ` · 流民 ${selectedSector.vagrant_count}` : ''}
                          </span>
                          {selectedSector.active_vacancies > 0 ? (
                            <span className="tile-tag shortage-tag" data-testid="region-labor-status">
                              缺工 {selectedSector.active_vacancies}
                            </span>
                          ) : selectedSector.jobs_required > selectedSector.jobs_filled ? (
                            <span className="tile-tag empty-tag" data-testid="region-labor-status">
                              停招空置 {selectedSector.jobs_required - selectedSector.jobs_filled}
                            </span>
                          ) : (
                            <span className="tile-tag" data-testid="region-labor-status">
                              余力充足
                            </span>
                          )}
                        </div>
                        {selectedSector.wage_quotes.length > 0 && (
                          <div className="wage-strip" data-testid="sector-wage-strip">
                            <span className="wage-strip-label">工时行情（百人月薪）：</span>
                            {selectedSector.wage_quotes.map((wq) => (
                              <span key={wq.id} className="wage-pill">
                                {wq.name} {formatCents(wq.ma_wage_per_100_cents)}
                                {wq.last_hired > 0 ? ` (+${wq.last_hired})` : ''}
                                {wq.last_laid_off > 0 ? ` (-${wq.last_laid_off})` : ''}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>

                      <div className="sector-policy-row">
                        <button
                          type="button"
                          className="inline-link-btn"
                          data-testid="toggle-pops-detail"
                          onClick={() => setShowPopsDetail((prev) => !prev)}
                        >
                          {showPopsDetail
                            ? `收起 Pop 明细（${selectedSector.pops.length} 组）`
                            : `展开 Pop 明细（${selectedSector.pops.length} 组）`}
                        </button>
                        <button
                          type="button"
                          className={selectedSector.allow_private_land_use ? 'inline-link-btn policy-on' : 'inline-link-btn'}
                          data-testid="toggle-private-land"
                          onClick={() =>
                            void handleTogglePrivateLandUse(
                              planet.id,
                              selectedSector.id,
                              !selectedSector.allow_private_land_use,
                            )
                          }
                        >
                          {selectedSector.allow_private_land_use ? '民间次槽拿地：开放' : '民间次槽拿地：关闭'}
                        </button>
                      </div>

                      {showPopsDetail && (
                        <div className="pops-table" data-testid="sector-pops-table">
                          {selectedSector.pops.map((pg) => (
                            <div key={pg.pop_id} className="pop-row">
                              <span className="pop-role-name">{pg.name}</span>
                              <span className="pop-role-num">
                                {pg.id === 'base:vagrant'
                                  ? `流民 ${pg.count}`
                                  : pg.employer_kind === 'unowned' && pg.employed === 0
                                    ? `待业 ${pg.count}`
                                    : `在岗 ${pg.employed}/${pg.count}`}
                                {pg.contract_wage_per_100_cents > 0
                                  ? ` · 合同 ${formatCents(pg.contract_wage_per_100_cents)}`
                                  : ''}
                              </span>
                              <span className="pop-role-meta">
                                储蓄 {pg.savings} · {planet.primary_staple_name} {pg.pantry_apples}/{pg.pantry_capacity} · 食 {pg.food_met_percent}% · 满 {pg.satisfaction}%
                              </span>
                            </div>
                          ))}
                        </div>
                      )}

                      {sectorEnterprises.length > 0 && (
                        <div className="enterprise-list" data-testid="sector-enterprises">
                          {sectorEnterprises.map((ent) => (
                            <button
                              key={ent.id}
                              type="button"
                              className="core-pill-btn campus-pill"
                              onClick={() => jumpToTileInDetail(ent.campus_tile_index)}
                            >
                              <span>▲ {ent.name}（+{ent.output_bonus_percent}%产出）</span>
                              <span>
                                金库 {ent.treasury} · 托管 {ent.hosted_count}/{ent.hosting_capacity} · #{String(ent.campus_tile_index + 1).padStart(4, '0')}
                              </span>
                            </button>
                          ))}
                        </div>
                      )}
                    </>
                  ) : selectedRegion.is_ocean ? (
                    <p className="core-empty-note">远洋公海区域（无常住人口、本地市场与行政辖区）</p>
                  ) : (
                    <p className="core-empty-note">
                      未拓荒荒野大区（共 {selectedRegion.land_tile_count} 个陆地格，尚未框入本政权辖区；从接壤领土连片框入并抵达「中心选址」后即可建立殖民据点开埠）
                    </p>
                  )}

                  {!selectedRegion.is_ocean && selectedRegionSummary.claimed_land_tiles < selectedRegion.land_tile_count && (
                    <div className="frontier-claim-bar" data-testid="region-frontier-bar">
                      <span>
                        本区已框入直辖陆地 {selectedRegionSummary.claimed_land_tiles}/{selectedRegion.land_tile_count} 格
                        （政权可用行政力 +{adminFree}）
                      </span>
                      <button
                        type="button"
                        className="slot-build-btn"
                        data-testid="region-claim-frontier"
                        disabled={regionFrontierClaimableCount === 0 || adminFree <= 0}
                        onClick={() => void handleClaimRegionFrontier(planet.id, selectedRegion.id, 5)}
                      >
                        {regionFrontierClaimableCount === 0
                          ? '尚未与已控领土接壤'
                          : adminFree <= 0
                            ? '行政力已满'
                            : `+ 连片框入相邻荒野（最多 ${Math.min(5, adminFree, regionFrontierClaimableCount)} 格）`}
                      </button>
                    </div>
                  )}

                  <div className="region-slot-stats" data-testid="region-slot-stats">
                    <span>
                      主槽 {selectedRegionSummary.primary_used}/{selectedRegionSummary.primary_total}
                    </span>
                    <span>
                      次槽 {selectedRegionSummary.secondary_used}/{selectedRegionSummary.secondary_total}（可用陆地空次槽 {selectedRegionSummary.unlocked_secondary_empty}）
                    </span>
                  </div>

                  <div className="region-core-bar" data-testid="region-core-status">
                    {selectedRegionSummary.core_tile_index !== null ? (
                      <button
                        type="button"
                        className="core-pill-btn active-core"
                        onClick={() => jumpToTileInDetail(selectedRegionSummary.core_tile_index!)}
                      >
                        <span>
                          {selectedRegionSummary.core_building_name === '行星首府' ? '★ ' : '◆ '}
                          本区据点：{selectedRegionSummary.core_building_name}
                        </span>
                        <span>
                          #{String(selectedRegionSummary.core_tile_index + 1).padStart(4, '0')}
                        </span>
                      </button>
                    ) : selectedRegion.land_tile_count === 0 ? null : (
                      <span className="core-empty-note">本辖区尚未设立大区核心据点</span>
                    )}
                  </div>

                  {selectedSector && (
                    <div className="data-build-section">
                      <div className="slot-picker-row">
                        <select
                          className="region-select full-select"
                          aria-label="选择本辖区自动选格投建建筑"
                          data-testid="region-build-select"
                          value={effectiveRegionBuildDef?.id ?? ''}
                          onChange={(ev) => setRegionBuildId(ev.target.value)}
                        >
                          {regionBuildableCatalog.map((def) => (
                            <option key={def.id} value={def.id}>
                              {def.name} · 基准{def.build_cost}资金 · {formatCatalogRecipe(def, 1, goodNameById)}
                            </option>
                          ))}
                        </select>
                        <button
                          type="button"
                          className="slot-build-btn"
                          data-testid="region-build-submit"
                          disabled={!effectiveRegionBuildDef}
                          onClick={() => {
                            if (effectiveRegionBuildDef) {
                              void handleBuildInRegion(
                                planet.id,
                                selectedRegion.id,
                                effectiveRegionBuildDef.id,
                              );
                            }
                          }}
                        >
                          +1 投建
                        </button>
                      </div>

                      <div className="macro-btn-row">
                        {outpostBuildingDef && selectedRegionSummary.core_tile_index === null && (
                          <button
                            type="button"
                            className="macro-build-btn"
                            data-testid="region-build-outpost"
                            disabled={!regionHasClaimedCoreSite}
                            onClick={() =>
                              void handleBuildInRegion(planet.id, selectedRegion.id, outpostBuildingDef.id)
                            }
                          >
                            <span>+1 {outpostBuildingDef.name}</span>
                            <small>
                              {regionHasClaimedCoreSite
                                ? `${outpostBuildingDef.build_cost}资金 · 零水电开埠 +18行政力`
                                : '需先框入本区「中心选址」格'}
                            </small>
                          </button>
                        )}
                        {coreBuildingDef && (
                          <button
                            type="button"
                            className="macro-build-btn"
                            data-testid="region-build-core"
                            disabled={
                              selectedRegionSummary.core_tile_index !== null
                              || !regionHasClaimedCoreSite
                            }
                            onClick={() =>
                              void handleBuildInRegion(planet.id, selectedRegion.id, coreBuildingDef.id)
                            }
                          >
                            <span>+1 {coreBuildingDef.name}</span>
                            <small>
                              {selectedRegionSummary.core_tile_index !== null
                                ? `已有${selectedRegionSummary.core_building_name ?? '核心'}`
                                : regionHasClaimedCoreSite
                                  ? `主槽 ${coreBuildingDef.build_cost}资金 · +48行政力`
                                  : '需先框入本区「中心选址」格'}
                            </small>
                          </button>
                        )}
                        {commercialBuildingDef && (
                          <button
                            type="button"
                            className="macro-build-btn"
                            data-testid="region-build-cbd"
                            disabled={selectedRegionSummary.core_tile_index === null && sectorEnterprises.length === 0}
                            onClick={() =>
                              void handleBuildInRegion(planet.id, selectedRegion.id, commercialBuildingDef.id)
                            }
                          >
                            <span>+1 {commercialBuildingDef.name}</span>
                            <small>
                              次槽 {commercialBuildingDef.build_cost}资金 · +{commercialBuildingDef.hosting_capacity}私产名额
                            </small>
                          </button>
                        )}
                        {campusBuildingDef && (
                          <button
                            type="button"
                            className="macro-build-btn span-two"
                            data-testid="region-build-campus"
                            disabled={
                              !selectedSector.can_found_enterprise
                              || selectedRegionSummary.land_primary_used >= selectedRegionSummary.land_primary_total
                            }
                            onClick={() =>
                              void handleBuildInRegion(planet.id, selectedRegion.id, campusBuildingDef.id)
                            }
                          >
                            <span>▲ 批建{campusBuildingDef.name}</span>
                            <small>
                              {selectedSector.can_found_enterprise
                                ? `已满3座同业私产 · ${campusBuildingDef.build_cost}资金建总部`
                                : `需同业私产 ${selectedSector.unincorporated_ag_private}/3 座`}
                            </small>
                          </button>
                        )}
                      </div>
                    </div>
                  )}
                </div>
              </div>
            </FloatingWindow>
          )}

          {/* 4. Level-2 Click Target: Specific Tile Slot Window */}
          {planetWindowOpen && tileWindowOpen && selectedTileGeo && (
            <FloatingWindow
              testId="tile-window"
              zIndex={subZIndex('tile')}
              onFocus={() => bringSubWinToFront('tile')}
              onClose={() => setTileWindowOpen(false)}
              minWidth={390}
              minHeight={340}
              topSafeInset={84}
              bottomSafeInset={48}
              defaultRect={(vw, vh) => ({
                x: Math.max(24, vw - 468),
                y: 96,
                width: 436,
                height: Math.min(620, vh - 150),
              })}
              title={
                <div className="win-planet-title">
                  <span>地块</span>
                  <span className="tile-id-inline" data-testid="selected-tile-id">
                    #{String(activeTileIndex + 1).padStart(4, '0')}
                  </span>
                  {tileParentRegion && (
                    <button
                      type="button"
                      className="inline-link-btn"
                      data-testid="open-tile-parent-region"
                      onClick={() => handleMapSelectRegion(tileParentRegion.id)}
                    >
                      {tileParentRegion.name}
                    </button>
                  )}
                </div>
              }
              headerActions={
                <div className="win-toolbar">
                  <button
                    type="button"
                    className="inline-link-btn"
                    data-testid="pick-empty-tile"
                    onClick={() => selectNextMatchingEmptyTile((t) => !isWaterTags(t.tags))}
                  >
                    空陆格
                  </button>
                  <button
                    type="button"
                    className="inline-link-btn"
                    data-testid="pick-coastal-tile"
                    onClick={() =>
                      selectNextMatchingEmptyTile(
                        (t) => !isWaterTags(t.tags) && t.tags.includes('coastal'),
                      )
                    }
                  >
                    空海岸格
                  </button>
                  <button
                    type="button"
                    className="inline-link-btn"
                    data-testid="pick-sea-tile"
                    onClick={() => selectNextMatchingEmptyTile((t) => isWaterTags(t.tags))}
                  >
                    空海格
                  </button>
                </div>
              }
            >
              <div className="inspector-scroll">
                <div className="surface-card" aria-label="具体地块槽位管理">
                  <div className="terrain-badges" data-testid="selected-tile-tags">
                    {selectedTileGeo.tags.map((tagId) => (
                      <span key={tagId} className="meta-tag">
                        {TILE_TAG_LABELS[tagId]}
                      </span>
                    ))}
                    <span className="meta-tag habitability-tag" data-testid="tile-habitability">
                      宜居度 {selectedTileGeo.habitability}%
                    </span>
                    <span className="meta-tag">
                      {selectedIsWater
                        ? `水深 ${(20 - selectedTileGeo.elevation) * 220}m`
                        : `海拔 ${selectedTileGeo.elevation}`}
                    </span>
                    {!selectedIsWater && (
                      <span
                        className={selectedTileSectorId !== null ? 'meta-tag habitability-tag' : 'meta-tag'}
                        data-testid="tile-claim-badge"
                      >
                        {selectedTileSectorId !== null
                          ? `直辖：${tileParentSector?.name ?? '已纳入辖区'}`
                          : '无主荒野（未纳入政区）'}
                      </span>
                    )}
                  </div>

                  {!selectedIsWater && selectedTileSectorId === null && (
                    <div className="frontier-claim-bar" data-testid="tile-claim-bar">
                      <span>
                        该陆地格尚未纳入政区直辖（框入耗 1 行政管理力，当前可用 +{adminFree}）
                      </span>
                      <button
                        type="button"
                        className="slot-build-btn"
                        data-testid="claim-tile-btn"
                        disabled={!selectedTileCanClaim || adminFree <= 0}
                        onClick={() => void handleClaimTile(planet.id, activeTileIndex)}
                      >
                        {!selectedTileCanClaim
                          ? '需与已控领土接壤后方可连片框入'
                          : adminFree <= 0
                            ? `行政力已满（${planet.polity.admin_used}/${planet.polity.admin_capacity}）`
                            : `+ 框入政区直辖（耗 1 行政力）`}
                      </button>
                    </div>
                  )}

                  <div className="slots-section">
                    <div className="slot-box primary-box" data-testid="slot-primary">
                      <div className="tile-top">
                        <span className="slot-label">
                          主导产业槽（主槽）
                          {primaryBuilding?.tags.includes('planetary_capital')
                            ? ' · ★ 全星唯一'
                            : primaryBuilding?.tags.includes('regional_core')
                              ? ' · ◆ 大区核心'
                              : primaryBuilding?.tags.includes('corporate_campus')
                                ? ' · ▲ 企业总部'
                                : primaryBuilding && primaryBuilding.scale_multiplier > 1
                                  ? ` · ${primaryBuilding.scale_multiplier}x 主导规模`
                                  : ''}
                        </span>
                        {primaryBuilding ? (
                          <span className={formatStaffingBadge(primaryBuilding).className}>
                            {formatStaffingBadge(primaryBuilding).text}
                          </span>
                        ) : (
                          <span className="tile-tag empty-tag">空闲 · 2x/核心槽</span>
                        )}
                      </div>

                      {primaryBuilding ? (
                        <div className="slot-occupied-col">
                          <div className="slot-occupied-row">
                            <span className="tile-building">{primaryBuilding.name}</span>
                            <span className="tile-output">
                              {formatPlacedOutput(primaryBuilding, goodNameById)}
                            </span>
                          </div>
                          <div className="slot-owner-row">
                            <span
                              className={primaryBuilding.owner.kind === 'state' ? 'owner-badge state' : 'owner-badge private'}
                              data-testid="owner-primary"
                            >
                              {primaryBuilding.owner.label}
                            </span>
                            <div className="slot-mini-actions">
                              {primaryBuilding.owner.can_privatize && (
                                <button
                                  type="button"
                                  className="mini-action-btn"
                                  data-testid="privatize-primary"
                                  onClick={() => void handlePrivatizeSlot(planet.id, activeTileIndex, 'primary')}
                                >
                                  私有化 (+{primaryBuilding.owner.privatize_price})
                                </button>
                              )}
                              {primaryBuilding.owner.can_nationalize && (
                                <button
                                  type="button"
                                  className="mini-action-btn"
                                  data-testid="nationalize-primary"
                                  onClick={() => void handleNationalizeSlot(planet.id, activeTileIndex, 'primary')}
                                >
                                  国有化 (-{primaryBuilding.owner.nationalize_cost})
                                </button>
                              )}
                              {canRedevelopPrimary
                                && coreBuildingDef
                                && !selectedIsWater
                                && selectedTileGeo.tags.includes('core_site')
                                && ((tileParentRegionSummary?.core_tile_index ?? null) === null
                                  || primaryBuilding?.id === BUILDING_COLONIAL_OUTPOST) && (
                                  <button
                                    type="button"
                                    className="mini-action-btn"
                                    data-testid="redevelop-primary-core"
                                    onClick={() =>
                                      void handleBuildOnTile(
                                        planet.id,
                                        activeTileIndex,
                                        coreBuildingDef.id,
                                        'primary',
                                      )
                                    }
                                  >
                                    {primaryBuilding?.id === BUILDING_COLONIAL_OUTPOST
                                      ? `升级${coreBuildingDef.name}`
                                      : `改建${coreBuildingDef.name}`}
                                  </button>
                                )}
                              {canRedevelopPrimary
                                && primaryBuilding?.id !== BUILDING_COLONIAL_OUTPOST
                                && campusBuildingDef
                                && !selectedIsWater
                                && Boolean(tileParentSector?.can_found_enterprise) && (
                                  <button
                                    type="button"
                                    className="mini-action-btn"
                                    data-testid="redevelop-primary-campus"
                                    onClick={() =>
                                      void handleBuildOnTile(
                                        planet.id,
                                        activeTileIndex,
                                        campusBuildingDef.id,
                                        'primary',
                                      )
                                    }
                                  >
                                    改建{campusBuildingDef.name}
                                  </button>
                                )}
                              {!primaryBuilding.tags.includes('planetary_capital') && !hasDependentSecondaryOccupied && (
                                <button
                                  type="button"
                                  className="mini-action-btn danger-btn"
                                  data-testid="demolish-primary"
                                  onClick={() => void handleDemolishSlot(planet.id, activeTileIndex, 'primary')}
                                >
                                  拆除
                                </button>
                              )}
                            </div>
                          </div>
                        </div>
                      ) : slotBuildOptions.primary.length === 0 ? (
                        <span className="core-empty-note">当前地形无可建主槽建筑</span>
                      ) : (
                        <div className="slot-picker-row">
                          <select
                            className="region-select full-select"
                            aria-label="选择主槽建筑"
                            data-testid="select-build-primary"
                            value={primaryChoice?.id ?? ''}
                            onChange={(ev) =>
                              setSlotBuildIds((prev) => ({ ...prev, primary: ev.target.value }))
                            }
                          >
                            {slotBuildOptions.primary.map((def) => {
                              const scale = def.slot_rule === 'any' ? 2 : 1;
                              return (
                                <option key={def.id} value={def.id}>
                                  {def.name}
                                  {scale > 1 ? `（${scale}x主导）` : ''} · {def.build_cost * scale}资金 · {formatCatalogRecipe(def, scale, goodNameById)}
                                </option>
                              );
                            })}
                          </select>
                          <button
                            type="button"
                            className="slot-build-btn"
                            data-testid="build-primary-submit"
                            disabled={!primaryChoice}
                            onClick={() => {
                              if (primaryChoice) {
                                void handleBuildOnTile(
                                  planet.id,
                                  activeTileIndex,
                                  primaryChoice.id,
                                  'primary',
                                );
                              }
                            }}
                          >
                            + 建造主槽
                          </button>
                        </div>
                      )}
                    </div>

                    <div className="secondary-stack">
                      {SECONDARY_KEYS.map(({ key, label, idx }) => {
                        const secBuilding = selectedOccupied?.secondary[idx] ?? null;
                        const badge = secBuilding ? formatStaffingBadge(secBuilding) : null;
                        const secChoice = getEffectiveSlotBuildDef(key);
                        return (
                          <div key={key} className="slot-box secondary-box" data-testid={`slot-${key}`}>
                            <div className="tile-top">
                              <span className="slot-label">{label} · 1x标准</span>
                              <span className={badge ? badge.className : 'tile-tag empty-tag'}>
                                {badge ? `${secBuilding!.jobs_filled}/${secBuilding!.jobs_required}` : '空闲'}
                              </span>
                            </div>
                            {secBuilding ? (
                              <div className="slot-occupied-col">
                                <div className="slot-occupied-row">
                                  <span className="tile-building">{secBuilding.name}</span>
                                  <span className="tile-output">
                                    {formatPlacedOutput(secBuilding, goodNameById)}
                                  </span>
                                </div>
                                <div className="slot-owner-row">
                                  <span
                                    className={secBuilding.owner.kind === 'state' ? 'owner-badge state' : 'owner-badge private'}
                                    data-testid={`owner-${key}`}
                                  >
                                    {secBuilding.owner.kind === 'state'
                                      ? '国有'
                                      : secBuilding.owner.kind === 'enterprise'
                                        ? secBuilding.owner.label
                                        : '民间私产'}
                                  </span>
                                  <div className="slot-mini-actions">
                                    {secBuilding.owner.can_privatize && (
                                      <button
                                        type="button"
                                        className="mini-action-btn"
                                        data-testid={`privatize-${key}`}
                                        onClick={() => void handlePrivatizeSlot(planet.id, activeTileIndex, key)}
                                      >
                                        私有化 (+{secBuilding.owner.privatize_price})
                                      </button>
                                    )}
                                    {secBuilding.owner.can_nationalize && (
                                      <button
                                        type="button"
                                        className="mini-action-btn"
                                        data-testid={`nationalize-${key}`}
                                        onClick={() => void handleNationalizeSlot(planet.id, activeTileIndex, key)}
                                      >
                                        国有化 (-{secBuilding.owner.nationalize_cost})
                                      </button>
                                    )}
                                    <button
                                      type="button"
                                      className="mini-action-btn danger-btn"
                                      data-testid={`demolish-${key}`}
                                      onClick={() => void handleDemolishSlot(planet.id, activeTileIndex, key)}
                                    >
                                      拆除
                                    </button>
                                  </div>
                                </div>
                              </div>
                            ) : slotBuildOptions[key].length === 0 ? (
                              <span className="core-empty-note">无可建次槽设施</span>
                            ) : (
                              <div className="slot-picker-row">
                                <select
                                  className="region-select full-select"
                                  aria-label={`选择${label}建筑`}
                                  data-testid={`select-build-${key}`}
                                  value={secChoice?.id ?? ''}
                                  onChange={(ev) =>
                                    setSlotBuildIds((prev) => ({ ...prev, [key]: ev.target.value }))
                                  }
                                >
                                  {slotBuildOptions[key].map((def) => (
                                    <option key={def.id} value={def.id}>
                                      {def.name} · {def.build_cost}资金 · {formatCatalogRecipe(def, 1, goodNameById)}
                                    </option>
                                  ))}
                                </select>
                                <button
                                  type="button"
                                  className="slot-build-btn"
                                  data-testid={`build-${key}-submit`}
                                  disabled={!secChoice}
                                  onClick={() => {
                                    if (secChoice) {
                                      void handleBuildOnTile(
                                        planet.id,
                                        activeTileIndex,
                                        secChoice.id,
                                        key,
                                      );
                                    }
                                  }}
                                >
                                  + 建造
                                </button>
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                </div>
              </div>
            </FloatingWindow>
          )}
        </>
      ) : (
        <section className="status-panel" aria-live="polite">
          <p className="eyebrow">SPACE STRATEGY</p>
          <h1>{streamError ? '无法读取星球' : '正在读取星球'}</h1>
          <p>{streamError ?? '正在建立持久状态流并生成星球维诺地表与海域版图。'}</p>
          {streamError && (
            <button type="button" className="retry-btn" onClick={() => setAttempt((value) => value + 1)}>
              重新连接
            </button>
          )}
        </section>
      )}

      <footer className="footer">
        <span>
          <i className={isConnected ? 'indicator connected' : 'indicator'} />
          {isConnected ? connectionLabel : '等待内核连接'}
        </span>
        <span>
          点击地球展开地表 · 第一层点选大区 · 放大进入第二层点选地块 <span className="muted">空格键暂停</span>
        </span>
      </footer>
    </main>
  );
}
