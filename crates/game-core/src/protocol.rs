//! Client-Server Wire Protocol & Snapshot DTOs (`前后端通信协议与快照契约层`).

use crate::building::{BuildingTags, JobPriority, SlotRule, TileSlotKey, SECONDARY_SLOT_COUNT};
use crate::calendar::GameSpeed;
use crate::economy::{
    BuildingOwnerSnapshot, DepotPolicy, EnterpriseSnapshot, GoodInfoSnapshot, MonetaryPolicy,
    PolitySnapshot, PopTypeSnapshot, SectorSnapshot,
};
use crate::planet::{Appearance, Position2d};
use crate::surface::{PlanetSurfaceSnapshot, TileTags};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioPreset {
    #[default]
    Earth,
    Colony,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimeSnapshot {
    pub scenario: ScenarioPreset,
    pub epoch_name: String,
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub subtick: u8,
    pub formatted: String,
    pub elapsed_days: u64,
    pub speed: GameSpeed,
    pub active_combats: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceStockSnapshot {
    pub id: String,
    pub name: String,
    pub amount: i64,
    pub monthly_delta: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BuildingSnapshot {
    pub id: String,
    pub name: String,
    pub slot_rule: SlotRule,
    pub job_priority: JobPriority,
    pub build_cost: i64,
    pub hosting_capacity: u16,
    pub jobs_required: u32,
    pub role_jobs: BTreeMap<String, u32>,
    pub tags: BuildingTags,
    pub required_tile_tags: TileTags,
    pub forbidden_tile_tags: TileTags,
    pub required_primary_tags: BuildingTags,
    pub unique_planet_tags: BuildingTags,
    pub unique_region_tags: BuildingTags,
    pub unique_tile_tags: BuildingTags,
    pub monthly_inputs: BTreeMap<String, i64>,
    pub monthly_output: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlacedBuildingSnapshot {
    pub id: String,
    pub name: String,
    pub slot_rule: SlotRule,
    pub scale_multiplier: u8,
    pub job_priority: JobPriority,
    pub tags: BuildingTags,
    pub jobs_required: u32,
    pub jobs_filled: u32,
    pub hosting_capacity: u16,
    pub owner: BuildingOwnerSnapshot,
    pub monthly_inputs: BTreeMap<String, i64>,
    pub base_monthly_output: BTreeMap<String, i64>,
    pub effective_monthly_output: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OccupiedTileSnapshot {
    pub index: usize,
    pub region_id: u16,
    pub sector_id: Option<u16>,
    pub tags: TileTags,
    pub habitability: u16,
    pub primary: Option<PlacedBuildingSnapshot>,
    pub secondary: [Option<PlacedBuildingSnapshot>; SECONDARY_SLOT_COUNT],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RegionSummarySnapshot {
    pub region_id: u16,
    pub sector_id: Option<u16>,
    pub claimed_land_tiles: u16,
    pub population: u32,
    pub monthly_growth: u32,
    pub jobs_required: u32,
    pub jobs_filled: u32,
    pub free_labor: u32,
    pub primary_used: u16,
    pub primary_total: u16,
    pub land_primary_used: u16,
    pub land_primary_total: u16,
    pub secondary_used: u16,
    pub secondary_total: u16,
    pub unlocked_secondary_empty: u16,
    pub core_tile_index: Option<usize>,
    pub core_building_name: Option<String>,
    pub building_counts: BTreeMap<String, u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanetSnapshot {
    pub scenario: ScenarioPreset,
    pub id: String,
    pub name: String,
    pub radius_km: f64,
    pub position: Position2d,
    pub appearance: Appearance,
    pub polity: PolitySnapshot,
    pub primary_staple_id: String,
    pub primary_staple_name: String,
    pub good_catalog: Vec<GoodInfoSnapshot>,
    pub pop_catalog: Vec<PopTypeSnapshot>,
    pub stockpile: BTreeMap<String, i64>,
    pub resources: Vec<ResourceStockSnapshot>,
    pub building_catalog: Vec<BuildingSnapshot>,
    pub tile_count: usize,
    pub land_tile_count: usize,
    pub sea_tile_count: usize,
    pub region_count: usize,
    pub starter_tile_index: usize,
    pub capital_tile_index: Option<usize>,
    pub total_population: u32,
    pub monthly_population_growth: u32,
    pub total_jobs_required: u32,
    pub total_jobs_filled: u32,
    pub total_building_count: usize,
    pub sectors: Vec<SectorSnapshot>,
    pub enterprises: Vec<EnterpriseSnapshot>,
    pub region_summaries: Vec<RegionSummarySnapshot>,
    pub tile_sectors: Vec<Option<u16>>,
    pub occupied_tiles: Vec<OccupiedTileSnapshot>,
}

#[derive(Clone, Serialize)]
pub struct GameSnapshot {
    pub time: TimeSnapshot,
    pub polities: Vec<PolitySnapshot>,
    pub planets: Vec<PlanetSnapshot>,
    pub pending_events: Vec<crate::rules::PendingEvent>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    Init {
        time: TimeSnapshot,
        default_planet_id: Option<String>,
    },
    Tick {
        time: TimeSnapshot,
    },
    PlanetSurface {
        surface: Option<PlanetSurfaceSnapshot>,
    },
    PlanetView {
        planet: Option<PlanetSnapshot>,
    },
    CommandNotice {
        message: String,
    },
    CommandError {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ClientCommand {
    SetSpeed {
        speed: GameSpeed,
    },
    WatchPlanet {
        planet_id: Option<String>,
    },
    BuildOnTile {
        planet_id: String,
        tile_index: usize,
        building_id: String,
        #[serde(default)]
        slot: Option<TileSlotKey>,
    },
    BuildInRegion {
        planet_id: String,
        region_id: u16,
        building_id: String,
    },
    DemolishOnTile {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    },
    PrivatizeSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    },
    NationalizeSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    },
    AcquireBankruptSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        #[serde(default)]
        buyer_polity: Option<crate::economy::PolityId>,
    },
    ResolvePendingEvent {
        instance_id: u64,
        option_index: usize,
    },
    TogglePrivateLandUse {
        planet_id: String,
        sector_id: u16,
        allow: bool,
    },
    SetMonetaryPolicy {
        planet_id: String,
        policy: MonetaryPolicy,
    },
    SetDepotPolicy {
        planet_id: String,
        sector_id: u16,
        policy: DepotPolicy,
    },
    SetEarthGridPreset {
        dense: bool,
    },
    SetScenarioPreset {
        scenario: ScenarioPreset,
    },
    ClaimTile {
        planet_id: String,
        tile_index: usize,
    },
    ClaimRegionFrontier {
        planet_id: String,
        region_id: u16,
        #[serde(default = "default_frontier_batch")]
        max_count: usize,
    },
    ReloadData {
        #[serde(default)]
        restart_world: bool,
    },
}

const fn default_frontier_batch() -> usize {
    5
}

/// Macro policy changes routed through one runtime command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyChange {
    Monetary(MonetaryPolicy),
    Depot { sector_id: u16, policy: DepotPolicy },
}
