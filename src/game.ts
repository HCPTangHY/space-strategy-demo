import { Channel, invoke, isTauri } from '@tauri-apps/api/core';

export type GameSpeed = 0 | 1 | 2 | 3 | 4;

export type TileTagId =
  | 'land'
  | 'sea'
  | 'coastal'
  | 'coastal_sea'
  | 'deep_ocean'
  | 'inland_sea'
  | 'plains'
  | 'hills'
  | 'mountains'
  | 'forest'
  | 'arid'
  | 'tundra';

export const TILE_TAG_LABELS: Record<TileTagId, string> = {
  land: '陆地',
  sea: '海域',
  coastal: '沿海',
  coastal_sea: '近海大陆架',
  deep_ocean: '远洋深海',
  inland_sea: '内陆湖海',
  plains: '平原',
  hills: '丘陵',
  mountains: '山脉',
  forest: '树林',
  arid: '荒漠',
  tundra: '冻土',
};

export function isWaterTags(tags: readonly TileTagId[]): boolean {
  return tags.includes('sea');
}

export type SlotRule = 'primary_only' | 'secondary_only' | 'any';

export type JobPriority = 'core' | 'commercial' | 'standard';

export type BuildingTagId =
  | 'planetary_capital'
  | 'regional_core'
  | 'agriculture'
  | 'commercial'
  | 'corporate_campus';

export const BUILDING_TAG_LABELS: Record<BuildingTagId, string> = {
  planetary_capital: '行星首府',
  regional_core: '大区核心',
  agriculture: '农业设施',
  commercial: '商业街',
  corporate_campus: '企业园区',
};

export type TileSlotKey = 'primary' | 'secondary0' | 'secondary1' | 'secondary2';

export const BUILDING_APPLE_ORCHARD = 'base:apple_orchard';
export const BUILDING_REGIONAL_CORE = 'base:regional_core';
export const BUILDING_PLANETARY_CAPITAL = 'base:planetary_capital';
export const BUILDING_COMMERCIAL_STREET = 'base:commercial_street';
export const BUILDING_CORPORATE_CAMPUS = 'base:corporate_campus';

export interface TimeSnapshot {
  epoch_name: string;
  year: number;
  month: number;
  day: number;
  subtick: number;
  formatted: string;
  elapsed_days: number;
  speed: GameSpeed;
  active_combats: number;
}

export type MonetaryPolicy = 'tight' | 'moderate' | 'loose';
export type DepotPolicy = 'idle' | 'stockpile' | 'stabilize';

export const MONETARY_POLICY_NEXT: Record<MonetaryPolicy, MonetaryPolicy> = {
  tight: 'moderate',
  moderate: 'loose',
  loose: 'tight',
};
export const DEPOT_POLICY_NEXT: Record<DepotPolicy, DepotPolicy> = {
  idle: 'stockpile',
  stockpile: 'stabilize',
  stabilize: 'idle',
};

export interface PriceLevelSnapshot {
  price_cents: number;
  qty: number;
  cumulative_qty: number;
}

export interface TradeFillRecord {
  price_cents: number;
  qty: number;
  from_sector: number;
  to_sector: number;
  freight_cents: number;
}

export interface OrderBookSnapshot {
  last_price_cents: number;
  prev_price_cents: number;
  ma_price_cents: number;
  best_ask_cents: number | null;
  best_bid_cents: number | null;
  asks: PriceLevelSnapshot[];
  bids: PriceLevelSnapshot[];
  last_contract_volume: number;
  last_spot_volume: number;
  last_net_import: number;
  recent_trades: TradeFillRecord[];
}

/** Formats integer cents as credits with 2 decimals (e.g. 1050 -> "10.50"). */
export function formatCents(cents: number): string {
  return (cents / 100).toFixed(2);
}

export interface PolitySnapshot {
  id: number;
  name: string;
  treasury: number;
  treasury_cents: number;
  monetary_policy: MonetaryPolicy;
  monetary_policy_label: string;
  tax_rate_percent: number;
  last_freight_revenue: number;
  last_seigniorage: number;
  last_tax_income: number;
  last_state_enterprise_net: number;
  last_admin_upkeep: number;
  last_net_cashflow: number;
}

export interface PopTypeSnapshot {
  id: string;
  name: string;
  base_wage_per_100: number;
  is_default_pool: boolean;
  is_vagrant_pool: boolean;
  receives_dividends: boolean;
}

export interface PopGroupSnapshot {
  pop_id: number;
  id: string;
  culture_id: number;
  employer_kind: string;
  employer_label: string;
  name: string;
  count: number;
  employed: number;
  contract_wage_per_100_cents: number;
  savings: number;
  pantry_apples: number;
  pantry_capacity: number;
  satisfaction: number;
  last_income: number;
  food_met_percent: number;
}

export interface RoleWageSnapshot {
  id: string;
  name: string;
  wage_per_100_cents: number;
  ma_wage_per_100_cents: number;
  last_hired: number;
  last_laid_off: number;
}

export interface EnterpriseSnapshot {
  id: number;
  name: string;
  industry_tag: string;
  hq_sector_id: number;
  campus_tile_index: number;
  hosted_count: number;
  hosting_capacity: number;
  output_bonus_percent: number;
  last_revenue: number;
  last_profit: number;
}

export interface BuildingOwnerSnapshot {
  kind: 'state' | 'local_private' | 'enterprise' | 'trust' | string;
  label: string;
  home_sector_id: number | null;
  enterprise_id: number | null;
  can_privatize: boolean;
  privatize_price: number;
  can_nationalize: boolean;
  nationalize_cost: number;
}

export interface SectorSnapshot {
  id: number;
  geo_region_id: number;
  owner_polity_id: number;
  name: string;
  allow_private_land_use: boolean;
  population: number;
  monthly_growth: number;
  jobs_required: number;
  jobs_filled: number;
  free_labor: number;
  idle_pool_count: number;
  unemployed_count: number;
  vagrant_count: number;
  active_vacancies: number;
  average_satisfaction: number;
  apple_price: number;
  apple_price_cents: number;
  ma_apple_price_cents: number;
  order_book: OrderBookSnapshot;
  wage_quotes: RoleWageSnapshot[];
  depot_apples: number;
  depot_capacity: number;
  depot_policy: DepotPolicy;
  depot_policy_label: string;
  local_apple_stockpile: number;
  apple_supply: number;
  apple_demand: number;
  net_trade_in: number;
  investment_pool: number;
  last_dividends: number;
  cbd_count: number;
  cbd_hosted: number;
  cbd_capacity: number;
  unincorporated_ag_private: number;
  can_found_enterprise: boolean;
  pops: PopGroupSnapshot[];
}

export interface ResourceStockSnapshot {
  id: string;
  name: string;
  amount: number;
  monthly_delta: number;
}

export interface BuildingSnapshot {
  id: string;
  name: string;
  slot_rule: SlotRule;
  job_priority: JobPriority;
  build_cost: number;
  hosting_capacity: number;
  jobs_required: number;
  role_jobs: Record<string, number>;
  tags: BuildingTagId[];
  required_tile_tags: TileTagId[];
  monthly_output: Record<string, number>;
}

export interface PlacedBuildingSnapshot {
  id: string;
  name: string;
  slot_rule: SlotRule;
  scale_multiplier: number;
  job_priority: JobPriority;
  tags: BuildingTagId[];
  jobs_required: number;
  jobs_filled: number;
  hosting_capacity: number;
  owner: BuildingOwnerSnapshot;
  base_monthly_output: Record<string, number>;
  effective_monthly_output: Record<string, number>;
}

export interface OccupiedTileSnapshot {
  index: number;
  region_id: number;
  sector_id: number | null;
  tags: TileTagId[];
  habitability: number;
  primary: PlacedBuildingSnapshot | null;
  secondary: [PlacedBuildingSnapshot | null, PlacedBuildingSnapshot | null, PlacedBuildingSnapshot | null];
}

export interface RegionSummarySnapshot {
  region_id: number;
  sector_id: number | null;
  population: number;
  monthly_growth: number;
  jobs_required: number;
  jobs_filled: number;
  free_labor: number;
  primary_used: number;
  primary_total: number;
  land_primary_used: number;
  land_primary_total: number;
  secondary_used: number;
  secondary_total: number;
  unlocked_secondary_empty: number;
  core_tile_index: number | null;
  core_building_name: string | null;
  building_counts: Record<string, number>;
}

export interface PlanetSnapshot {
  id: string;
  name: string;
  radius_km: number;
  position: { x: number; y: number };
  appearance: {
    ocean_rgb: [number, number, number];
    land_rgb: [number, number, number];
    seed: number;
  };
  polity: PolitySnapshot;
  pop_catalog: PopTypeSnapshot[];
  stockpile: Record<string, number>;
  resources: ResourceStockSnapshot[];
  building_catalog: BuildingSnapshot[];
  tile_count: number;
  land_tile_count: number;
  sea_tile_count: number;
  region_count: number;
  starter_tile_index: number;
  capital_tile_index: number | null;
  total_population: number;
  monthly_population_growth: number;
  total_jobs_required: number;
  total_jobs_filled: number;
  total_building_count: number;
  sectors: SectorSnapshot[];
  enterprises: EnterpriseSnapshot[];
  region_summaries: RegionSummarySnapshot[];
  occupied_tiles: OccupiedTileSnapshot[];
}

export interface SurfaceRegion {
  id: number;
  name: string;
  is_ocean: boolean;
  color_rgb: [number, number, number];
  center: [number, number];
  tile_count: number;
  land_tile_count: number;
  sea_tile_count: number;
}

export interface SurfacePolygonTile {
  index: number;
  region_id: number;
  tags: TileTagId[];
  elevation: number;
  habitability: number;
  center: [number, number];
  polygon: [number, number][];
  neighbors: number[];
}

export interface PlanetSurfaceSnapshot {
  planet_id: string;
  seed: number;
  width: number;
  height: number;
  grid_cell_count: number;
  land_tile_count: number;
  coastal_sea_count: number;
  deep_ocean_count: number;
  ocean_merged_count: number;
  regions: SurfaceRegion[];
  tiles: SurfacePolygonTile[];
  land_loops: [number, number][][];
  lake_loops: [number, number][][];
  shelf_loops: [number, number][][];
  region_borders: [[number, number], [number, number]][];
}

export type StreamEvent =
  | { kind: 'init'; time: TimeSnapshot; default_planet_id: string | null }
  | { kind: 'tick'; time: TimeSnapshot }
  | { kind: 'planet_surface'; surface: PlanetSurfaceSnapshot | null }
  | { kind: 'planet_view'; planet: PlanetSnapshot | null }
  | { kind: 'command_error'; message: string };

export interface GameStreamHandlers {
  onEvent: (event: StreamEvent) => void;
  onDisconnect: (reason: string) => void;
}

export interface GameStreamHandle {
  setSpeed: (speed: GameSpeed) => Promise<void>;
  watchPlanet: (planetId: string | null) => Promise<void>;
  buildOnTile: (
    planetId: string,
    tileIndex: number,
    buildingId: string,
    slot?: TileSlotKey,
  ) => Promise<void>;
  buildInRegion: (planetId: string, regionId: number, buildingId: string) => Promise<void>;
  demolishOnTile: (planetId: string, tileIndex: number, slot: TileSlotKey) => Promise<void>;
  privatizeSlot: (planetId: string, tileIndex: number, slot: TileSlotKey) => Promise<void>;
  nationalizeSlot: (planetId: string, tileIndex: number, slot: TileSlotKey) => Promise<void>;
  togglePrivateLandUse: (planetId: string, sectorId: number, allow: boolean) => Promise<void>;
  setMonetaryPolicy: (planetId: string, policy: MonetaryPolicy) => Promise<void>;
  setDepotPolicy: (planetId: string, sectorId: number, policy: DepotPolicy) => Promise<void>;
  close: () => void;
}

export const connectionLabel = isTauri()
  ? 'Rust 内核 · Tauri Channel'
  : 'Rust 内核 · WebSocket 推送';

export function connectGameStream(handlers: GameStreamHandlers): GameStreamHandle {
  if (isTauri()) {
    return connectTauriStream(handlers);
  }
  return connectWebSocketStream(handlers);
}

function connectTauriStream(handlers: GameStreamHandlers): GameStreamHandle {
  let closed = false;
  let subId: number | null = null;
  const channel = new Channel<StreamEvent>();
  channel.onmessage = (raw: unknown) => {
    if (closed) return;
    if (!isStreamEvent(raw)) {
      handlers.onDisconnect('桌面通道返回的数据格式无效。');
      return;
    }
    handlers.onEvent(raw);
  };

  invoke<number>('subscribe_stream', { planetId: null, channel })
    .then((id) => {
      if (closed) {
        void invoke('unsubscribe_stream', { subId: id });
      } else {
        subId = id;
      }
    })
    .catch((err: unknown) => {
      if (!closed) handlers.onDisconnect(String(err));
    });

  return {
    async setSpeed(speed: GameSpeed) {
      await invoke('set_game_speed', { speed });
    },
    async watchPlanet(planetId: string | null) {
      if (subId === null) throw new Error('桌面状态通道尚未就绪。');
      await invoke('watch_planet', { subId, planetId });
    },
    async buildOnTile(planetId: string, tileIndex: number, buildingId: string, slot?: TileSlotKey) {
      await invoke('build_on_tile', { planetId, tileIndex, buildingId, slot: slot ?? null });
    },
    async buildInRegion(planetId: string, regionId: number, buildingId: string) {
      await invoke('build_in_region', { planetId, regionId, buildingId });
    },
    async demolishOnTile(planetId: string, tileIndex: number, slot: TileSlotKey) {
      await invoke('demolish_on_tile', { planetId, tileIndex, slot });
    },
    async privatizeSlot(planetId: string, tileIndex: number, slot: TileSlotKey) {
      await invoke('privatize_slot', { planetId, tileIndex, slot });
    },
    async nationalizeSlot(planetId: string, tileIndex: number, slot: TileSlotKey) {
      await invoke('nationalize_slot', { planetId, tileIndex, slot });
    },
    async togglePrivateLandUse(planetId: string, sectorId: number, allow: boolean) {
      await invoke('toggle_private_land_use', { planetId, sectorId, allow });
    },
    async setMonetaryPolicy(planetId: string, policy: MonetaryPolicy) {
      await invoke('set_monetary_policy', { planetId, policy });
    },
    async setDepotPolicy(planetId: string, sectorId: number, policy: DepotPolicy) {
      await invoke('set_depot_policy', { planetId, sectorId, policy });
    },
    close() {
      closed = true;
      if (subId !== null) {
        void invoke('unsubscribe_stream', { subId });
      }
    },
  };
}

function connectWebSocketStream(handlers: GameStreamHandlers): GameStreamHandle {
  let closedByClient = false;
  let opened = false;
  let notified = false;

  const notifyDisconnect = (reason: string) => {
    if (closedByClient || notified) return;
    notified = true;
    handlers.onDisconnect(reason);
  };

  const wsUrl = new URL(`${import.meta.env.BASE_URL}api/ws`, window.location.href);
  wsUrl.protocol = wsUrl.protocol === 'https:' ? 'wss:' : 'ws:';

  const socket = new WebSocket(wsUrl.href);
  const connectTimer = window.setTimeout(() => {
    if (!opened && !closedByClient) {
      socket.close();
      notifyDisconnect('连接 Rust 状态流超时，请检查后端服务。');
    }
  }, 8000);

  socket.onopen = () => {
    opened = true;
    window.clearTimeout(connectTimer);
  };

  socket.onmessage = (message) => {
    if (closedByClient || typeof message.data !== 'string') return;
    try {
      const parsed: unknown = JSON.parse(message.data);
      if (!isStreamEvent(parsed)) {
        socket.close();
        notifyDisconnect('Rust 状态流返回的数据格式无效。');
        return;
      }
      handlers.onEvent(parsed);
    } catch {
      socket.close();
      notifyDisconnect('无法解析 Rust 状态流消息。');
    }
  };

  socket.onerror = () => {};

  socket.onclose = () => {
    window.clearTimeout(connectTimer);
    notifyDisconnect(
      opened
        ? '与 Rust 状态流的连接已断开，请重新连接。'
        : '无法连接 Rust WebSocket 状态流，请检查服务与代理配置。',
    );
  };

  const sendJson = (payload: Record<string, unknown>, actionName: string) => {
    if (socket.readyState !== WebSocket.OPEN) {
      throw new Error(`状态流尚未连接，无法${actionName}。`);
    }
    socket.send(JSON.stringify(payload));
  };

  return {
    async setSpeed(speed: GameSpeed) {
      sendJson({ cmd: 'set_speed', speed }, '修改游戏速度');
    },
    async watchPlanet(planetId: string | null) {
      sendJson({ cmd: 'watch_planet', planet_id: planetId }, '切换星球视图');
    },
    async buildOnTile(planetId: string, tileIndex: number, buildingId: string, slot?: TileSlotKey) {
      sendJson(
        {
          cmd: 'build_on_tile',
          planet_id: planetId,
          tile_index: tileIndex,
          building_id: buildingId,
          slot: slot ?? null,
        },
        '在地块上建造',
      );
    },
    async buildInRegion(planetId: string, regionId: number, buildingId: string) {
      sendJson(
        {
          cmd: 'build_in_region',
          planet_id: planetId,
          region_id: regionId,
          building_id: buildingId,
        },
        '在大区建造',
      );
    },
    async demolishOnTile(planetId: string, tileIndex: number, slot: TileSlotKey) {
      sendJson(
        {
          cmd: 'demolish_on_tile',
          planet_id: planetId,
          tile_index: tileIndex,
          slot,
        },
        '拆除建筑',
      );
    },
    async privatizeSlot(planetId: string, tileIndex: number, slot: TileSlotKey) {
      sendJson(
        {
          cmd: 'privatize_slot',
          planet_id: planetId,
          tile_index: tileIndex,
          slot,
        },
        '挂牌私有化',
      );
    },
    async nationalizeSlot(planetId: string, tileIndex: number, slot: TileSlotKey) {
      sendJson(
        {
          cmd: 'nationalize_slot',
          planet_id: planetId,
          tile_index: tileIndex,
          slot,
        },
        '赎买国有化',
      );
    },
    async togglePrivateLandUse(planetId: string, sectorId: number, allow: boolean) {
      sendJson(
        {
          cmd: 'toggle_private_land_use',
          planet_id: planetId,
          sector_id: sectorId,
          allow,
        },
        '切换用地许可',
      );
    },
    async setMonetaryPolicy(planetId: string, policy: MonetaryPolicy) {
      sendJson({ cmd: 'set_monetary_policy', planet_id: planetId, policy }, '调整货币政策');
    },
    async setDepotPolicy(planetId: string, sectorId: number, policy: DepotPolicy) {
      sendJson(
        { cmd: 'set_depot_policy', planet_id: planetId, sector_id: sectorId, policy },
        '调整官仓政策',
      );
    },
    close() {
      closedByClient = true;
      window.clearTimeout(connectTimer);
      socket.close();
    },
  };
}

function isGameSpeed(value: unknown): value is GameSpeed {
  return value === 0 || value === 1 || value === 2 || value === 3 || value === 4;
}

function isTileTagId(value: unknown): value is TileTagId {
  return typeof value === 'string' && value in TILE_TAG_LABELS;
}

function isTagList(value: unknown): value is TileTagId[] {
  return Array.isArray(value) && value.every(isTileTagId);
}

function isPoint2d(value: unknown): value is [number, number] {
  return (
    Array.isArray(value)
    && value.length === 2
    && typeof value[0] === 'number'
    && Number.isFinite(value[0])
    && typeof value[1] === 'number'
    && Number.isFinite(value[1])
  );
}

function isRgbTuple(color: unknown): color is [number, number, number] {
  return (
    Array.isArray(color)
    && color.length === 3
    && color.every((channel) => Number.isInteger(channel) && channel >= 0 && channel <= 255)
  );
}

function isIntMap(value: unknown): value is Record<string, number> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  return Object.values(value).every((v) => typeof v === 'number' && Number.isInteger(v));
}

function isSlotRule(value: unknown): value is SlotRule {
  return value === 'primary_only' || value === 'secondary_only' || value === 'any';
}

function isJobPriority(value: unknown): value is JobPriority {
  return value === 'core' || value === 'commercial' || value === 'standard';
}

function isBuildingTagId(value: unknown): value is BuildingTagId {
  return typeof value === 'string' && value in BUILDING_TAG_LABELS;
}

function isOwnerSnapshot(value: unknown): value is BuildingOwnerSnapshot {
  if (!value || typeof value !== 'object') return false;
  const o = value as Partial<BuildingOwnerSnapshot>;
  return (
    typeof o.kind === 'string'
    && typeof o.label === 'string'
    && typeof o.can_privatize === 'boolean'
    && typeof o.privatize_price === 'number'
    && typeof o.can_nationalize === 'boolean'
    && typeof o.nationalize_cost === 'number'
  );
}

function isBuildingSnapshot(value: unknown): value is BuildingSnapshot {
  if (!value || typeof value !== 'object') return false;
  const b = value as Partial<BuildingSnapshot>;
  return (
    typeof b.id === 'string'
    && typeof b.name === 'string'
    && isSlotRule(b.slot_rule)
    && isJobPriority(b.job_priority)
    && typeof b.build_cost === 'number'
    && typeof b.hosting_capacity === 'number'
    && typeof b.jobs_required === 'number' && Number.isInteger(b.jobs_required)
    && isIntMap(b.role_jobs)
    && Array.isArray(b.tags)
    && b.tags.every(isBuildingTagId)
    && isTagList(b.required_tile_tags)
    && isIntMap(b.monthly_output)
  );
}

function isPlacedBuildingSnapshot(value: unknown): value is PlacedBuildingSnapshot {
  if (!value || typeof value !== 'object') return false;
  const b = value as Partial<PlacedBuildingSnapshot>;
  return (
    typeof b.id === 'string'
    && typeof b.name === 'string'
    && isSlotRule(b.slot_rule)
    && typeof b.scale_multiplier === 'number'
    && isJobPriority(b.job_priority)
    && Array.isArray(b.tags)
    && b.tags.every(isBuildingTagId)
    && typeof b.jobs_required === 'number' && Number.isInteger(b.jobs_required)
    && typeof b.jobs_filled === 'number' && Number.isInteger(b.jobs_filled)
    && typeof b.hosting_capacity === 'number'
    && isOwnerSnapshot(b.owner)
    && isIntMap(b.base_monthly_output)
    && isIntMap(b.effective_monthly_output)
  );
}

function isOptionalPlacedBuilding(value: unknown): value is PlacedBuildingSnapshot | null {
  return value === null || isPlacedBuildingSnapshot(value);
}

function isResourceStock(value: unknown): value is ResourceStockSnapshot {
  if (!value || typeof value !== 'object') return false;
  const item = value as Partial<ResourceStockSnapshot>;
  return (
    typeof item.id === 'string'
    && typeof item.name === 'string'
    && typeof item.amount === 'number' && Number.isInteger(item.amount)
    && typeof item.monthly_delta === 'number' && Number.isInteger(item.monthly_delta)
  );
}

function isOccupiedTileSnapshot(value: unknown): value is OccupiedTileSnapshot {
  if (!value || typeof value !== 'object') return false;
  const tile = value as Partial<OccupiedTileSnapshot>;
  return (
    typeof tile.index === 'number'
    && Number.isInteger(tile.index)
    && tile.index >= 0
    && typeof tile.region_id === 'number'
    && Number.isInteger(tile.region_id)
    && (tile.sector_id === null || typeof tile.sector_id === 'number')
    && isTagList(tile.tags)
    && typeof tile.habitability === 'number'
    && Number.isInteger(tile.habitability)
    && isOptionalPlacedBuilding(tile.primary)
    && Array.isArray(tile.secondary)
    && tile.secondary.length === 3
    && tile.secondary.every(isOptionalPlacedBuilding)
  );
}

function isRegionSummary(value: unknown): value is RegionSummarySnapshot {
  if (!value || typeof value !== 'object') return false;
  const r = value as Partial<RegionSummarySnapshot>;
  return (
    typeof r.region_id === 'number'
    && (r.sector_id === null || typeof r.sector_id === 'number')
    && typeof r.population === 'number' && Number.isInteger(r.population)
    && typeof r.monthly_growth === 'number' && Number.isInteger(r.monthly_growth)
    && typeof r.jobs_required === 'number' && Number.isInteger(r.jobs_required)
    && typeof r.jobs_filled === 'number' && Number.isInteger(r.jobs_filled)
    && typeof r.free_labor === 'number' && Number.isInteger(r.free_labor)
    && typeof r.primary_used === 'number'
    && typeof r.primary_total === 'number'
    && typeof r.land_primary_used === 'number'
    && typeof r.land_primary_total === 'number'
    && typeof r.secondary_used === 'number'
    && typeof r.secondary_total === 'number'
    && typeof r.unlocked_secondary_empty === 'number'
    && (r.core_tile_index === null || (typeof r.core_tile_index === 'number' && Number.isInteger(r.core_tile_index)))
    && (r.core_building_name === null || typeof r.core_building_name === 'string')
    && isIntMap(r.building_counts)
  );
}

function isPolitySnapshot(value: unknown): value is PolitySnapshot {
  if (!value || typeof value !== 'object') return false;
  const p = value as Partial<PolitySnapshot>;
  return (
    typeof p.id === 'number'
    && typeof p.name === 'string'
    && typeof p.treasury === 'number'
    && typeof p.monetary_policy === 'string'
    && typeof p.tax_rate_percent === 'number'
    && typeof p.last_net_cashflow === 'number'
  );
}

function isSectorSnapshot(value: unknown): value is SectorSnapshot {
  if (!value || typeof value !== 'object') return false;
  const s = value as Partial<SectorSnapshot>;
  return (
    typeof s.id === 'number'
    && typeof s.geo_region_id === 'number'
    && typeof s.name === 'string'
    && typeof s.allow_private_land_use === 'boolean'
    && typeof s.population === 'number'
    && typeof s.apple_price === 'number'
    && typeof s.investment_pool === 'number'
    && typeof s.depot_policy === 'string'
    && !!s.order_book
    && Array.isArray(s.order_book.asks)
    && Array.isArray(s.order_book.bids)
    && Array.isArray(s.wage_quotes)
    && Array.isArray(s.pops)
  );
}

function isTimeSnapshot(value: unknown): value is TimeSnapshot {
  if (!value || typeof value !== 'object') return false;
  const time = value as Partial<TimeSnapshot>;
  return (
    typeof time.epoch_name === 'string'
    && typeof time.year === 'number' && Number.isInteger(time.year) && time.year !== 0
    && typeof time.month === 'number' && Number.isInteger(time.month) && time.month >= 1 && time.month <= 12
    && typeof time.day === 'number' && Number.isInteger(time.day) && time.day >= 1 && time.day <= 30
    && typeof time.subtick === 'number' && Number.isInteger(time.subtick) && time.subtick >= 0 && time.subtick <= 9
    && typeof time.formatted === 'string' && time.formatted.length > 0
    && typeof time.elapsed_days === 'number' && Number.isInteger(time.elapsed_days) && time.elapsed_days >= 0
    && isGameSpeed(time.speed)
    && typeof time.active_combats === 'number' && Number.isInteger(time.active_combats) && time.active_combats >= 0
  );
}

function isPlanetSnapshot(value: unknown): value is PlanetSnapshot {
  if (!value || typeof value !== 'object') return false;
  const planet = value as Partial<PlanetSnapshot>;
  return (
    typeof planet.id === 'string'
    && typeof planet.name === 'string'
    && typeof planet.radius_km === 'number' && Number.isFinite(planet.radius_km) && planet.radius_km > 0
    && typeof planet.position?.x === 'number' && Number.isFinite(planet.position.x)
    && typeof planet.position?.y === 'number' && Number.isFinite(planet.position.y)
    && isRgbTuple(planet.appearance?.ocean_rgb)
    && isRgbTuple(planet.appearance?.land_rgb)
    && typeof planet.appearance?.seed === 'number' && Number.isInteger(planet.appearance.seed)
    && isPolitySnapshot(planet.polity)
    && Array.isArray(planet.pop_catalog)
    && isIntMap(planet.stockpile)
    && Array.isArray(planet.resources) && planet.resources.every(isResourceStock)
    && Array.isArray(planet.building_catalog) && planet.building_catalog.every(isBuildingSnapshot)
    && typeof planet.tile_count === 'number' && Number.isInteger(planet.tile_count) && planet.tile_count > 0
    && typeof planet.land_tile_count === 'number' && Number.isInteger(planet.land_tile_count)
    && typeof planet.sea_tile_count === 'number' && Number.isInteger(planet.sea_tile_count)
    && typeof planet.region_count === 'number' && Number.isInteger(planet.region_count) && planet.region_count > 0
    && typeof planet.starter_tile_index === 'number' && Number.isInteger(planet.starter_tile_index)
    && (planet.capital_tile_index === null || (typeof planet.capital_tile_index === 'number' && Number.isInteger(planet.capital_tile_index)))
    && typeof planet.total_population === 'number' && Number.isInteger(planet.total_population)
    && typeof planet.monthly_population_growth === 'number' && Number.isInteger(planet.monthly_population_growth)
    && typeof planet.total_jobs_required === 'number' && Number.isInteger(planet.total_jobs_required)
    && typeof planet.total_jobs_filled === 'number' && Number.isInteger(planet.total_jobs_filled)
    && typeof planet.total_building_count === 'number' && Number.isInteger(planet.total_building_count)
    && Array.isArray(planet.sectors) && planet.sectors.every(isSectorSnapshot)
    && Array.isArray(planet.enterprises)
    && Array.isArray(planet.region_summaries) && planet.region_summaries.every(isRegionSummary)
    && Array.isArray(planet.occupied_tiles) && planet.occupied_tiles.every(isOccupiedTileSnapshot)
  );
}

function isPlanetSurfaceSnapshot(value: unknown): value is PlanetSurfaceSnapshot {
  if (!value || typeof value !== 'object') return false;
  const s = value as Partial<PlanetSurfaceSnapshot>;
  if (
    typeof s.planet_id !== 'string'
    || typeof s.seed !== 'number'
    || typeof s.width !== 'number' || s.width <= 0
    || typeof s.height !== 'number' || s.height <= 0
    || typeof s.grid_cell_count !== 'number'
    || typeof s.land_tile_count !== 'number'
    || typeof s.coastal_sea_count !== 'number'
    || typeof s.deep_ocean_count !== 'number'
    || typeof s.ocean_merged_count !== 'number'
    || !Array.isArray(s.regions)
    || !Array.isArray(s.tiles)
    || !Array.isArray(s.land_loops)
    || !Array.isArray(s.lake_loops)
    || !Array.isArray(s.shelf_loops)
    || !Array.isArray(s.region_borders)
  ) {
    return false;
  }
  const validRegion = (r: unknown): boolean => {
    if (!r || typeof r !== 'object') return false;
    const item = r as Partial<SurfaceRegion>;
    return (
      typeof item.id === 'number'
      && typeof item.name === 'string'
      && typeof item.is_ocean === 'boolean'
      && isRgbTuple(item.color_rgb)
      && isPoint2d(item.center)
      && typeof item.tile_count === 'number'
      && typeof item.land_tile_count === 'number'
      && typeof item.sea_tile_count === 'number'
    );
  };
  const validTile = (t: unknown): boolean => {
    if (!t || typeof t !== 'object') return false;
    const item = t as Partial<SurfacePolygonTile>;
    return (
      typeof item.index === 'number'
      && typeof item.region_id === 'number'
      && isTagList(item.tags)
      && typeof item.elevation === 'number'
      && typeof item.habitability === 'number'
      && isPoint2d(item.center)
      && Array.isArray(item.polygon) && item.polygon.length >= 3 && item.polygon.every(isPoint2d)
      && Array.isArray(item.neighbors)
    );
  };
  return s.regions.every(validRegion) && s.tiles.every(validTile);
}

function isStreamEvent(value: unknown): value is StreamEvent {
  if (!value || typeof value !== 'object') return false;
  const event = value as Record<string, unknown>;
  if (event.kind === 'init') {
    return (
      isTimeSnapshot(event.time)
      && (event.default_planet_id === null || typeof event.default_planet_id === 'string')
    );
  }
  if (event.kind === 'tick') {
    return isTimeSnapshot(event.time);
  }
  if (event.kind === 'planet_surface') {
    return event.surface === null || isPlanetSurfaceSnapshot(event.surface);
  }
  if (event.kind === 'planet_view') {
    return event.planet === null || isPlanetSnapshot(event.planet);
  }
  if (event.kind === 'command_error') {
    return typeof event.message === 'string';
  }
  return false;
}
