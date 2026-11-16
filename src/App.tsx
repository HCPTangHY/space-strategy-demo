import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  BUILDING_APPLE_ORCHARD,
  BUILDING_COMMERCIAL_STREET,
  BUILDING_CORPORATE_CAMPUS,
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
  DepotPolicy,
  GameSpeed,
  MonetaryPolicy,
  GameStreamHandle,
  OccupiedTileSnapshot,
  PlacedBuildingSnapshot,
  PlanetSnapshot,
  PlanetSurfaceSnapshot,
  SectorSnapshot,
  TileSlotKey,
  TimeSnapshot,
} from './game';
import { PlanetSurfaceView } from './PlanetSurfaceView';
import type { SurfaceLayerMode } from './PlanetSurfaceView';
import { PlanetView } from './PlanetView';

type ViewMode = 'surface' | 'orbit';

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

function formatPlacedOutput(building: PlacedBuildingSnapshot): string {
  if (building.tags.includes('commercial')) {
    return `托管上限 ${building.hosting_capacity} 座`;
  }
  if (building.tags.includes('corporate_campus')) {
    return `园区托管 ${building.hosting_capacity} 座 · +15%产出`;
  }
  if (building.tags.includes('planetary_capital') || building.tags.includes('regional_core')) {
    return '行政与驻军中枢';
  }
  const eff = building.effective_monthly_output['base:apple'] ?? 0;
  const base = building.base_monthly_output['base:apple'] ?? 0;
  if (eff < base) {
    return `+${eff}/${base} 苹果/月`;
  }
  return `+${eff} 苹果/月`;
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

export function App() {
  const [time, setTime] = useState<TimeSnapshot | null>(null);
  const [planet, setPlanet] = useState<PlanetSnapshot | null>(null);
  const [surface, setSurface] = useState<PlanetSurfaceSnapshot | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>('surface');
  const [layerMode, setLayerMode] = useState<SurfaceLayerMode>('region');
  const [showGrid, setShowGrid] = useState<boolean>(true);
  const [showPopsDetail, setShowPopsDetail] = useState<boolean>(false);
  const [selectedTileIndex, setSelectedTileIndex] = useState<number | null>(null);
  const [streamError, setStreamError] = useState<string | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  const streamRef = useRef<GameStreamHandle | null>(null);
  const lastRunningSpeedRef = useRef<Exclude<GameSpeed, 0>>(2);

  useEffect(() => {
    setTime(null);
    setPlanet(null);
    setSurface(null);
    setSelectedTileIndex(null);
    setStreamError(null);
    setControlError(null);

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
            setSurface(event.surface);
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
        } else if (event.kind === 'command_error') {
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
      if (!handle) return;
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
      if (!handle) return;
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
  const selectedRegion =
    selectedTileGeo && surface ? surface.regions[selectedTileGeo.region_id] ?? null : null;
  const selectedRegionSummary =
    selectedRegion && planet ? planet.region_summaries[selectedRegion.id] ?? null : null;
  const selectedSector =
    selectedRegionSummary?.sector_id !== null && selectedRegionSummary?.sector_id !== undefined
      ? sectorById.get(selectedRegionSummary.sector_id) ?? null
      : null;
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

  const selectRegionRepresentativeTile = useCallback(
    (regionId: number) => {
      if (!surface) return;
      const coreIdx = planet?.region_summaries[regionId]?.core_tile_index ?? null;
      if (coreIdx !== null && surface.tiles[coreIdx]) {
        setSelectedTileIndex(coreIdx);
        return;
      }
      const landTile = surface.tiles.find(
        (t) => t.region_id === regionId && !isWaterTags(t.tags),
      );
      const anyTile = landTile ?? surface.tiles.find((t) => t.region_id === regionId);
      if (anyTile) {
        setSelectedTileIndex(anyTile.index);
      }
    },
    [planet, surface],
  );

  const selectNextEmptyLandTile = useCallback(() => {
    if (!surface) return;
    const total = surface.tiles.length;
    for (let offset = 1; offset <= total; offset++) {
      const idx = (activeTileIndex + offset) % total;
      const cand = surface.tiles[idx];
      if (cand && !isWaterTags(cand.tags) && !occupiedMap.has(idx)) {
        setSelectedTileIndex(idx);
        return;
      }
    }
  }, [activeTileIndex, occupiedMap, surface]);

  const selectNextEmptySeaTile = useCallback(() => {
    if (!surface) return;
    const total = surface.tiles.length;
    for (let offset = 1; offset <= total; offset++) {
      const idx = (activeTileIndex + offset) % total;
      const cand = surface.tiles[idx];
      if (cand && isWaterTags(cand.tags) && !occupiedMap.has(idx)) {
        setSelectedTileIndex(idx);
        return;
      }
    }
  }, [activeTileIndex, occupiedMap, surface]);

  const primaryBuilding = selectedOccupied?.primary ?? null;
  const hasAnySecondaryOccupied = Boolean(
    selectedOccupied?.secondary.some((b) => b !== null),
  );
  const tileHasUrbanOrCampusHub = Boolean(
    primaryBuilding
      && (primaryBuilding.tags.includes('regional_core')
        || primaryBuilding.tags.includes('corporate_campus')),
  );
  const canRedevelopPrimary = Boolean(
    primaryBuilding
      && !primaryBuilding.tags.includes('planetary_capital')
      && !primaryBuilding.tags.includes('regional_core')
      && !primaryBuilding.tags.includes('corporate_campus'),
  );

  return (
    <main className="observatory">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">✦</span>
          <span className="brand-title">SPACE STRATEGY</span>
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
          </div>
        ) : (
          <span className="section-label">星球视图 <span className="muted">/ 001</span></span>
        )}
      </header>

      {controlError && (
        <p className="control-toast" role="alert">{controlError}</p>
      )}

      {isConnected && planet && surface ? (
        <>
          <div className="map-mode-bar" role="toolbar" aria-label="星球视图与图层切换">
            <div className="mode-segment" role="group" aria-label="视图模式">
              <button
                type="button"
                className={viewMode === 'surface' ? 'speed-btn active' : 'speed-btn'}
                aria-pressed={viewMode === 'surface'}
                onClick={() => setViewMode('surface')}
              >
                地表版图
              </button>
              <button
                type="button"
                className={viewMode === 'orbit' ? 'speed-btn active' : 'speed-btn'}
                aria-pressed={viewMode === 'orbit'}
                onClick={() => setViewMode('orbit')}
              >
                轨道视图
              </button>
            </div>

            {viewMode === 'surface' && (
              <div className="mode-segment" role="group" aria-label="版图图层">
                <button
                  type="button"
                  className={layerMode === 'region' ? 'speed-btn active' : 'speed-btn'}
                  aria-pressed={layerMode === 'region'}
                  onClick={() => setLayerMode('region')}
                >
                  大区染色
                </button>
                <button
                  type="button"
                  className={layerMode === 'terrain' ? 'speed-btn active' : 'speed-btn'}
                  aria-pressed={layerMode === 'terrain'}
                  onClick={() => setLayerMode('terrain')}
                >
                  自然地形
                </button>
                <button
                  type="button"
                  className={showGrid ? 'speed-btn active' : 'speed-btn'}
                  aria-pressed={showGrid}
                  onClick={() => setShowGrid((prev) => !prev)}
                >
                  细格线
                </button>
              </div>
            )}
          </div>

          {viewMode === 'surface' ? (
            <PlanetSurfaceView
              planet={planet}
              surface={surface}
              selectedTileIndex={activeTileIndex}
              onSelectTile={setSelectedTileIndex}
              layerMode={layerMode}
              showGrid={showGrid}
            />
          ) : (
            <PlanetView planet={planet} />
          )}

          <section className="planet-panel" aria-label="星球信息与地表管理">
            <div className="planet-header">
              <div className="card-header">
                <p className="eyebrow">{planet.polity.name} · {planet.id}</p>
                {planet.capital_tile_index !== null && (
                  <button
                    type="button"
                    className="capital-jump-btn"
                    data-testid="jump-capital-tile"
                    onClick={() => setSelectedTileIndex(planet.capital_tile_index)}
                  >
                    ★ 首府 #{String(planet.capital_tile_index + 1).padStart(4, '0')}
                  </button>
                )}
              </div>
              <h1>{planet.name}</h1>
              <div className="planet-meta">
                <span>半径 {planet.radius_km.toLocaleString('zh-CN')} km</span>
                <span data-testid="surface-summary">
                  {landRegionCount} 辖区 + {oceanRegionCount} 海域
                </span>
                <span>
                  陆地 {surface.land_tile_count} · 近海 {surface.coastal_sea_count} · 深海 {surface.deep_ocean_count} 格
                </span>
              </div>
            </div>

            <div className="surface-card" aria-label="国家财政与星球总览">
              <div className="card-header">
                <h2 className="card-title">中央国库与星球人力</h2>
                <span className="card-caption" data-testid="total-buildings">
                  建筑 {planet.total_building_count} 座 · 在岗{' '}
                  <span data-testid="planet-jobs">
                    {planet.total_jobs_filled.toLocaleString('zh-CN')}/{planet.total_jobs_required.toLocaleString('zh-CN')}
                  </span>
                </span>
              </div>
              <ul className="stockpile-list">
                <li className="stockpile-row">
                  <div className="row-label-col">
                    <span className="resource-name">中央国库</span>
                    <small className="row-subhint">
                      税率 {planet.polity.tax_rate_percent}% · 税入 {planet.polity.last_tax_income} · 国企 {formatSigned(planet.polity.last_state_enterprise_net)} · 运费 {planet.polity.last_freight_revenue} · 铸币 {planet.polity.last_seigniorage} · 行政 -{planet.polity.last_admin_upkeep}
                    </small>
                    <button
                      type="button"
                      className="inline-link-btn"
                      data-testid="cycle-monetary-policy"
                      title="切换央行货币政策：紧缩 0% / 温和 3.6% / 宽松 12%（按年）"
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
                    <span className="resource-amount" data-testid="polity-treasury">
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
                <li className="stockpile-row">
                  <div className="row-label-col">
                    <span className="resource-name">全星总人口</span>
                    <small className="row-subhint">
                      数据表定义 {planet.pop_catalog.length} 类 Pop · 统一单一人力口径
                    </small>
                  </div>
                  <div className="resource-values">
                    <span className="resource-amount" data-testid="planet-population">
                      {planet.total_population.toLocaleString('zh-CN')}
                    </span>
                    <span
                      className={planet.monthly_population_growth > 0 ? 'delta-badge positive' : 'delta-badge'}
                      data-testid="planet-pop-growth"
                    >
                      +{planet.monthly_population_growth}/月
                    </span>
                  </div>
                </li>
                {planet.resources.map((item) => (
                  <li key={item.id} className="stockpile-row">
                    <div className="row-label-col">
                      <span className="resource-name">{item.name}（全星实物库存）</span>
                      <small className="row-subhint">工场、官仓与居民仓库合计 · 增量为上月产出减消费</small>
                    </div>
                    <div className="resource-values">
                      <span className="resource-amount" data-testid={`stock-${item.id}`}>
                        {item.amount.toLocaleString('zh-CN')}
                      </span>
                      <span
                        className={item.monthly_delta > 0 ? 'delta-badge positive' : 'delta-badge'}
                        data-testid={`delta-${item.id}`}
                      >
                        {item.monthly_delta >= 0 ? `+${item.monthly_delta}` : item.monthly_delta}/月
                      </span>
                    </div>
                  </li>
                ))}
              </ul>
            </div>

            {selectedRegion && selectedRegionSummary && (
              <div className="surface-card" aria-label="实控辖区政经与建设">
                <div className="card-header">
                  <h2 className="card-title">实控辖区管理</h2>
                  <select
                    className="region-select"
                    aria-label="切换当前大区"
                    value={selectedRegion.id}
                    onChange={(ev) => selectRegionRepresentativeTile(Number(ev.target.value))}
                  >
                    {surface.regions.map((r) => (
                      <option key={r.id} value={r.id}>
                        {r.name}
                      </option>
                    ))}
                  </select>
                </div>

                {selectedSector ? (
                  <>
                    <div className="sector-econ-grid" data-testid="sector-market-bar">
                      <div className="econ-cell">
                        <span className="econ-label">苹果最新成交价</span>
                        <strong className="econ-val" data-testid="sector-apple-price">
                          {formatCents(selectedSector.apple_price_cents)}
                        </strong>
                        <small className="econ-sub">
                          MA {formatCents(selectedSector.ma_apple_price_cents)} · 产 {selectedSector.apple_supply} / 需 {selectedSector.apple_demand}
                          {selectedSector.net_trade_in !== 0
                            ? ` · 跨区${selectedSector.net_trade_in > 0 ? `净进口 ${selectedSector.net_trade_in}` : `净出口 ${-selectedSector.net_trade_in}`}`
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

                    <div className="order-book" data-testid="sector-order-book">
                      <div className="order-book-head">
                        <span>盘口（上月收盘）</span>
                        <small>
                          长协 {selectedSector.order_book.last_contract_volume} · 现货 {selectedSector.order_book.last_spot_volume}
                        </small>
                      </div>
                      <div className="order-book-grid">
                        <div className="order-side asks" data-testid="order-book-asks">
                          {selectedSector.order_book.asks.length === 0 ? (
                            <span className="order-empty">无卖单</span>
                          ) : (
                            [...selectedSector.order_book.asks].reverse().map((lv) => (
                              <div key={`a${lv.price_cents}`} className="order-level ask">
                                <span>卖 {formatCents(lv.price_cents)}</span>
                                <span>{lv.qty}</span>
                              </div>
                            ))
                          )}
                        </div>
                        <div className="order-side bids" data-testid="order-book-bids">
                          {selectedSector.order_book.bids.length === 0 ? (
                            <span className="order-empty">无买单</span>
                          ) : (
                            selectedSector.order_book.bids.map((lv) => (
                              <div key={`b${lv.price_cents}`} className="order-level bid">
                                <span>买 {formatCents(lv.price_cents)}</span>
                                <span>{lv.qty}</span>
                              </div>
                            ))
                          )}
                        </div>
                      </div>
                      {selectedSector.order_book.recent_trades.length > 0 && (
                        <div className="order-trades">
                          {selectedSector.order_book.recent_trades.map((t, i) => (
                            <span key={i}>
                              {t.from_sector === t.to_sector ? '本地' : `#${t.from_sector + 1}→#${t.to_sector + 1}`} {t.qty} @ {formatCents(t.price_cents)}
                              {t.freight_cents > 0 ? `（含运费 ${formatCents(t.freight_cents)}）` : ''}
                            </span>
                          ))}
                        </div>
                      )}
                      <div className="order-book-head">
                        <span data-testid="sector-depot">
                          辖区官仓 苹果 {selectedSector.depot_apples}/{selectedSector.depot_capacity}
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
                            个体农余 {selectedSector.idle_pool_count}
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
                                : pg.employer_kind === 'unowned' && pg.id !== 'base:subsistence'
                                  ? `待业 ${pg.count}`
                                  : `在岗 ${pg.employed}/${pg.count}`}
                              {pg.contract_wage_per_100_cents > 0
                                ? ` · 合同 ${formatCents(pg.contract_wage_per_100_cents)}`
                                : ''}
                            </span>
                            <span className="pop-role-meta">
                              储蓄 {pg.savings} · 粮 {pg.pantry_apples}/{pg.pantry_capacity} · 食 {pg.food_met_percent}% · 满 {pg.satisfaction}%
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
                            onClick={() => setSelectedTileIndex(ent.campus_tile_index)}
                          >
                            <span>▲ {ent.name}（+{ent.output_bonus_percent}%产出）</span>
                            <span>
                              托管 {ent.hosted_count}/{ent.hosting_capacity} · #{String(ent.campus_tile_index + 1).padStart(4, '0')}
                            </span>
                          </button>
                        ))}
                      </div>
                    )}
                  </>
                ) : (
                  <p className="core-empty-note">远洋公海区域（无常住人口、本地市场与行政辖区）</p>
                )}

                <div className="region-slot-stats" data-testid="region-slot-stats">
                  <span>
                    主槽 {selectedRegionSummary.primary_used}/{selectedRegionSummary.primary_total}
                  </span>
                  <span>
                    次槽 {selectedRegionSummary.secondary_used}/{selectedRegionSummary.secondary_total}（已解锁空次槽 {selectedRegionSummary.unlocked_secondary_empty}）
                  </span>
                </div>

                <div className="region-core-bar" data-testid="region-core-status">
                  {selectedRegionSummary.core_tile_index !== null ? (
                    <button
                      type="button"
                      className="core-pill-btn active-core"
                      onClick={() => setSelectedTileIndex(selectedRegionSummary.core_tile_index)}
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
                  <div className="macro-btn-row">
                    <button
                      type="button"
                      className="macro-build-btn"
                      data-testid="region-build-orchard"
                      disabled={
                        selectedRegionSummary.unlocked_secondary_empty === 0
                        && selectedRegionSummary.primary_used >= selectedRegionSummary.primary_total
                      }
                      onClick={() =>
                        void handleBuildInRegion(planet.id, selectedRegion.id, BUILDING_APPLE_ORCHARD)
                      }
                    >
                      <span>+1 国营苹果园</span>
                      <small>
                        {selectedRegionSummary.unlocked_secondary_empty > 0
                          ? '次槽 150资金 · +20/月'
                          : '开主槽 300资金 · 2x主导+40/月'}
                      </small>
                    </button>
                    <button
                      type="button"
                      className="macro-build-btn"
                      data-testid="region-build-core"
                      disabled={
                        selectedRegionSummary.core_tile_index !== null
                        || selectedRegionSummary.land_primary_used >= selectedRegionSummary.land_primary_total
                      }
                      onClick={() =>
                        void handleBuildInRegion(planet.id, selectedRegion.id, BUILDING_REGIONAL_CORE)
                      }
                    >
                      <span>+1 大区核心</span>
                      <small>
                        {selectedRegionSummary.core_tile_index !== null
                          ? `已有${selectedRegionSummary.core_building_name ?? '核心'}`
                          : '主槽 400资金 · 150岗位'}
                      </small>
                    </button>
                    <button
                      type="button"
                      className="macro-build-btn"
                      data-testid="region-build-cbd"
                      disabled={selectedRegionSummary.core_tile_index === null && sectorEnterprises.length === 0}
                      onClick={() =>
                        void handleBuildInRegion(planet.id, selectedRegion.id, BUILDING_COMMERCIAL_STREET)
                      }
                    >
                      <span>+1 商业街</span>
                      <small>核心次槽 200资金 · +6私产名额</small>
                    </button>
                    <button
                      type="button"
                      className="macro-build-btn"
                      data-testid="region-build-campus"
                      disabled={
                        !selectedSector.can_found_enterprise
                        || selectedRegionSummary.land_primary_used >= selectedRegionSummary.land_primary_total
                      }
                      onClick={() =>
                        void handleBuildInRegion(planet.id, selectedRegion.id, BUILDING_CORPORATE_CAMPUS)
                      }
                    >
                      <span>▲ 批建企业园区</span>
                      <small>
                        {selectedSector.can_found_enterprise
                          ? '已满3座私产 · 350资金建总部'
                          : `需同业私产 ${selectedSector.unincorporated_ag_private}/3 座`}
                      </small>
                    </button>
                  </div>
                )}
              </div>
            )}

            <div className="surface-card" aria-label="具体地块槽位管理">
              <div className="card-header">
                <h2 className="card-title">
                  地块{' '}
                  <span className="tile-id-inline" data-testid="selected-tile-id">
                    #{String(activeTileIndex + 1).padStart(4, '0')}
                  </span>
                </h2>
                <div className="mode-segment">
                  <button
                    type="button"
                    className="inline-link-btn"
                    data-testid="pick-empty-tile"
                    onClick={selectNextEmptyLandTile}
                  >
                    空陆格
                  </button>
                  <button
                    type="button"
                    className="inline-link-btn"
                    data-testid="pick-sea-tile"
                    onClick={selectNextEmptySeaTile}
                  >
                    空海格
                  </button>
                </div>
              </div>

              {selectedTileGeo && (
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
                      <span className="tile-tag empty-tag">待确立主业</span>
                    )}
                  </div>

                  {primaryBuilding ? (
                    <div className="slot-occupied-col">
                      <div className="slot-occupied-row">
                        <span className="tile-building">{primaryBuilding.name}</span>
                        <span className="tile-output">{formatPlacedOutput(primaryBuilding)}</span>
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
                          {canRedevelopPrimary && !selectedIsWater && (selectedRegionSummary?.core_tile_index ?? null) === null && (
                            <button
                              type="button"
                              className="mini-action-btn"
                              data-testid="redevelop-primary-core"
                              onClick={() =>
                                void handleBuildOnTile(planet.id, activeTileIndex, BUILDING_REGIONAL_CORE, 'primary')
                              }
                            >
                              改建大区核心
                            </button>
                          )}
                          {canRedevelopPrimary && !selectedIsWater && Boolean(selectedSector?.can_found_enterprise) && (
                            <button
                              type="button"
                              className="mini-action-btn"
                              data-testid="redevelop-primary-campus"
                              onClick={() =>
                                void handleBuildOnTile(planet.id, activeTileIndex, BUILDING_CORPORATE_CAMPUS, 'primary')
                              }
                            >
                              改建企业园区
                            </button>
                          )}
                          {!primaryBuilding.tags.includes('planetary_capital') && !hasAnySecondaryOccupied && (
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
                  ) : (
                    <div className="slot-actions">
                      <button
                        type="button"
                        className="slot-build-btn"
                        data-testid="build-primary-core"
                        disabled={
                          selectedIsWater || (selectedRegionSummary?.core_tile_index ?? null) !== null
                        }
                        onClick={() =>
                          void handleBuildOnTile(
                            planet.id,
                            activeTileIndex,
                            BUILDING_REGIONAL_CORE,
                            'primary',
                          )
                        }
                      >
                        {selectedIsWater
                          ? '大区核心（仅陆格）'
                          : (selectedRegionSummary?.core_tile_index ?? null) !== null
                            ? '大区核心（本区已有）'
                            : '+ 大区核心（400）'}
                      </button>
                      <button
                        type="button"
                        className="slot-build-btn"
                        data-testid="build-primary-orchard"
                        onClick={() =>
                          void handleBuildOnTile(
                            planet.id,
                            activeTileIndex,
                            BUILDING_APPLE_ORCHARD,
                            'primary',
                          )
                        }
                      >
                        + 主导苹果园（2x·300）
                      </button>
                      {!selectedIsWater && Boolean(selectedSector?.can_found_enterprise) && (
                        <button
                          type="button"
                          className="slot-build-btn span-two"
                          data-testid="build-primary-campus"
                          onClick={() =>
                            void handleBuildOnTile(
                              planet.id,
                              activeTileIndex,
                              BUILDING_CORPORATE_CAMPUS,
                              'primary',
                            )
                          }
                        >
                          ▲ 批地设立企业园区（整合私产组建公司 · 350）
                        </button>
                      )}
                    </div>
                  )}
                </div>

                {!primaryBuilding ? (
                  <div className="secondary-locked-banner" data-testid="secondary-locked-hint">
                    次建筑槽（3 个）尚未解锁：请先在上方主槽确立该地块的主导产业或核心建筑。
                  </div>
                ) : (
                  <div className="secondary-grid">
                    {SECONDARY_KEYS.map(({ key, label, idx }) => {
                      const secBuilding = selectedOccupied?.secondary[idx] ?? null;
                      const badge = secBuilding ? formatStaffingBadge(secBuilding) : null;
                      return (
                        <div key={key} className="slot-box secondary-box" data-testid={`slot-${key}`}>
                          <div className="tile-top">
                            <span className="slot-label">{label}</span>
                            <span className={badge ? badge.className : 'tile-tag empty-tag'}>
                              {badge ? `${secBuilding!.jobs_filled}/${secBuilding!.jobs_required}` : '空'}
                            </span>
                          </div>
                          {secBuilding ? (
                            <div className="slot-occupied-col">
                              <span className="tile-building">{secBuilding.name}</span>
                              <span className="tile-output">{formatPlacedOutput(secBuilding)}</span>
                              <span
                                className={secBuilding.owner.kind === 'state' ? 'owner-badge state' : 'owner-badge private'}
                                data-testid={`owner-${key}`}
                              >
                                {secBuilding.owner.kind === 'state' ? '国有' : secBuilding.owner.kind === 'enterprise' ? secBuilding.owner.label : '民间私产'}
                              </span>
                              <div className="slot-mini-actions">
                                {secBuilding.owner.can_privatize && (
                                  <button
                                    type="button"
                                    className="mini-action-btn"
                                    data-testid={`privatize-${key}`}
                                    onClick={() => void handlePrivatizeSlot(planet.id, activeTileIndex, key)}
                                  >
                                    私有化
                                  </button>
                                )}
                                {secBuilding.owner.can_nationalize && (
                                  <button
                                    type="button"
                                    className="mini-action-btn"
                                    data-testid={`nationalize-${key}`}
                                    onClick={() => void handleNationalizeSlot(planet.id, activeTileIndex, key)}
                                  >
                                    国有化
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
                          ) : (
                            <div className="sec-build-stack">
                              <button
                                type="button"
                                className="slot-build-btn full-width"
                                data-testid={`build-${key}-orchard`}
                                onClick={() =>
                                  void handleBuildOnTile(
                                    planet.id,
                                    activeTileIndex,
                                    BUILDING_APPLE_ORCHARD,
                                    key,
                                  )
                                }
                              >
                                + 苹果园（150）
                              </button>
                              {tileHasUrbanOrCampusHub && !selectedIsWater && (
                                <button
                                  type="button"
                                  className="slot-build-btn full-width"
                                  data-testid={`build-${key}-cbd`}
                                  onClick={() =>
                                    void handleBuildOnTile(
                                      planet.id,
                                      activeTileIndex,
                                      BUILDING_COMMERCIAL_STREET,
                                      key,
                                    )
                                  }
                                >
                                  + 商业街（200）
                                </button>
                              )}
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          </section>
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
          实控辖区本地市场 / 1主导+3次要槽位 · 滚轮缩放 / 拖拽平移 <span className="muted">空格键暂停</span>
        </span>
      </footer>
    </main>
  );
}
