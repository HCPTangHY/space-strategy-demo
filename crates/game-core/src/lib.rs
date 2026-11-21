//! Pure game state and simulation clock: no Tauri, filesystem, rendering, or network dependencies.
//!
//! Module organization (`机制与数据分离 + 按经济层级与对象模块化`):
//! - `data_bundle` / `rules`: External JSONC catalog loader & declarative event/rule engine
//! - `calendar` / `surface`: Time clock primitives & planetary Voronoi geography engine
//! - `building` / `planet`: Building & slot objects, planetary spatial container, objective output evaluation
//! - `economy` / `market`: Households, polities, workshops, corporations & spatial order-book clearing
//! - `polity_ops` / `workshop_ops` / `corporate_ops`: Tiered entity commands (Polity / Slot & Labor / Corp & Finance)
//! - `decision` / `settlement`: Monthly autonomous entity decisions & 12-phase conservation clearing
//! - `scenario`: Isolated starting world & Earth bootstrap presets
//! - `protocol` / `snapshot` / `runtime`: Wire DTOs, read-only view projection & worker thread adapter

pub mod actors;
pub mod ai;
pub mod building;
pub mod calendar;
mod corporate_ops;
pub mod data_bundle;
pub use actors as economy;
pub mod hot_reload;
pub mod market;
pub mod modifiers;
pub mod planet;
mod polity_ops;
pub mod protocol;
pub mod rules;
pub mod runtime;
pub mod scenario;
mod settlement;
mod snapshot;
pub mod surface;
pub mod workshop_ops;

#[cfg(test)]
mod tests;

use hecs::World;
use std::time::Duration;

pub use building::{
    good_display_name, scale_output_by_staffing, BuildingCatalog, BuildingDef, BuildingTag,
    BuildingTags, BuildingTypeId, JobPriority, SlotRule, TileSlotKey, TileSlots,
    BUILDING_APPLE_ORCHARD, BUILDING_COLONIAL_OUTPOST, BUILDING_COMMERCIAL_STREET,
    BUILDING_CORPORATE_CAMPUS, BUILDING_PLANETARY_CAPITAL, BUILDING_REGIONAL_CORE,
    SECONDARY_SLOT_COUNT, SLOTS_PER_TILE,
};
#[cfg(test)]
pub use building::default_building_defs;
pub use calendar::{
    CalendarConfig, GameSpeed, InCombat, StepOutcome, TickCounters, DAYS_PER_MONTH, DAYS_PER_YEAR,
    MONTHS_PER_YEAR, SUBTICKS_PER_DAY,
};
pub use data_bundle::{
    load_economy_v1_bundle, load_full_gamedata, load_full_gamedata_from_dir, parse_jsonc,
    parse_jsonc_list_named, parse_jsonc_named, resolve_gamedata_dir, strip_jsonc, EconomyBundle,
    EconomyBundleDef, FullGameData, TransportHubDef, TransportHubKind,
};
pub use economy::{
    compute_primary_staple_demand, BilateralSupplyContract, BuildingOwner, BuildingOwnerSnapshot,
    CounterpartyId, DepotPolicy, Enterprise, EnterpriseId, EnterpriseSnapshot, GoodInfoSnapshot,
    MonetaryPolicy, Polity, PolityId, PolitySnapshot, PopGroup, PopGroupSnapshot, PopId,
    PopTypeCatalog, PopTypeDef, PopTypeId, PopTypeSnapshot, RoleWageSnapshot, Sector,
    SectorCapital, SectorGoodBookSnapshot, SectorId, SectorMarket, SectorPops, SectorSnapshot,
    SectorTradeLink, StaffContract, Trust, TrustId, UnpaidWageClaim, Workshop,
    BASE_SECTOR_CAPITAL_POOL_CAP_CENTS, BASE_UNINCORPORATED_PRIVATE_CAP, CAMPUS_HOSTING_CAPACITY,
    CBD_HOSTING_CAPACITY, ENTERPRISE_FOUNDING_MIN_PRIVATE, ENTERPRISE_OUTPUT_BONUS_PERMILLE,
    POP_ACADEMIC, POP_BUREAUCRAT, POP_CAPITALIST, POP_ENGINEER, POP_LABORER, POP_SOLDIER,
    POP_VAGRANT, SECTOR_CAPITAL_OVERFLOW_CAP_CENTS,
};
#[cfg(test)]
pub use economy::default_pop_type_defs;
pub use market::{
    clear_spatial_good_market, compute_per_100_demand, GoodCatalog, GoodDef, GoodId,
    HubCliqueRule, Inventory, LogisticsMatrix, MarketClearingRule, NodeMatrix, NodeNetworkSpec,
    NodeOrderBook, OrderBookSnapshot, PriceLevelSnapshot, PricingMode, RoleWageBook,
    TradeFillRecord, TransportChannel, CASH_SCALE, GOOD_ADMIN, GOOD_APPLE, GOOD_CASH,
    GOOD_SERVICE,
};
#[cfg(test)]
pub use market::default_good_defs;
pub use planet::{make_workshop, Appearance, Planet, PlanetTiles, Position2d, Tile};
pub(crate) use planet::{evaluate_planet_economy, PlanetSimEvaluation, SectorSimView};
pub use protocol::{
    BuildingSnapshot, ClientCommand, GameSnapshot, OccupiedTileSnapshot, PlacedBuildingSnapshot,
    PlanetSnapshot, PolicyChange, RegionSummarySnapshot, ResourceStockSnapshot, ScenarioPreset,
    StreamEvent, TimeSnapshot,
};
pub use rules::{
    AiWeightSpec, CmpOp, ContextVar, EventCatalog, EventDef, EventOptionDef, EventParty,
    IfElseWeightRule, PendingEvent, PendingEventPayload, RuleCond, RuleContext, RuleEffect,
    WeightDelta, EVENT_BILATERAL_CONTRACT_DUE, EVENT_COUPON_CLAIM_DUE, EVENT_FORCED_EXPROPRIATION,
    EVENT_SECTOR_INVEST_COMMERCIAL,
};
pub use runtime::GameRuntime;
pub use surface::{
    generate_planet_surface, PlanetSurfaceSnapshot, SurfacePolygonTile, SurfaceRegion, TileTag,
    TileTags,
};
pub use ai::{AiDecisionConfig, SiteScoringWeights};
pub use modifiers::StatModifier;

pub struct Game {
    pub(crate) world: World,
    pub(crate) scenario: ScenarioPreset,
    pub(crate) calendar: CalendarConfig,
    pub(crate) pop_catalog: PopTypeCatalog,
    pub(crate) goods: GoodCatalog,
    pub(crate) buildings: BuildingCatalog,
    pub(crate) transport_hubs: Vec<data_bundle::TransportHubDef>,
    pub(crate) events: EventCatalog,
    pub(crate) ai_config: AiDecisionConfig,
    pub(crate) controlled_actor: Option<CounterpartyId>,
    pub(crate) pending_events: Vec<PendingEvent>,
    pub(crate) next_event_instance_id: u64,
    pub(crate) elapsed_subticks: u64,
    pub(crate) speed: GameSpeed,
    pub(crate) counters: TickCounters,
}

impl Default for Game {
    fn default() -> Self {
        Self::with_calendar(CalendarConfig::default())
    }
}

impl Game {
    pub fn with_calendar(calendar: CalendarConfig) -> Self {
        Self::with_economy_v1(calendar).expect("加载外部 gamedata 数据包失败")
    }

    #[cfg(test)]
    pub fn with_legacy_apple_catalog() -> Self {
        Self::with_pop_catalog(
            CalendarConfig::default(),
            PopTypeCatalog::from_defs(default_pop_type_defs()).expect("测试 Pop 数据合法"),
        )
    }

    #[cfg(test)]
    pub fn with_pop_catalog(calendar: CalendarConfig, pop_catalog: PopTypeCatalog) -> Self {
        Self::with_catalogs(
            calendar,
            GoodCatalog::from_defs(default_good_defs()).expect("测试商品数据合法"),
            BuildingCatalog::from_defs(default_building_defs()).expect("测试建筑数据合法"),
            pop_catalog,
            EventCatalog::default(),
        )
    }

    pub fn with_economy_v1(calendar: CalendarConfig) -> Result<Self, String> {
        Self::with_earth_grid_cells(calendar, surface::DEFAULT_GRID_CELLS)
    }

    pub fn with_earth_grid_cells(
        calendar: CalendarConfig,
        earth_grid_cells: usize,
    ) -> Result<Self, String> {
        Self::with_scenario_and_cells(calendar, ScenarioPreset::Earth, earth_grid_cells)
    }

    pub fn with_scenario_preset(
        calendar: CalendarConfig,
        scenario: ScenarioPreset,
    ) -> Result<Self, String> {
        Self::with_scenario_and_cells(calendar, scenario, surface::DEFAULT_GRID_CELLS)
    }

    pub fn with_scenario_and_cells(
        calendar: CalendarConfig,
        scenario: ScenarioPreset,
        grid_cells: usize,
    ) -> Result<Self, String> {
        let full = data_bundle::load_full_gamedata()?;
        let mut g = Self::with_catalogs_and_hubs_scenario(
            calendar,
            scenario,
            full.economy.goods,
            full.economy.buildings,
            full.economy.pop_types,
            full.economy.transport_hubs,
            full.events,
            grid_cells,
        );
        g.ai_config = full.ai_config;
        Ok(g)
    }

    pub fn with_economy_bundle(
        calendar: CalendarConfig,
        bundle: data_bundle::EconomyBundle,
        events: EventCatalog,
    ) -> Self {
        Self::with_catalogs_and_hubs(
            calendar,
            bundle.goods,
            bundle.buildings,
            bundle.pop_types,
            bundle.transport_hubs,
            events,
        )
    }

    pub fn with_catalogs(
        calendar: CalendarConfig,
        goods: GoodCatalog,
        buildings: BuildingCatalog,
        pop_catalog: PopTypeCatalog,
        events: EventCatalog,
    ) -> Self {
        Self::with_catalogs_and_hubs(calendar, goods, buildings, pop_catalog, Vec::new(), events)
    }

    pub fn with_catalogs_and_hubs(
        calendar: CalendarConfig,
        goods: GoodCatalog,
        buildings: BuildingCatalog,
        pop_catalog: PopTypeCatalog,
        transport_hubs: Vec<data_bundle::TransportHubDef>,
        events: EventCatalog,
    ) -> Self {
        Self::with_catalogs_and_hubs_ext(
            calendar,
            goods,
            buildings,
            pop_catalog,
            transport_hubs,
            events,
            surface::DEFAULT_GRID_CELLS,
        )
    }

    pub fn with_catalogs_and_hubs_ext(
        calendar: CalendarConfig,
        goods: GoodCatalog,
        buildings: BuildingCatalog,
        pop_catalog: PopTypeCatalog,
        transport_hubs: Vec<data_bundle::TransportHubDef>,
        events: EventCatalog,
        earth_grid_cells: usize,
    ) -> Self {
        Self::with_catalogs_and_hubs_scenario(
            calendar,
            ScenarioPreset::Earth,
            goods,
            buildings,
            pop_catalog,
            transport_hubs,
            events,
            earth_grid_cells,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_catalogs_and_hubs_scenario(
        calendar: CalendarConfig,
        scenario: ScenarioPreset,
        goods: GoodCatalog,
        buildings: BuildingCatalog,
        pop_catalog: PopTypeCatalog,
        transport_hubs: Vec<data_bundle::TransportHubDef>,
        events: EventCatalog,
        grid_cells: usize,
    ) -> Self {
        let world =
            scenario::seed_scenario_world(scenario, &goods, &buildings, &pop_catalog, grid_cells);
        Self {
            world,
            scenario,
            calendar,
            pop_catalog,
            goods,
            buildings,
            transport_hubs,
            events,
            ai_config: AiDecisionConfig::default(),
            controlled_actor: None,
            pending_events: Vec::new(),
            next_event_instance_id: 1,
            elapsed_subticks: 0,
            speed: GameSpeed::Paused,
            counters: TickCounters::default(),
        }
    }

    #[inline]
    pub fn scenario(&self) -> ScenarioPreset {
        self.scenario
    }

    #[inline]
    pub fn goods(&self) -> &GoodCatalog {
        &self.goods
    }

    #[inline]
    pub fn goods_mut(&mut self) -> &mut GoodCatalog {
        &mut self.goods
    }

    #[inline]
    pub fn buildings(&self) -> &BuildingCatalog {
        &self.buildings
    }

    #[inline]
    pub fn buildings_mut(&mut self) -> &mut BuildingCatalog {
        &mut self.buildings
    }

    #[inline]
    pub fn transport_hubs(&self) -> &[data_bundle::TransportHubDef] {
        &self.transport_hubs
    }

    #[inline]
    pub fn events(&self) -> &EventCatalog {
        &self.events
    }

    #[inline]
    pub fn events_mut(&mut self) -> &mut EventCatalog {
        &mut self.events
    }

    #[inline]
    pub fn ai_config(&self) -> &AiDecisionConfig {
        &self.ai_config
    }

    #[inline]
    pub fn pop_catalog(&self) -> &PopTypeCatalog {
        &self.pop_catalog
    }

    pub fn speed(&self) -> GameSpeed {
        self.speed
    }

    pub fn set_speed(&mut self, speed: GameSpeed) {
        self.speed = speed;
    }

    pub fn counters(&self) -> TickCounters {
        self.counters
    }

    pub fn active_combat_count(&self) -> usize {
        self.world.query::<&InCombat>().iter().count()
    }

    pub fn has_active_combat(&self) -> bool {
        self.world.query::<&InCombat>().iter().next().is_some()
    }

    pub fn next_step_interval(&self) -> Option<Duration> {
        let day_interval = self.speed.day_interval()?;
        if self.speed != GameSpeed::Speed4 && self.has_active_combat() {
            Some(day_interval / SUBTICKS_PER_DAY as u32)
        } else {
            let rem = self.remaining_subticks_in_day() as u32;
            Some((day_interval * rem) / SUBTICKS_PER_DAY as u32)
        }
    }

    pub fn step_once(&mut self) -> StepOutcome {
        if !self.has_active_combat() {
            let rem = self.remaining_subticks_in_day();
            self.elapsed_subticks += rem;
            return self.run_day_boundary_systems();
        }

        if self.speed == GameSpeed::Speed4 {
            let rem = self.remaining_subticks_in_day();
            for _ in 0..rem {
                self.elapsed_subticks += 1;
                self.run_combat_subtick_systems();
            }
            self.run_day_boundary_systems()
        } else {
            self.elapsed_subticks += 1;
            self.run_combat_subtick_systems();
            if self.elapsed_subticks % SUBTICKS_PER_DAY == 0 {
                self.run_day_boundary_systems()
            } else {
                StepOutcome::default()
            }
        }
    }

    fn remaining_subticks_in_day(&self) -> u64 {
        let subtick = self.elapsed_subticks % SUBTICKS_PER_DAY;
        SUBTICKS_PER_DAY - subtick
    }

    fn run_combat_subtick_systems(&mut self) {
        self.counters.combat_subticks += 1;
        for (_, combat) in self.world.query_mut::<&mut InCombat>() {
            combat.subticks_fought += 1;
        }
    }

    fn run_day_boundary_systems(&mut self) -> StepOutcome {
        debug_assert_eq!(self.elapsed_subticks % SUBTICKS_PER_DAY, 0);
        let elapsed_days = self.elapsed_subticks / SUBTICKS_PER_DAY;
        self.counters.daily_ticks += 1;
        let month_advanced = elapsed_days % DAYS_PER_MONTH == 0;
        let year_advanced = elapsed_days % DAYS_PER_YEAR == 0;

        if month_advanced {
            self.counters.monthly_ticks += 1;
            self.run_monthly_political_economy();
        }
        if year_advanced {
            self.counters.yearly_ticks += 1;
        }

        StepOutcome {
            day_advanced: true,
            month_advanced,
            year_advanced,
        }
    }
}
