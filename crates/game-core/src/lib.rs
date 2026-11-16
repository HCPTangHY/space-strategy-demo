//! Pure game state and simulation clock: no Tauri, filesystem, rendering, or network dependencies.
pub mod economy;
pub mod market;
mod settlement;
pub mod surface;

use hecs::World;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub use economy::{
    default_pop_type_defs, BuildingOwner, BuildingOwnerSnapshot, DepotPolicy, Enterprise,
    EnterpriseId, EnterpriseSnapshot, MonetaryPolicy, Polity, PolityId, PolitySnapshot, PopGroup,
    PopGroupSnapshot, PopId, PopTypeCatalog, PopTypeDef, PopTypeId, PopTypeSnapshot,
    RoleWageSnapshot, Sector, SectorCapital, SectorId, SectorMarket, SectorPops, SectorSnapshot,
    SectorTradeLink, StaffContract, TrustId, Workshop, CAMPUS_HOSTING_CAPACITY,
    CBD_HOSTING_CAPACITY, ENTERPRISE_FOUNDING_MIN_PRIVATE, ENTERPRISE_OUTPUT_BONUS_PERMILLE,
    POP_ACADEMIC, POP_BUREAUCRAT, POP_CAPITALIST, POP_ENGINEER, POP_LABORER, POP_SOLDIER,
    POP_SUBSISTENCE, POP_VAGRANT,
};
pub use market::{
    default_good_defs, GoodCatalog, GoodDef, GoodId, Inventory, LogisticsMatrix,
    OrderBookSnapshot, PriceLevelSnapshot, RoleWageBook, TradeFillRecord, TransportChannel,
    CASH_SCALE, GOOD_CASH,
};
use market::cents_to_credits_round;
pub use surface::{
    generate_planet_surface, PlanetSurfaceSnapshot, SurfacePolygonTile, SurfaceRegion, TileTag,
    TileTags,
};

pub const SUBTICKS_PER_DAY: u64 = 10;
pub const DAYS_PER_MONTH: u64 = 30;
pub const MONTHS_PER_YEAR: u64 = 12;
pub const DAYS_PER_YEAR: u64 = DAYS_PER_MONTH * MONTHS_PER_YEAR; // 360

pub const SECONDARY_SLOT_COUNT: usize = 3;
pub const SLOTS_PER_TILE: usize = 1 + SECONDARY_SLOT_COUNT; // 1 primary + 3 secondary

pub use market::GOOD_APPLE;
pub const BUILDING_APPLE_ORCHARD: &str = "base:apple_orchard";
pub const BUILDING_REGIONAL_CORE: &str = "base:regional_core";
pub const BUILDING_PLANETARY_CAPITAL: &str = "base:planetary_capital";
pub const BUILDING_COMMERCIAL_STREET: &str = "base:commercial_street";
pub const BUILDING_CORPORATE_CAMPUS: &str = "base:corporate_campus";

/// Scenario calendar definition. Internal simulation only stores elapsed subticks from zero;
/// display dates are derived from this configuration and skip year 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarConfig {
    pub epoch_name: String,
    pub start_year: i32,
}

impl CalendarConfig {
    pub fn new(epoch_name: impl Into<String>, start_year: i32) -> Result<Self, &'static str> {
        if start_year == 0 {
            return Err("历法起始年份不能为 0（无公元 0 年）。");
        }
        Ok(Self {
            epoch_name: epoch_name.into(),
            start_year,
        })
    }
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            epoch_name: "星历".into(),
            start_year: 1,
        }
    }
}

/// Speed tiers: Pause (0) + 1x..4x.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum GameSpeed {
    Paused = 0,
    Speed1 = 1,
    Speed2 = 2,
    Speed3 = 3,
    Speed4 = 4,
}

impl GameSpeed {
    pub fn day_interval(self) -> Option<Duration> {
        match self {
            Self::Paused => None,
            Self::Speed1 => Some(Duration::from_millis(2000)),
            Self::Speed2 => Some(Duration::from_millis(1000)),
            Self::Speed3 => Some(Duration::from_millis(250)),
            // ponytail: 60 days/sec ceiling prevents empty worlds from skipping millennia in 1s;
            // once late-game daily simulation exceeds 16.67ms, it degrades naturally to zero-delay.
            Self::Speed4 => Some(Duration::from_nanos(16_666_667)),
        }
    }
}

impl From<GameSpeed> for u8 {
    fn from(speed: GameSpeed) -> Self {
        speed as u8
    }
}

impl TryFrom<u8> for GameSpeed {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Paused),
            1 => Ok(Self::Speed1),
            2 => Ok(Self::Speed2),
            3 => Ok(Self::Speed3),
            4 => Ok(Self::Speed4),
            other => Err(format!("无效的速度档位：{other}（仅支持 0..=4）")),
        }
    }
}

pub struct Planet {
    pub id: String,
    pub name: String,
    pub radius_km: f64,
    pub ruling_polity: PolityId,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Position2d {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Appearance {
    pub ocean_rgb: [u8; 3],
    pub land_rgb: [u8; 3],
    pub seed: u32,
}

/// Compact 2-byte building catalog handle (`Option<BuildingTypeId>` is still 2 bytes via `NonZeroU16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BuildingTypeId(pub NonZeroU16);

/// Compact building category/capability tags (`u32` bitset; serializes as string array).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BuildingTag {
    PlanetaryCapital = 0,
    RegionalCore = 1,
    Agriculture = 2,
    Commercial = 3,
    CorporateCampus = 4,
}

impl BuildingTag {
    pub const ALL: [Self; 5] = [
        Self::PlanetaryCapital,
        Self::RegionalCore,
        Self::Agriculture,
        Self::Commercial,
        Self::CorporateCampus,
    ];

    #[inline]
    pub const fn bit(self) -> u32 {
        1u32 << (self as u8)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanetaryCapital => "planetary_capital",
            Self::RegionalCore => "regional_core",
            Self::Agriculture => "agriculture",
            Self::Commercial => "commercial",
            Self::CorporateCampus => "corporate_campus",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::PlanetaryCapital => "行星首府",
            Self::RegionalCore => "大区核心",
            Self::Agriculture => "农业设施",
            Self::Commercial => "商业街",
            Self::CorporateCampus => "企业园区",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BuildingTags(pub u32);

impl BuildingTags {
    pub const EMPTY: Self = Self(0);

    #[inline]
    pub const fn with(self, tag: BuildingTag) -> Self {
        Self(self.0 | tag.bit())
    }

    #[inline]
    pub const fn has(self, tag: BuildingTag) -> bool {
        (self.0 & tag.bit()) != 0
    }

    #[inline]
    pub const fn intersects(self, mask: Self) -> bool {
        (self.0 & mask.0) != 0
    }

    pub fn first_overlap(self, mask: Self) -> Option<BuildingTag> {
        BuildingTag::ALL
            .into_iter()
            .find(|&t| self.has(t) && mask.has(t))
    }

    pub fn iter(self) -> impl Iterator<Item = BuildingTag> {
        BuildingTag::ALL.into_iter().filter(move |&t| self.has(t))
    }
}

impl Serialize for BuildingTags {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let list: Vec<&'static str> = self.iter().map(BuildingTag::as_str).collect();
        list.serialize(serializer)
    }
}

/// Defines which slot class a building may occupy:
/// - `PrimaryOnly`: dominant identity hubs (`行星首府`, `大区核心`, `企业园区`)
/// - `SecondaryOnly`: urban/industrial attachments (`商业街`)
/// - `Any`: productive industries (`苹果园`) — runs at `2x` dominant scale in `Primary`, `1x` in `Secondary`!
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotRule {
    PrimaryOnly,
    SecondaryOnly,
    Any,
}

/// Identifies one of the 4 fixed slots on a tile (`1 主 + 3 次`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileSlotKey {
    Primary,
    Secondary0,
    Secondary1,
    Secondary2,
}

impl TileSlotKey {
    pub const ALL: [Self; SLOTS_PER_TILE] = [
        Self::Primary,
        Self::Secondary0,
        Self::Secondary1,
        Self::Secondary2,
    ];

    pub const fn is_primary(self) -> bool {
        matches!(self, Self::Primary)
    }

    pub const fn as_index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary0 => 1,
            Self::Secondary1 => 2,
            Self::Secondary2 => 3,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Primary => "主建筑槽",
            Self::Secondary0 => "次建筑槽 1",
            Self::Secondary1 => "次建筑槽 2",
            Self::Secondary2 => "次建筑槽 3",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum JobPriority {
    Core = 0,
    Commercial = 1,
    Standard = 2,
}

/// Pure data-referenced building definition (`jobs_by_role` references string Pop type IDs!).
#[derive(Clone, Debug)]
pub struct BuildingDef {
    pub type_id: BuildingTypeId,
    pub id: &'static str,
    pub name: &'static str,
    pub slot_rule: SlotRule,
    pub job_priority: JobPriority,
    pub build_cost: i64,
    pub hosting_capacity: u16,
    pub jobs_by_role: &'static [(&'static str, u32)],
    pub tags: BuildingTags,
    pub unique_planet_tags: BuildingTags,
    pub unique_region_tags: BuildingTags,
    pub unique_tile_tags: BuildingTags,
    pub required_tags: TileTags,
    pub forbidden_tags: TileTags,
    pub monthly_output: &'static [(&'static str, i64)],
}

const BUILTIN_BUILDINGS: [BuildingDef; 5] = [
    BuildingDef {
        type_id: BuildingTypeId(NonZeroU16::new(1).unwrap()),
        id: BUILDING_APPLE_ORCHARD,
        name: "苹果园",
        slot_rule: SlotRule::Any,
        job_priority: JobPriority::Standard,
        build_cost: 150,
        hosting_capacity: 0,
        jobs_by_role: &[(POP_LABORER, 100)],
        tags: BuildingTags::EMPTY.with(BuildingTag::Agriculture),
        unique_planet_tags: BuildingTags::EMPTY,
        unique_region_tags: BuildingTags::EMPTY,
        unique_tile_tags: BuildingTags::EMPTY,
        required_tags: TileTags::EMPTY,
        forbidden_tags: TileTags::EMPTY,
        monthly_output: &[(GOOD_APPLE, 20)],
    },
    BuildingDef {
        type_id: BuildingTypeId(NonZeroU16::new(2).unwrap()),
        id: BUILDING_REGIONAL_CORE,
        name: "大区核心",
        slot_rule: SlotRule::PrimaryOnly,
        job_priority: JobPriority::Core,
        build_cost: 400,
        hosting_capacity: 0,
        jobs_by_role: &[
            (POP_BUREAUCRAT, 70),
            (POP_ENGINEER, 30),
            (POP_SOLDIER, 30),
            (POP_LABORER, 20),
        ],
        tags: BuildingTags::EMPTY.with(BuildingTag::RegionalCore),
        unique_planet_tags: BuildingTags::EMPTY,
        unique_region_tags: BuildingTags::EMPTY.with(BuildingTag::RegionalCore),
        unique_tile_tags: BuildingTags::EMPTY,
        required_tags: TileTags::EMPTY.with(TileTag::Land),
        forbidden_tags: TileTags::EMPTY,
        monthly_output: &[],
    },
    BuildingDef {
        type_id: BuildingTypeId(NonZeroU16::new(3).unwrap()),
        id: BUILDING_PLANETARY_CAPITAL,
        name: "行星首府",
        slot_rule: SlotRule::PrimaryOnly,
        job_priority: JobPriority::Core,
        build_cost: 800,
        hosting_capacity: 0,
        jobs_by_role: &[
            (POP_BUREAUCRAT, 80),
            (POP_ACADEMIC, 40),
            (POP_ENGINEER, 40),
            (POP_SOLDIER, 40),
        ],
        tags: BuildingTags::EMPTY
            .with(BuildingTag::PlanetaryCapital)
            .with(BuildingTag::RegionalCore),
        unique_planet_tags: BuildingTags::EMPTY.with(BuildingTag::PlanetaryCapital),
        unique_region_tags: BuildingTags::EMPTY.with(BuildingTag::RegionalCore),
        unique_tile_tags: BuildingTags::EMPTY,
        required_tags: TileTags::EMPTY.with(TileTag::Land),
        forbidden_tags: TileTags::EMPTY,
        monthly_output: &[],
    },
    BuildingDef {
        type_id: BuildingTypeId(NonZeroU16::new(4).unwrap()),
        id: BUILDING_COMMERCIAL_STREET,
        name: "商业街",
        slot_rule: SlotRule::SecondaryOnly,
        job_priority: JobPriority::Commercial,
        build_cost: 200,
        hosting_capacity: CBD_HOSTING_CAPACITY,
        jobs_by_role: &[(POP_CAPITALIST, 15), (POP_BUREAUCRAT, 25)],
        tags: BuildingTags::EMPTY.with(BuildingTag::Commercial),
        unique_planet_tags: BuildingTags::EMPTY,
        unique_region_tags: BuildingTags::EMPTY,
        unique_tile_tags: BuildingTags::EMPTY,
        required_tags: TileTags::EMPTY.with(TileTag::Land),
        forbidden_tags: TileTags::EMPTY,
        monthly_output: &[],
    },
    BuildingDef {
        type_id: BuildingTypeId(NonZeroU16::new(5).unwrap()),
        id: BUILDING_CORPORATE_CAMPUS,
        name: "企业园区",
        slot_rule: SlotRule::PrimaryOnly,
        job_priority: JobPriority::Commercial,
        build_cost: 350,
        hosting_capacity: CAMPUS_HOSTING_CAPACITY,
        jobs_by_role: &[
            (POP_CAPITALIST, 30),
            (POP_ENGINEER, 40),
            (POP_BUREAUCRAT, 30),
        ],
        tags: BuildingTags::EMPTY.with(BuildingTag::CorporateCampus),
        unique_planet_tags: BuildingTags::EMPTY,
        unique_region_tags: BuildingTags::EMPTY,
        unique_tile_tags: BuildingTags::EMPTY,
        required_tags: TileTags::EMPTY.with(TileTag::Land),
        forbidden_tags: TileTags::EMPTY,
        monthly_output: &[],
    },
];

pub fn scale_output_by_staffing(full_amount: i64, employed: u32, required: u32) -> i64 {
    if required == 0 || employed >= required {
        return full_amount;
    }
    if employed == 0 {
        return 0;
    }
    let num = i128::from(full_amount) * i128::from(employed);
    let den = i128::from(required);
    let rounded = if num >= 0 {
        (num + den / 2) / den
    } else {
        (num - den / 2) / den
    };
    rounded as i64
}

impl BuildingDef {
    pub fn by_id(id: &str) -> Result<&'static Self, String> {
        BUILTIN_BUILDINGS
            .iter()
            .find(|b| b.id == id)
            .ok_or_else(|| format!("未知的建筑类型：{id}"))
    }

    pub fn by_type_id(type_id: BuildingTypeId) -> &'static Self {
        let idx = (type_id.0.get() as usize).saturating_sub(1);
        &BUILTIN_BUILDINGS[idx]
    }

    #[inline]
    pub const fn scale_for_slot(&self, slot: TileSlotKey) -> u32 {
        match (self.slot_rule, slot.is_primary()) {
            (SlotRule::Any, true) => 2,
            _ => 1,
        }
    }

    pub fn base_jobs_required(&self) -> u32 {
        self.jobs_by_role.iter().map(|&(_, n)| n).sum()
    }

    pub fn slot_jobs_required(&self, slot: TileSlotKey) -> u32 {
        self.base_jobs_required() * self.scale_for_slot(slot)
    }

    pub fn slot_build_cost(&self, slot: TileSlotKey) -> i64 {
        self.build_cost * i64::from(self.scale_for_slot(slot))
    }

    /// Full-staffing monthly wage bill in cents for `slot` under `catalog`.
    pub fn slot_monthly_wage_bill_cents(
        &self,
        catalog: &PopTypeCatalog,
        slot: TileSlotKey,
    ) -> i64 {
        let scale = i64::from(self.scale_for_slot(slot));
        let mut total = 0i64;
        for &(role_str, base_req) in self.jobs_by_role {
            let pid = self.resolve_role_id(catalog, role_str);
            let wage_100 = catalog.get(pid).base_wage_per_100;
            if wage_100 > 0 {
                total += (i64::from(base_req) * scale * wage_100 * CASH_SCALE) / 100;
            }
        }
        total
    }

    pub fn is_state_core_or_public(&self) -> bool {
        self.tags.has(BuildingTag::PlanetaryCapital) || self.tags.has(BuildingTag::RegionalCore)
    }

    pub fn is_privatizable_industry(&self) -> bool {
        self.slot_rule == SlotRule::Any && !self.monthly_output.is_empty()
    }

    pub fn can_use_slot(&self, slot: TileSlotKey) -> bool {
        match self.slot_rule {
            SlotRule::PrimaryOnly => slot.is_primary(),
            SlotRule::SecondaryOnly => !slot.is_primary(),
            SlotRule::Any => true,
        }
    }

    pub fn matches_tags(&self, tags: TileTags) -> bool {
        tags.contains_all(self.required_tags) && !tags.intersects(self.forbidden_tags)
    }

    pub fn resolve_role_id(&self, catalog: &PopTypeCatalog, role_str: &str) -> PopTypeId {
        if let Some(pid) = catalog.id_of(role_str) {
            return pid;
        }
        if role_str == POP_CAPITALIST {
            if let Some(&div_pid) = catalog.dividend_recipients().first() {
                return div_pid;
            }
        }
        catalog.default_pool_id()
    }

    pub fn to_snapshot(&self) -> BuildingSnapshot {
        let mut monthly_output = BTreeMap::new();
        for &(good, delta) in self.monthly_output {
            monthly_output.insert(good.to_string(), delta);
        }
        let mut role_jobs = BTreeMap::new();
        for &(role_id, count) in self.jobs_by_role {
            role_jobs.insert(role_id.to_string(), count);
        }
        BuildingSnapshot {
            id: self.id.to_string(),
            name: self.name.to_string(),
            slot_rule: self.slot_rule,
            job_priority: self.job_priority,
            build_cost: self.build_cost,
            hosting_capacity: self.hosting_capacity,
            jobs_required: self.base_jobs_required(),
            role_jobs,
            tags: self.tags,
            required_tile_tags: self.required_tags,
            monthly_output,
        }
    }

    pub fn to_placed_snapshot(
        &self,
        slot: TileSlotKey,
        jobs_filled: u32,
        bonus_permille: i64,
        owner: BuildingOwnerSnapshot,
    ) -> PlacedBuildingSnapshot {
        let scale = self.scale_for_slot(slot);
        let jobs_required = self.slot_jobs_required(slot);
        let mut base_monthly_output = BTreeMap::new();
        let mut effective_monthly_output = BTreeMap::new();
        for &(good, unit_delta) in self.monthly_output {
            let scaled_base = unit_delta * i64::from(scale);
            let with_bonus = (scaled_base * (1000 + bonus_permille) + 500) / 1000;
            base_monthly_output.insert(good.to_string(), with_bonus);
            effective_monthly_output.insert(
                good.to_string(),
                scale_output_by_staffing(with_bonus, jobs_filled, jobs_required),
            );
        }
        PlacedBuildingSnapshot {
            id: self.id.to_string(),
            name: self.name.to_string(),
            slot_rule: self.slot_rule,
            scale_multiplier: scale as u8,
            job_priority: self.job_priority,
            tags: self.tags,
            jobs_required,
            jobs_filled,
            hosting_capacity: self.hosting_capacity,
            owner,
            base_monthly_output,
            effective_monthly_output,
        }
    }
}

pub fn good_display_name(good_id: &str) -> &str {
    match good_id {
        GOOD_APPLE => "苹果",
        other => other,
    }
}

/// Fixed-length 8-byte building slots stored inline on every tile (`1 主 + 3 次`, zero heap allocation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileSlots {
    pub primary: Option<BuildingTypeId>,
    pub secondary: [Option<BuildingTypeId>; SECONDARY_SLOT_COUNT],
}

impl TileSlots {
    pub fn get(&self, key: TileSlotKey) -> Option<BuildingTypeId> {
        match key {
            TileSlotKey::Primary => self.primary,
            TileSlotKey::Secondary0 => self.secondary[0],
            TileSlotKey::Secondary1 => self.secondary[1],
            TileSlotKey::Secondary2 => self.secondary[2],
        }
    }

    pub fn set(&mut self, key: TileSlotKey, val: Option<BuildingTypeId>) {
        match key {
            TileSlotKey::Primary => self.primary = val,
            TileSlotKey::Secondary0 => self.secondary[0] = val,
            TileSlotKey::Secondary1 => self.secondary[1] = val,
            TileSlotKey::Secondary2 => self.secondary[2] = val,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.primary.is_none() && self.secondary.iter().all(Option::is_none)
    }

    pub fn has_any_secondary(&self) -> bool {
        self.secondary.iter().any(Option::is_some)
    }

    pub fn used_count(&self) -> usize {
        TileSlotKey::ALL
            .into_iter()
            .filter(|&k| self.get(k).is_some())
            .count()
    }

    pub fn first_compatible_empty_slot(&self, def: &BuildingDef) -> Option<TileSlotKey> {
        let primary_ready = self.primary.is_some();
        TileSlotKey::ALL.into_iter().find(|&k| {
            self.get(k).is_none() && def.can_use_slot(k) && (k.is_primary() || primary_ready)
        })
    }

    pub fn iter_occupied(&self) -> impl Iterator<Item = (TileSlotKey, BuildingTypeId)> + '_ {
        TileSlotKey::ALL
            .into_iter()
            .filter_map(move |k| self.get(k).map(|tid| (k, tid)))
    }
}

/// Compact planetary Voronoi tile state in ECS (`24 bytes` total per tile!).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub index: u16,
    pub region_id: u16,
    pub sector_id: u16,
    pub elevation: u8,
    pub tags: TileTags,
    pub slots: TileSlots,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanetTiles {
    pub regions: Vec<SurfaceRegion>,
    pub land_tile_count: usize,
    pub sea_tile_count: usize,
    pub starter_tile_index: usize,
    pub tiles: Vec<Tile>,
    pub slot_owners: Vec<[BuildingOwner; SLOTS_PER_TILE]>,
    /// Persistent locked labor contracts per building slot (`(pop_id, count, monthly_bill_cents)`).
    pub slot_contracts: Vec<[Vec<StaffContract>; SLOTS_PER_TILE]>,
    /// Independent per-slot economic agents: only productive buildings own a `Workshop`
    /// (its own capacity-bounded `Inventory` holding stock + `base:cash`).
    pub workshops: Vec<[Option<Workshop>; SLOTS_PER_TILE]>,
    pub tile_neighbors: Vec<Vec<u16>>,
    pub trade_links: Vec<SectorTradeLink>,
    pub sector_coastal: Vec<bool>,
    /// Precomputed `S × S` freight & monthly capacity matrix (overland relay + coastal shipping lanes).
    pub logistics: LogisticsMatrix,
    pub enterprises: Vec<Enterprise>,
}

/// Creates the per-slot `Workshop` for productive buildings (`None` for hubs / commercial / campus),
/// seeded with 1 month of full-staffing working capital (`base:cash`) carved out of construction expenditure.
pub fn make_workshop(
    goods: &GoodCatalog,
    pop_catalog: &PopTypeCatalog,
    def: &BuildingDef,
    slot: TileSlotKey,
) -> Option<Workshop> {
    def.is_privatizable_industry().then(|| {
        let seed_cash = def.slot_monthly_wage_bill_cents(pop_catalog, slot);
        Workshop::new(goods, def.scale_for_slot(slot), seed_cash, 0)
    })
}

impl PlanetTiles {
    pub fn from_planet_seed(
        planet_id: &str,
        seed: u32,
        ruling_polity: PolityId,
        goods: &GoodCatalog,
    ) -> Self {
        let generated = surface::generate_sim_surface(planet_id, seed);
        let starter_tile_index = generated.starter_tile_index;

        let mut region_to_sector = vec![u16::MAX; generated.regions.len()];
        let mut next_sector: u16 = 0;
        for reg in &generated.regions {
            if !reg.is_ocean && reg.land_tile_count > 0 {
                region_to_sector[usize::from(reg.id)] = next_sector;
                next_sector += 1;
            }
        }

        let mut tiles: Vec<Tile> = generated
            .tiles
            .into_iter()
            .enumerate()
            .map(|(index, geo)| Tile {
                index: index as u16,
                region_id: geo.region_id,
                sector_id: region_to_sector
                    .get(usize::from(geo.region_id))
                    .copied()
                    .unwrap_or(u16::MAX),
                elevation: geo.elevation,
                tags: geo.tags,
                slots: TileSlots::default(),
            })
            .collect();

        let mut slot_owners = vec![[BuildingOwner::Unowned; SLOTS_PER_TILE]; tiles.len()];
        let slot_contracts: Vec<[Vec<StaffContract>; SLOTS_PER_TILE]> = (0..tiles.len())
            .map(|_| std::array::from_fn(|_| Vec::new()))
            .collect();
        let mut workshops: Vec<[Option<Workshop>; SLOTS_PER_TILE]> =
            (0..tiles.len()).map(|_| [None, None, None, None]).collect();

        let starter_sector_u16 = tiles
            .get(starter_tile_index)
            .map(|t| t.sector_id)
            .unwrap_or(0);
        let starter_sector = SectorId(starter_sector_u16);

        let cap_def =
            BuildingDef::by_id(BUILDING_PLANETARY_CAPITAL).expect("内置行星首府定义必须有效");
        let orchard_def =
            BuildingDef::by_id(BUILDING_APPLE_ORCHARD).expect("内置苹果园定义必须有效");
        let cbd_def =
            BuildingDef::by_id(BUILDING_COMMERCIAL_STREET).expect("内置商业街定义必须有效");

        if let Some(starter) = tiles.get_mut(starter_tile_index) {
            starter.slots.primary = Some(cap_def.type_id);
            starter.slots.secondary[0] = Some(orchard_def.type_id);
            starter.slots.secondary[1] = Some(cbd_def.type_id);
            slot_owners[starter_tile_index][TileSlotKey::Primary.as_index()] =
                BuildingOwner::State {
                    polity_id: ruling_polity,
                };
            slot_owners[starter_tile_index][TileSlotKey::Secondary0.as_index()] =
                BuildingOwner::State {
                    polity_id: ruling_polity,
                };
            slot_owners[starter_tile_index][TileSlotKey::Secondary1.as_index()] =
                BuildingOwner::LocalPrivate {
                    home_sector: starter_sector,
                };
            workshops[starter_tile_index][TileSlotKey::Secondary0.as_index()] =
                Some(Workshop::new(goods, 1, 60 * CASH_SCALE, 10));
        }

        let private_farm_idx = tiles
            .iter()
            .filter(|t| {
                usize::from(t.index) != starter_tile_index
                    && t.sector_id == starter_sector_u16
                    && t.tags.has(TileTag::Land)
            })
            .max_by_key(|t| (t.tags.habitability_permille(), std::cmp::Reverse(t.index)))
            .map(|t| usize::from(t.index));

        if let Some(farm_idx) = private_farm_idx {
            tiles[farm_idx].slots.primary = Some(orchard_def.type_id);
            slot_owners[farm_idx][TileSlotKey::Primary.as_index()] = BuildingOwner::LocalPrivate {
                home_sector: starter_sector,
            };
            workshops[farm_idx][TileSlotKey::Primary.as_index()] =
                Some(Workshop::new(goods, 2, 120 * CASH_SCALE, 20));
        }

        let sector_count = usize::from(next_sector);
        let tile_sectors: Vec<u16> = tiles.iter().map(|t| t.sector_id).collect();
        let tile_tags: Vec<TileTags> = tiles.iter().map(|t| t.tags).collect();
        let trade_links = economy::derive_sector_trade_links(
            &tile_sectors,
            &tile_tags,
            &generated.tile_neighbors,
            sector_count,
        );

        let mut sector_coastal = vec![false; sector_count];
        for t in &tiles {
            let s = usize::from(t.sector_id);
            if s < sector_count && (t.tags.has(TileTag::Coastal) || t.tags.has(TileTag::CoastalSea))
            {
                sector_coastal[s] = true;
            }
        }
        let edges: Vec<(usize, usize, i64, i64)> = trade_links
            .iter()
            .map(|l| {
                (
                    usize::from(l.sector_a.0),
                    usize::from(l.sector_b.0),
                    l.monthly_capacity,
                    l.transport_fee,
                )
            })
            .collect();
        let logistics = LogisticsMatrix::from_links(sector_count, &edges, &sector_coastal);

        Self {
            regions: generated.regions,
            land_tile_count: generated.land_tile_count,
            sea_tile_count: generated.sea_tile_count,
            starter_tile_index,
            tiles,
            slot_owners,
            slot_contracts,
            workshops,
            tile_neighbors: generated.tile_neighbors,
            trade_links,
            sector_coastal,
            logistics,
            enterprises: Vec::new(),
        }
    }

    pub fn referenced_pop_ids_in_sector(&self, sector_id: SectorId) -> BTreeSet<PopId> {
        let mut set = BTreeSet::new();
        for tile in &self.tiles {
            if tile.sector_id != sector_id.0 {
                continue;
            }
            let t = usize::from(tile.index);
            for sk in TileSlotKey::ALL {
                for c in &self.slot_contracts[t][sk.as_index()] {
                    if c.count > 0 {
                        set.insert(c.pop_id);
                    }
                }
            }
        }
        set
    }

    /// Appends or merges a `(pop_id, count, monthly_bill_cents)` contract onto `(tile_index, slot)`.
    pub fn add_slot_contract(
        &mut self,
        tile_index: usize,
        slot: TileSlotKey,
        pop_id: PopId,
        count: u32,
        monthly_bill_cents: i64,
    ) {
        if count == 0 || tile_index >= self.slot_contracts.len() {
            return;
        }
        let list = &mut self.slot_contracts[tile_index][slot.as_index()];
        if let Some(existing) = list.iter_mut().find(|c| c.pop_id == pop_id) {
            existing.count += count;
            existing.monthly_bill_cents += monthly_bill_cents;
        } else {
            list.push(StaffContract {
                pop_id,
                count,
                monthly_bill_cents,
            });
        }
    }

    pub fn slot_owner(&self, tile_index: usize, slot: TileSlotKey) -> BuildingOwner {
        self.slot_owners
            .get(tile_index)
            .map(|arr| arr[slot.as_index()])
            .unwrap_or(BuildingOwner::Unowned)
    }

    pub fn set_slot_owner(&mut self, tile_index: usize, slot: TileSlotKey, owner: BuildingOwner) {
        if let Some(arr) = self.slot_owners.get_mut(tile_index) {
            arr[slot.as_index()] = owner;
        }
    }

    /// Resolves where a building's liquidated cash should flow back to.
    fn owner_cash_sink(&self, owner: BuildingOwner, ruling: PolityId, fallback: SectorId) -> CashSink {
        match owner {
            BuildingOwner::LocalPrivate { home_sector } => CashSink::Capital(home_sector),
            BuildingOwner::Enterprise { enterprise_id } => CashSink::Capital(
                self.enterprises
                    .get(usize::from(enterprise_id.0))
                    .map(|e| e.hq_sector)
                    .unwrap_or(fallback),
            ),
            _ => CashSink::Polity(ruling),
        }
    }

    pub fn check_uniqueness(
        &self,
        target_tile_index: usize,
        replacing_slot: Option<TileSlotKey>,
        def: &BuildingDef,
    ) -> Result<(), String> {
        let target_region = self
            .tiles
            .get(target_tile_index)
            .map(|t| t.region_id)
            .ok_or_else(|| format!("地块序号 {target_tile_index} 超出范围。"))?;

        if def.unique_planet_tags == BuildingTags::EMPTY
            && def.unique_region_tags == BuildingTags::EMPTY
            && def.unique_tile_tags == BuildingTags::EMPTY
        {
            return Ok(());
        }

        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                if t_idx == target_tile_index && replacing_slot == Some(slot_key) {
                    continue;
                }
                let existing = BuildingDef::by_type_id(tid);
                if let Some(conflict) = existing.tags.first_overlap(def.unique_planet_tags) {
                    return Err(format!(
                        "全星球仅限建造 1 座「{}」（地块 #{:04} 已存在「{}」）。",
                        conflict.display_name(),
                        t_idx + 1,
                        existing.name
                    ));
                }
                if tile.region_id == target_region {
                    if let Some(conflict) = existing.tags.first_overlap(def.unique_region_tags) {
                        return Err(format!(
                            "每个大区仅限拥有 1 座「{}」（本大区地块 #{:04} 已有「{}」）。",
                            conflict.display_name(),
                            t_idx + 1,
                            existing.name
                        ));
                    }
                }
                if t_idx == target_tile_index {
                    if let Some(conflict) = existing.tags.first_overlap(def.unique_tile_tags) {
                        return Err(format!(
                            "同一地块仅限建造 1 座「{}」（已有「{}」）。",
                            conflict.display_name(),
                            existing.name
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn sector_cbd_hosting_stats(&self, sector_id: SectorId) -> (u16, u16, u16, u16) {
        let mut cbd_count = 0u16;
        let mut cbd_capacity = 0u16;
        let mut cbd_hosted = 0u16;
        let mut ag_private = 0u16;

        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                let def = BuildingDef::by_type_id(tid);
                let owner = self.slot_owner(t_idx, slot_key);
                if tile.sector_id == sector_id.0 && def.tags.has(BuildingTag::Commercial) {
                    cbd_count += 1;
                    cbd_capacity += def.hosting_capacity;
                }
                if def.is_privatizable_industry() {
                    if let BuildingOwner::LocalPrivate { home_sector } = owner {
                        if home_sector == sector_id {
                            cbd_hosted += 1;
                            if def.tags.has(BuildingTag::Agriculture) {
                                ag_private += 1;
                            }
                        }
                    }
                }
            }
        }
        (cbd_count, cbd_hosted, cbd_capacity, ag_private)
    }

    pub fn enterprise_hosted_count(&self, ent_id: EnterpriseId) -> u16 {
        let mut count = 0u16;
        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                let def = BuildingDef::by_type_id(tid);
                if def.is_privatizable_industry()
                    && matches!(
                        self.slot_owner(t_idx, slot_key),
                        BuildingOwner::Enterprise { enterprise_id } if enterprise_id == ent_id
                    )
                {
                    count += 1;
                }
            }
        }
        count
    }
}

/// Destination of liquidated or transferred cash outside the monthly clearing cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CashSink {
    Polity(PolityId),
    Capital(SectorId),
}

struct SectorSimView {
    sector_id: SectorId,
    pops: SectorPops,
}

struct SectorLaborStats {
    jobs_required: u32,
    jobs_filled: u32,
    employed_by_type: Vec<u32>,
    required_by_type: Vec<u32>,
    has_core: bool,
    building_apple_output: i64,
}

struct PlanetSimEvaluation {
    slot_employed: Vec<[u32; SLOTS_PER_TILE]>,
    slot_bonus_permille: Vec<[i64; SLOTS_PER_TILE]>,
    enterprise_active: Vec<bool>,
    sector_stats: Vec<SectorLaborStats>,
    /// Exact contracted `(headcount, monthly_bill_cents)` per `(sector_vec_idx, group_idx)`.
    pop_contracted: Vec<Vec<(u32, i64)>>,
}

/// Pure evaluation of existing locked `slot_contracts` & nominal output (no re-auction, no cash movement).
fn evaluate_planet_economy(
    catalog: &PopTypeCatalog,
    planet_tiles: &PlanetTiles,
    sectors: &[SectorSimView],
) -> PlanetSimEvaluation {
    let type_count = catalog.len();
    let sector_count = sectors.len();
    let tile_count = planet_tiles.tiles.len();
    let mut slot_employed = vec![[0u32; SLOTS_PER_TILE]; tile_count];
    let mut slot_bonus_permille = vec![[0i64; SLOTS_PER_TILE]; tile_count];
    let mut pop_contracted: Vec<Vec<(u32, i64)>> = sectors
        .iter()
        .map(|v| vec![(0u32, 0i64); v.pops.groups.len()])
        .collect();

    let mut sector_buckets: Vec<Vec<(usize, TileSlotKey, &'static BuildingDef)>> =
        vec![Vec::new(); sector_count];
    let mut sector_stats: Vec<SectorLaborStats> = (0..sector_count)
        .map(|_| SectorLaborStats {
            jobs_required: 0,
            jobs_filled: 0,
            employed_by_type: vec![0; type_count],
            required_by_type: vec![0; type_count],
            has_core: false,
            building_apple_output: 0,
        })
        .collect();

    for tile in &planet_tiles.tiles {
        let s_idx = usize::from(tile.sector_id);
        if s_idx >= sector_count {
            continue;
        }
        let t_idx = usize::from(tile.index);
        for (slot_key, tid) in tile.slots.iter_occupied() {
            let def = BuildingDef::by_type_id(tid);
            if def.tags.has(BuildingTag::RegionalCore) {
                sector_stats[s_idx].has_core = true;
            }
            let scale = def.scale_for_slot(slot_key);
            for &(role_str, base_req) in def.jobs_by_role {
                let req = base_req * scale;
                let pid = def.resolve_role_id(catalog, role_str);
                sector_stats[s_idx].jobs_required += req;
                sector_stats[s_idx].required_by_type[usize::from(pid.0)] += req;
            }

            let mut filled_sum = 0u32;
            for c in &planet_tiles.slot_contracts[t_idx][slot_key.as_index()] {
                if c.count == 0 {
                    continue;
                }
                if let Some(gi) = sectors[s_idx].pops.index_by_id(c.pop_id) {
                    let p_i = usize::from(sectors[s_idx].pops.groups[gi].pop_type.0);
                    if p_i < type_count {
                        sector_stats[s_idx].employed_by_type[p_i] += c.count;
                    }
                    pop_contracted[s_idx][gi].0 += c.count;
                    pop_contracted[s_idx][gi].1 += c.monthly_bill_cents;
                    filled_sum += c.count;
                }
            }
            slot_employed[t_idx][slot_key.as_index()] = filled_sum;
            sector_stats[s_idx].jobs_filled += filled_sum;
            sector_buckets[s_idx].push((t_idx, slot_key, def));
        }
    }

    let mut enterprise_active = vec![false; planet_tiles.enterprises.len()];
    for (e_idx, ent) in planet_tiles.enterprises.iter().enumerate() {
        if let Some(tile) = planet_tiles.tiles.get(ent.campus_tile_index) {
            if let Some(tid) = tile.slots.primary {
                let def = BuildingDef::by_type_id(tid);
                if def.tags.has(BuildingTag::CorporateCampus) {
                    let req = def.slot_jobs_required(TileSlotKey::Primary);
                    let emp = slot_employed[ent.campus_tile_index][TileSlotKey::Primary.as_index()];
                    if req == 0 || emp * 2 >= req {
                        enterprise_active[e_idx] = true;
                    }
                }
            }
        }
    }

    for (s_idx, jobs) in sector_buckets.iter().enumerate() {
        for &(t_idx, slot_key, def) in jobs {
            let owner = planet_tiles.slot_owner(t_idx, slot_key);
            let bonus = match owner {
                BuildingOwner::Enterprise { enterprise_id }
                    if enterprise_active
                        .get(usize::from(enterprise_id.0))
                        .copied()
                        .unwrap_or(false) =>
                {
                    ENTERPRISE_OUTPUT_BONUS_PERMILLE
                }
                _ => 0,
            };
            slot_bonus_permille[t_idx][slot_key.as_index()] = bonus;
            sector_stats[s_idx].building_apple_output += slot_apple_output(
                def,
                slot_key,
                slot_employed[t_idx][slot_key.as_index()],
                bonus,
            );
        }
    }

    PlanetSimEvaluation {
        slot_employed,
        slot_bonus_permille,
        enterprise_active,
        sector_stats,
        pop_contracted,
    }
}

/// Seeds genesis `StaffContract`s on starter buildings at `t = 0` so each starter workplace's staff
/// lives in its exact employer-aligned `PopGroup` with locked genesis wages and starter savings/pantry!
fn seed_initial_planet_contracts(
    goods: &GoodCatalog,
    catalog: &PopTypeCatalog,
    planet_tiles: &mut PlanetTiles,
    sector_pops: &mut BTreeMap<u16, SectorPops>,
) {
    let cash_id = goods.cash_id();
    let apple_id = goods.id_of(GOOD_APPLE);
    let mut slots_by_sector: BTreeMap<u16, Vec<(usize, TileSlotKey, &'static BuildingDef, u16)>> =
        BTreeMap::new();
    for tile in &planet_tiles.tiles {
        if tile.sector_id == u16::MAX {
            continue;
        }
        let t_idx = usize::from(tile.index);
        let hab = tile.tags.habitability_permille();
        for (sk, tid) in tile.slots.iter_occupied() {
            slots_by_sector
                .entry(tile.sector_id)
                .or_default()
                .push((t_idx, sk, BuildingDef::by_type_id(tid), hab));
        }
    }

    let default_pool = catalog.default_pool_id();
    for (sec_u16, mut jobs) in slots_by_sector {
        let Some(pops) = sector_pops.get_mut(&sec_u16) else {
            continue;
        };
        jobs.sort_by(|a, b| {
            a.2.job_priority
                .cmp(&b.2.job_priority)
                .then_with(|| b.3.cmp(&a.3))
                .then_with(|| a.1.as_index().cmp(&b.1.as_index()))
                .then_with(|| a.0.cmp(&b.0))
        });

        // Track total starter jobs per role in this sector so `def.initial_savings` distributes cleanly.
        let mut role_total_req = vec![0u32; catalog.len()];
        for &(_, sk, def, _) in &jobs {
            let scale = def.scale_for_slot(sk);
            for &(role_str, base_req) in def.jobs_by_role {
                let pid = def.resolve_role_id(catalog, role_str);
                role_total_req[usize::from(pid.0)] += base_req * scale;
            }
        }

        for (t_idx, sk, def, _) in jobs {
            let owner = planet_tiles.slot_owner(t_idx, sk);
            let scale = def.scale_for_slot(sk);
            for &(role_str, base_req) in def.jobs_by_role {
                let req = base_req * scale;
                if req == 0 {
                    continue;
                }
                let role_id = def.resolve_role_id(catalog, role_str);
                let r_def = catalog.get(role_id);
                let wage_per_worker_cents = r_def.base_wage_per_100 * CASH_SCALE / 100;

                // Draw starter staff from `default_pool` (leaving `unemployed_pool` intact as urban buffer!).
                let Some(src_idx) = pops.find_index(default_pool, 0, BuildingOwner::Unowned) else {
                    continue;
                };
                let avail = pops.groups[src_idx].count;
                let take = req.min(avail);
                if take == 0 {
                    continue;
                }
                let dst_idx = pops.ensure_group_index(goods, role_id, 0, owner);
                let dst_id = pops.groups[dst_idx].id;
                pops.groups[src_idx].count -= take;
                pops.groups[src_idx].sync_capacity();
                pops.groups[dst_idx].count += take;
                pops.groups[dst_idx].sync_capacity();

                let tot_role = role_total_req[usize::from(role_id.0)].max(1);
                let add_cash_cents = (r_def.initial_savings * CASH_SCALE * i64::from(take))
                    / i64::from(tot_role);
                pops.groups[dst_idx].inv.available[usize::from(cash_id.0)] += add_cash_cents;
                if let Some(aid) = apple_id {
                    let add_food = economy::compute_apple_demand(take);
                    pops.groups[dst_idx].inv.produce_capped(goods, aid, add_food);
                }

                let bill = i64::from(take) * wage_per_worker_cents;
                planet_tiles.add_slot_contract(t_idx, sk, dst_id, take, bill);
            }
        }
        // Seed any remaining `capital_starter_count` above starter job demand as `XX · 待业` (`Unowned`)
        // (e.g. `劳工 · 待业` urban job-seeking buffer with proportional savings & pantry!).
        for (idx, r_def) in catalog.defs().iter().enumerate() {
            let role_id = PopTypeId(idx as u16);
            if r_def.is_default_pool || r_def.is_vagrant_pool {
                continue;
            }
            let used = role_total_req[idx];
            if used > 0 && r_def.capital_starter_count > used {
                let extra_want = r_def.capital_starter_count - used;
                let Some(src_idx) = pops.find_index(default_pool, 0, BuildingOwner::Unowned) else {
                    break;
                };
                let take = extra_want.min(pops.groups[src_idx].count);
                if take > 0 {
                    let dst_idx =
                        pops.ensure_group_index(goods, role_id, 0, BuildingOwner::Unowned);
                    pops.groups[src_idx].count -= take;
                    pops.groups[src_idx].sync_capacity();
                    pops.groups[dst_idx].count += take;
                    pops.groups[dst_idx].sync_capacity();

                    let denom = r_def.capital_starter_count.max(1);
                    let add_cash_cents = (r_def.initial_savings * CASH_SCALE * i64::from(take))
                        / i64::from(denom);
                    pops.groups[dst_idx].inv.available[usize::from(cash_id.0)] += add_cash_cents;
                    if let Some(aid) = apple_id {
                        let add_food = economy::compute_apple_demand(take);
                        pops.groups[dst_idx].inv.produce_capped(goods, aid, add_food);
                    }
                }
            }
        }
        // Sync `default_pool` starter food to its post-staffing headcount.
        if let Some(sub_idx) = pops.find_index(default_pool, 0, BuildingOwner::Unowned) {
            if let Some(aid) = apple_id {
                let target_food = economy::compute_apple_demand(pops.groups[sub_idx].count);
                pops.groups[sub_idx].inv.available[usize::from(aid.0)] =
                    target_food.min(pops.groups[sub_idx].inv.capacity);
            }
        }
    }
}

/// Effective monthly apple output of one slot after scale, enterprise bonus and staffing.
fn slot_apple_output(def: &BuildingDef, slot: TileSlotKey, employed: u32, bonus_permille: i64) -> i64 {
    let scale = def.scale_for_slot(slot);
    let req = def.slot_jobs_required(slot);
    let mut out = 0i64;
    for &(good_id, unit_delta) in def.monthly_output {
        if good_id == GOOD_APPLE {
            let base_out = unit_delta * i64::from(scale);
            let with_bonus = (base_out * (1000 + bonus_permille) + 500) / 1000;
            out += scale_output_by_staffing(with_bonus, employed, req);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InCombat {
    pub subticks_fought: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimeSnapshot {
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
    pub id: String,
    pub name: String,
    pub radius_km: f64,
    pub position: Position2d,
    pub appearance: Appearance,
    pub polity: PolitySnapshot,
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
    pub occupied_tiles: Vec<OccupiedTileSnapshot>,
}

#[derive(Clone, Serialize)]
pub struct GameSnapshot {
    pub time: TimeSnapshot,
    pub polities: Vec<PolitySnapshot>,
    pub planets: Vec<PlanetSnapshot>,
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
}

/// Macro policy changes routed through one runtime command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyChange {
    Monetary(MonetaryPolicy),
    Depot { sector_id: u16, policy: DepotPolicy },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickCounters {
    pub combat_subticks: u64,
    pub daily_ticks: u64,
    pub monthly_ticks: u64,
    pub yearly_ticks: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepOutcome {
    pub day_advanced: bool,
    pub month_advanced: bool,
    pub year_advanced: bool,
}

pub struct Game {
    world: World,
    calendar: CalendarConfig,
    pop_catalog: PopTypeCatalog,
    goods: GoodCatalog,
    elapsed_subticks: u64,
    speed: GameSpeed,
    counters: TickCounters,
}

impl Default for Game {
    fn default() -> Self {
        Self::with_calendar(CalendarConfig::default())
    }
}

impl Game {
    pub fn with_calendar(calendar: CalendarConfig) -> Self {
        Self::with_pop_catalog(calendar, PopTypeCatalog::default())
    }

    pub fn with_pop_catalog(calendar: CalendarConfig, pop_catalog: PopTypeCatalog) -> Self {
        let mut world = World::new();
        let goods = GoodCatalog::default();

        let earth_polity = Polity::earth_default(&goods);
        let earth_polity_id = earth_polity.id;
        world.spawn((earth_polity,));

        let earth_id = "base:earth";
        let earth_seed = 42u32;
        let mut earth_tiles =
            PlanetTiles::from_planet_seed(earth_id, earth_seed, earth_polity_id, &goods);

        let starter_sector_u16 = earth_tiles
            .tiles
            .get(earth_tiles.starter_tile_index)
            .map(|t| t.sector_id)
            .unwrap_or(0);

        let mut sector_meta: BTreeMap<u16, (u16, String, u32)> = BTreeMap::new();
        for tile in &earth_tiles.tiles {
            if tile.sector_id == u16::MAX {
                continue;
            }
            let reg_name = earth_tiles
                .regions
                .get(usize::from(tile.region_id))
                .map(|r| r.name.clone())
                .unwrap_or_else(|| format!("{:02} · 陆地辖区", tile.sector_id + 1));
            let entry = sector_meta
                .entry(tile.sector_id)
                .or_insert((tile.region_id, reg_name, 0));
            if tile.tags.has(TileTag::Land) {
                entry.2 += u32::from(tile.tags.habitability_permille());
            }
        }

        let mut sector_pops_map: BTreeMap<u16, SectorPops> = BTreeMap::new();
        for (&sec_u16, &(_, _, hab_permille)) in &sector_meta {
            let is_cap = sec_u16 == starter_sector_u16;
            let base = if is_cap { 620 } else { 180 };
            let raw = base + (hab_permille * 38) / 1000;
            let initial_pop = ((raw + 5) / 10) * 10;
            sector_pops_map.insert(
                sec_u16,
                SectorPops::from_catalog(&goods, &pop_catalog, initial_pop, is_cap),
            );
        }
        seed_initial_planet_contracts(
            &goods,
            &pop_catalog,
            &mut earth_tiles,
            &mut sector_pops_map,
        );

        for (sec_u16, (geo_region_id, name, hab_permille)) in sector_meta {
            let is_cap = sec_u16 == starter_sector_u16;

            let sector = Sector {
                id: SectorId(sec_u16),
                planet_id: earth_id.into(),
                geo_region_id,
                owner_polity: earth_polity_id,
                name,
                allow_private_land_use: true,
                habitability_sum_permille: hab_permille,
                is_coastal: earth_tiles
                    .sector_coastal
                    .get(usize::from(sec_u16))
                    .copied()
                    .unwrap_or(false),
                state_depot: Inventory::new(goods.len(), economy::BASE_SECTOR_DEPOT_CAPACITY),
                depot_policy: DepotPolicy::Idle,
            };
            let market = SectorMarket::new(&goods, &pop_catalog);
            let capital = SectorCapital::new(&goods, if is_cap { 320 } else { 140 });
            let pops = sector_pops_map.remove(&sec_u16).unwrap();
            world.spawn((sector, market, capital, pops));
        }

        world.spawn((
            Planet {
                id: earth_id.into(),
                name: "地球".into(),
                radius_km: 6371.0,
                ruling_polity: earth_polity_id,
            },
            Position2d { x: 0.0, y: 0.0 },
            Appearance {
                ocean_rgb: [44, 101, 139],
                land_rgb: [115, 145, 125],
                seed: earth_seed,
            },
            earth_tiles,
        ));

        Self {
            world,
            calendar,
            pop_catalog,
            goods,
            elapsed_subticks: 0,
            speed: GameSpeed::Paused,
            counters: TickCounters::default(),
        }
    }

    #[inline]
    pub fn goods(&self) -> &GoodCatalog {
        &self.goods
    }

    fn polity_cash_cents(&self, polity: PolityId) -> i64 {
        self.world
            .query::<&Polity>()
            .iter()
            .find_map(|(_, pol)| (pol.id == polity).then(|| pol.cash_cents(&self.goods)))
            .unwrap_or(0)
    }

    fn sector_pool_cents(&self, planet_id: &str, sector: SectorId) -> i64 {
        self.world
            .query::<(&Sector, &SectorCapital)>()
            .iter()
            .find_map(|(_, (s, cap))| {
                (s.planet_id == planet_id && s.id == sector).then(|| cap.pool_cents(&self.goods))
            })
            .unwrap_or(0)
    }

    /// Adds (or with a negative amount removes) cash directly in a polity vault or sector capital pool.
    /// Used for construction spending, buyouts and liquidation outside the monthly clearing cycle.
    fn adjust_cash(&mut self, planet_id: &str, sink: CashSink, delta_cents: i64) {
        let c = usize::from(self.goods.cash_id().0);
        match sink {
            CashSink::Polity(pid) => {
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == pid {
                        pol.vault.available[c] += delta_cents;
                        break;
                    }
                }
            }
            CashSink::Capital(sid) => {
                for (_, (sec, cap)) in self.world.query_mut::<(&Sector, &mut SectorCapital)>() {
                    if sec.planet_id == planet_id && sec.id == sid {
                        cap.vault.available[c] += delta_cents;
                        break;
                    }
                }
            }
        }
    }

    /// Releases all `StaffContract`s previously held on `(tile_index, slot)` into each staff group's
    /// own between-jobs row `(same_role, same_culture, Unowned)`.
    fn release_contracts_to_between_jobs(
        &mut self,
        planet_id: &str,
        sector_id: SectorId,
        contracts: Vec<StaffContract>,
        satisfaction_penalty: u8,
    ) {
        if contracts.is_empty() {
            return;
        }
        let def_pool = self.pop_catalog.default_pool_id();
        let mut ref_ids = BTreeSet::new();
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == planet_id {
                ref_ids = pt.referenced_pop_ids_in_sector(sector_id);
                break;
            }
        }
        for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.planet_id == planet_id && sec.id == sector_id {
                for c in contracts {
                    pops.release_to_between_jobs(
                        &self.goods,
                        c.pop_id,
                        c.count,
                        satisfaction_penalty,
                    );
                }
                pops.prune_empty_groups(&self.goods, def_pool, &ref_ids);
                break;
            }
        }
    }

    /// Migrates in-force `StaffContract`s on each `(tile_index, slot)` to `new_owner`'s matching
    /// `(pop_type, culture_id, new_owner)` `PopGroup`, preserving locked `monthly_bill_cents` & household assets!
    fn migrate_slot_contracts_owners(
        &mut self,
        planet_id: &str,
        transitions: &[(usize, TileSlotKey, BuildingOwner)],
    ) {
        if transitions.is_empty() {
            return;
        }
        let def_pool = self.pop_catalog.default_pool_id();
        for &(tile_index, slot, new_owner) in transitions {
            let mut tile_sec_opt = None;
            let mut old_contracts = Vec::new();
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id == planet_id && tile_index < pt.tiles.len() {
                    let s_raw = pt.tiles[tile_index].sector_id;
                    if s_raw != u16::MAX {
                        tile_sec_opt = Some(SectorId(s_raw));
                        old_contracts =
                            std::mem::take(&mut pt.slot_contracts[tile_index][slot.as_index()]);
                    }
                    break;
                }
            }
            let Some(tile_sec) = tile_sec_opt else {
                continue;
            };
            if old_contracts.is_empty() {
                continue;
            }

            let mut updated_contracts: Vec<StaffContract> = Vec::with_capacity(old_contracts.len());
            for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                if sec.planet_id == planet_id && sec.id == tile_sec {
                    for c in old_contracts {
                        if c.count == 0 {
                            continue;
                        }
                        if let Some(src_idx) = pops.index_by_id(c.pop_id) {
                            let role = pops.groups[src_idx].pop_type;
                            let cid = pops.groups[src_idx].culture_id;
                            let dst_idx = pops.ensure_group_index(&self.goods, role, cid, new_owner);
                            let dst_id = pops.groups[dst_idx].id;
                            let moved = pops.transfer_between_groups(
                                &self.goods,
                                src_idx,
                                dst_idx,
                                c.count,
                            );
                            if moved > 0 {
                                let bill = if moved == c.count {
                                    c.monthly_bill_cents
                                } else {
                                    (c.monthly_bill_cents * i64::from(moved)) / i64::from(c.count)
                                };
                                if let Some(ex) =
                                    updated_contracts.iter_mut().find(|u| u.pop_id == dst_id)
                                {
                                    ex.count += moved;
                                    ex.monthly_bill_cents += bill;
                                } else {
                                    updated_contracts.push(StaffContract {
                                        pop_id: dst_id,
                                        count: moved,
                                        monthly_bill_cents: bill,
                                    });
                                }
                            }
                        }
                    }
                    break;
                }
            }

            let mut ref_ids = BTreeSet::new();
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id == planet_id {
                    pt.slot_contracts[tile_index][slot.as_index()] = updated_contracts;
                    ref_ids = pt.referenced_pop_ids_in_sector(tile_sec);
                    break;
                }
            }
            for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                if sec.planet_id == planet_id && sec.id == tile_sec {
                    pops.prune_empty_groups(&self.goods, def_pool, &ref_ids);
                    break;
                }
            }
        }
    }

    pub fn set_monetary_policy(
        &mut self,
        planet_id: &str,
        policy: MonetaryPolicy,
    ) -> Result<PlanetSnapshot, String> {
        let ruling = self
            .world
            .query::<&Planet>()
            .iter()
            .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
            .ok_or_else(|| format!("找不到星球：{planet_id}"))?;
        for (_, pol) in self.world.query_mut::<&mut Polity>() {
            if pol.id == ruling {
                pol.monetary_policy = policy;
            }
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn set_depot_policy(
        &mut self,
        planet_id: &str,
        sector_id: u16,
        policy: DepotPolicy,
    ) -> Result<PlanetSnapshot, String> {
        let mut found = false;
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == SectorId(sector_id) {
                sec.depot_policy = policy;
                found = true;
            }
        }
        if !found {
            return Err(format!("找不到辖区编号：{sector_id}"));
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
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

    pub fn build_on_tile(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        building_id: &str,
        slot: Option<TileSlotKey>,
    ) -> Result<PlanetSnapshot, String> {
        let def = BuildingDef::by_id(building_id)?;

        let (ruling_polity, tile_sector_opt) = {
            let mut found = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id == planet_id {
                    if tile_index >= pt.tiles.len() {
                        return Err(format!("地块序号 {tile_index} 超出范围。"));
                    }
                    let sec_raw = pt.tiles[tile_index].sector_id;
                    let sec_opt = (sec_raw != u16::MAX).then_some(SectorId(sec_raw));
                    found = Some((planet.ruling_polity, sec_opt));
                    break;
                }
            }
            found.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let is_campus = def.tags.has(BuildingTag::CorporateCampus);
        let is_cbd = def.tags.has(BuildingTag::Commercial);

        if (is_campus || is_cbd) && tile_sector_opt.is_none() {
            return Err(format!("「{}」必须建造在已设立实控辖区的陆地地块上。", def.name));
        }

        // All amounts below are whole credits; vault balances are converted from cents.
        let treasury_avail = self.polity_cash_cents(ruling_polity) / CASH_SCALE;
        let sector_pool_avail = tile_sector_opt
            .map(|sid| self.sector_pool_cents(planet_id, sid) / CASH_SCALE)
            .unwrap_or(0);

        let mut treasury_charge: i64 = 0;
        let mut private_pool_charge: i64 = 0;
        let mut compensation_to_sector: Option<(SectorId, i64)> = None;
        let mut liquidated: Option<(Workshop, CashSink)> = None;
        let mut replaced_contracts: Vec<StaffContract> = Vec::new();
        let mut owner_transitions: Vec<(usize, TileSlotKey, BuildingOwner)> = Vec::new();

        for (_, (planet, planet_tiles)) in self
            .world
            .query_mut::<(&Planet, &mut PlanetTiles)>()
        {
            if planet.id != planet_id {
                continue;
            }
            let tile_tags = planet_tiles.tiles[tile_index].tags;
            if !def.matches_tags(tile_tags) {
                return Err(format!(
                    "地块 #{:04} 的地形标签不满足「{}」的建造要求。",
                    tile_index + 1,
                    def.name
                ));
            }

            let target_slot = match slot {
                Some(explicit) => {
                    if !def.can_use_slot(explicit) {
                        return Err(format!(
                            "「{}」不能放入{}。",
                            def.name,
                            explicit.label()
                        ));
                    }
                    if !explicit.is_primary()
                        && planet_tiles.tiles[tile_index].slots.primary.is_none()
                    {
                        return Err(
                            "请先在主建筑槽确立该地块的主导产业或核心建筑，再开发次建筑槽。"
                                .to_string(),
                        );
                    }
                    if let Some(existing_tid) = planet_tiles.tiles[tile_index].slots.get(explicit) {
                        if !explicit.is_primary() {
                            return Err(format!(
                                "地块 #{:04} 的{}已有建筑，请先拆除后再建。",
                                tile_index + 1,
                                explicit.label()
                            ));
                        }
                        let existing_def = BuildingDef::by_type_id(existing_tid);
                        if existing_tid == def.type_id {
                            return Err(format!(
                                "地块 #{:04} 的主建筑槽已是「{}」。",
                                tile_index + 1,
                                def.name
                            ));
                        }
                        if existing_def.is_state_core_or_public()
                            || existing_def.tags.has(BuildingTag::CorporateCampus)
                        {
                            return Err(format!(
                                "地块 #{:04} 的主建筑槽为「{}」，不可直接覆盖改建。",
                                tile_index + 1,
                                existing_def.name
                            ));
                        }
                    }
                    explicit
                }
                None => planet_tiles.tiles[tile_index]
                    .slots
                    .first_compatible_empty_slot(def)
                    .ok_or_else(|| {
                        if planet_tiles.tiles[tile_index].slots.primary.is_none()
                            && !def.can_use_slot(TileSlotKey::Primary)
                        {
                            "请先在主建筑槽确立该地块的主导产业或核心建筑，再开发次建筑槽。"
                                .to_string()
                        } else {
                            format!(
                                "地块 #{:04} 已无适配「{}」的空置建筑槽。",
                                tile_index + 1,
                                def.name
                            )
                        }
                    })?,
            };

            if is_cbd {
                let valid_hub = planet_tiles.tiles[tile_index]
                    .slots
                    .primary
                    .map(|ptid| {
                        let pdef = BuildingDef::by_type_id(ptid);
                        pdef.tags.has(BuildingTag::RegionalCore)
                            || pdef.tags.has(BuildingTag::CorporateCampus)
                    })
                    .unwrap_or(false);
                if !valid_hub {
                    return Err(
                        "「商业街」仅可依托行星首府、大区核心或企业园区所在的城镇/园区地块兴建。"
                            .to_string(),
                    );
                }
            }

            let replacing_slot = planet_tiles.tiles[tile_index]
                .slots
                .get(target_slot)
                .map(|_| target_slot);
            planet_tiles.check_uniqueness(tile_index, replacing_slot, def)?;

            if is_campus {
                let sid = tile_sector_opt.unwrap();
                let (_, _, _, ag_private) = planet_tiles.sector_cbd_hosting_stats(sid);
                let replacing_self_private = replacing_slot
                    .map(|s| {
                        matches!(
                            planet_tiles.slot_owner(tile_index, s),
                            BuildingOwner::LocalPrivate { home_sector } if home_sector == sid
                        )
                    })
                    .unwrap_or(false);
                let effective_count = usize::from(ag_private)
                    .saturating_sub(usize::from(replacing_self_private));
                if usize::from(ag_private) < ENTERPRISE_FOUNDING_MIN_PRIVATE || effective_count < 2
                {
                    return Err(format!(
                        "本辖区需先积累至少 {} 座同行业个体私产（当前 {} 座），方可批地组建「企业园区」。",
                        ENTERPRISE_FOUNDING_MIN_PRIVATE, ag_private
                    ));
                }
            }

            let cost = def.slot_build_cost(target_slot);
            if is_campus {
                let use_priv = sector_pool_avail.max(0).min(cost);
                let rem_treasury = cost - use_priv;
                if treasury_avail < rem_treasury {
                    return Err(format!(
                        "资金不足：批建「企业园区」需 {cost} 资金（当前本区民间投资池 {sector_pool_avail} + 国库 {treasury_avail}）。"
                    ));
                }
                private_pool_charge = use_priv;
                treasury_charge = rem_treasury;
            } else {
                let buyout = if let Some(rep_s) = replacing_slot {
                    match planet_tiles.slot_owner(tile_index, rep_s) {
                        BuildingOwner::LocalPrivate { home_sector } => {
                            compensation_to_sector = Some((home_sector, 150));
                            150
                        }
                        BuildingOwner::Enterprise { enterprise_id } => {
                            let hq = planet_tiles
                                .enterprises
                                .get(usize::from(enterprise_id.0))
                                .map(|e| e.hq_sector)
                                .unwrap_or(SectorId(0));
                            compensation_to_sector = Some((hq, 150));
                            150
                        }
                        _ => 0,
                    }
                } else {
                    0
                };
                if treasury_avail < cost + buyout {
                    return Err(format!(
                        "中央国库资金不足：建造「{}」需 {} 资金（当前国库 {}）。",
                        def.name,
                        cost + buyout,
                        treasury_avail
                    ));
                }
                treasury_charge = cost + buyout;
            }

            // Replacing a productive building liquidates its warehouse (cash returns to old owner)
            // and releases its contracted staff into `失业者`.
            replaced_contracts =
                std::mem::take(&mut planet_tiles.slot_contracts[tile_index][target_slot.as_index()]);
            if let Some(old_ws) =
                planet_tiles.workshops[tile_index][target_slot.as_index()].take()
            {
                let fallback = tile_sector_opt.unwrap_or(SectorId(0));
                let sink = planet_tiles.owner_cash_sink(
                    planet_tiles.slot_owner(tile_index, target_slot),
                    ruling_polity,
                    fallback,
                );
                liquidated = Some((old_ws, sink));
            }
            planet_tiles.tiles[tile_index]
                .slots
                .set(target_slot, Some(def.type_id));
            planet_tiles.workshops[tile_index][target_slot.as_index()] =
                make_workshop(&self.goods, &self.pop_catalog, def, target_slot);

            if is_campus {
                let sid = tile_sector_opt.unwrap();
                let ent_id = EnterpriseId(planet_tiles.enterprises.len() as u16);
                let reg_short = planet_tiles
                    .regions
                    .get(usize::from(planet_tiles.tiles[tile_index].region_id))
                    .and_then(|r| r.name.split(" · ").nth(1))
                    .unwrap_or("泰拉");
                let seq = planet_tiles
                    .enterprises
                    .iter()
                    .filter(|e| e.hq_sector == sid)
                    .count()
                    + 1;
                let ent_name = if seq == 1 {
                    format!("{reg_short}农垦集团")
                } else {
                    format!("{reg_short}农垦集团·{seq}号")
                };

                planet_tiles.enterprises.push(Enterprise {
                    id: ent_id,
                    name: ent_name,
                    industry_tag: BuildingTag::Agriculture.as_str().to_string(),
                    hq_sector: sid,
                    campus_tile_index: tile_index,
                    hosting_capacity: CAMPUS_HOSTING_CAPACITY,
                    last_revenue_cents: 0,
                    last_profit_cents: 0,
                });
                let ent_owner = BuildingOwner::Enterprise {
                    enterprise_id: ent_id,
                };
                planet_tiles.set_slot_owner(tile_index, target_slot, ent_owner);

                let mut absorbed = 0u16;
                for t_i in 0..planet_tiles.tiles.len() {
                    for sk in TileSlotKey::ALL {
                        if absorbed >= CAMPUS_HOSTING_CAPACITY {
                            break;
                        }
                        if let Some(tid) = planet_tiles.tiles[t_i].slots.get(sk) {
                            let bdef = BuildingDef::by_type_id(tid);
                            if bdef.tags.has(BuildingTag::Agriculture)
                                && matches!(
                                    planet_tiles.slot_owner(t_i, sk),
                                    BuildingOwner::LocalPrivate { home_sector } if home_sector == sid
                                )
                            {
                                planet_tiles.set_slot_owner(t_i, sk, ent_owner);
                                owner_transitions.push((t_i, sk, ent_owner));
                                absorbed += 1;
                            }
                        }
                    }
                }
            } else if is_cbd {
                let sid = tile_sector_opt.unwrap();
                planet_tiles.set_slot_owner(
                    tile_index,
                    target_slot,
                    BuildingOwner::LocalPrivate { home_sector: sid },
                );
            } else {
                planet_tiles.set_slot_owner(
                    tile_index,
                    target_slot,
                    BuildingOwner::State {
                        polity_id: ruling_polity,
                    },
                );
            }
            break;
        }

        // ponytail: construction spending leaves the economy (acts as a money sink against minting);
        // a construction-materials market can replace this later.
        if treasury_charge != 0 {
            self.adjust_cash(planet_id, CashSink::Polity(ruling_polity), -treasury_charge * CASH_SCALE);
        }
        if private_pool_charge != 0 {
            if let Some(sid) = tile_sector_opt {
                self.adjust_cash(planet_id, CashSink::Capital(sid), -private_pool_charge * CASH_SCALE);
            }
        }
        if let Some((comp_sid, amount)) = compensation_to_sector {
            self.adjust_cash(planet_id, CashSink::Capital(comp_sid), amount * CASH_SCALE);
        }
        if let Some((ws, sink)) = liquidated {
            let cash = ws.inv.total(self.goods.cash_id());
            self.adjust_cash(planet_id, sink, cash);
        }
        if !replaced_contracts.is_empty() {
            if let Some(sid) = tile_sector_opt {
                self.release_contracts_to_between_jobs(planet_id, sid, replaced_contracts, 5);
            }
        }
        if !owner_transitions.is_empty() {
            self.migrate_slot_contracts_owners(planet_id, &owner_transitions);
        }

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn build_in_region(
        &mut self,
        planet_id: &str,
        region_id: u16,
        building_id: &str,
    ) -> Result<PlanetSnapshot, String> {
        let def = BuildingDef::by_id(building_id)?;
        let mut chosen_target: Option<(usize, TileSlotKey)> = None;

        for (_, (planet, planet_tiles)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id != planet_id {
                continue;
            }
            if usize::from(region_id) >= planet_tiles.regions.len() {
                return Err(format!("无效的大区编号：{region_id}"));
            }
            if let Some(any_in_region) = planet_tiles.tiles.iter().find(|t| t.region_id == region_id)
            {
                if def.unique_planet_tags != BuildingTags::EMPTY
                    || def.unique_region_tags != BuildingTags::EMPTY
                {
                    planet_tiles.check_uniqueness(usize::from(any_in_region.index), None, def)?;
                }
            }
            let is_ocean_region = planet_tiles.regions[usize::from(region_id)].is_ocean;

            let best = planet_tiles
                .tiles
                .iter()
                .filter(|t| {
                    t.region_id == region_id
                        && def.matches_tags(t.tags)
                        && planet_tiles
                            .check_uniqueness(usize::from(t.index), None, def)
                            .is_ok()
                })
                .filter_map(|t| {
                    if def.tags.has(BuildingTag::Commercial) {
                        let valid_hub = t
                            .slots
                            .primary
                            .map(|ptid| {
                                let pdef = BuildingDef::by_type_id(ptid);
                                pdef.tags.has(BuildingTag::RegionalCore)
                                    || pdef.tags.has(BuildingTag::CorporateCampus)
                            })
                            .unwrap_or(false);
                        if !valid_hub {
                            return None;
                        }
                    }

                    let slot = match def.slot_rule {
                        SlotRule::PrimaryOnly => {
                            t.slots.primary.is_none().then_some(TileSlotKey::Primary)?
                        }
                        SlotRule::SecondaryOnly => {
                            if t.slots.primary.is_none() {
                                return None;
                            }
                            [
                                TileSlotKey::Secondary0,
                                TileSlotKey::Secondary1,
                                TileSlotKey::Secondary2,
                            ]
                            .into_iter()
                            .find(|&k| t.slots.get(k).is_none())?
                        }
                        SlotRule::Any => {
                            if t.slots.primary.is_some() {
                                [
                                    TileSlotKey::Secondary0,
                                    TileSlotKey::Secondary1,
                                    TileSlotKey::Secondary2,
                                ]
                                .into_iter()
                                .find(|&k| t.slots.get(k).is_none())?
                            } else {
                                TileSlotKey::Primary
                            }
                        }
                    };

                    let tier_bonus = if !slot.is_primary() { 50.0 } else { 0.0 };
                    let domain_bonus = if !is_ocean_region && t.tags.has(TileTag::Land) {
                        15.0
                    } else {
                        0.0
                    };
                    let score = tier_bonus + domain_bonus + t.tags.habitability();
                    Some((usize::from(t.index), slot, score))
                })
                .max_by(|a, b| {
                    a.2.partial_cmp(&b.2)
                        .unwrap_or(CmpOrdering::Equal)
                        .then_with(|| b.0.cmp(&a.0))
                });

            chosen_target = best.map(|(idx, slot, _)| (idx, slot));
            break;
        }

        let Some((tile_index, slot)) = chosen_target else {
            return Err(format!(
                "该大区已无满足「{}」建造条件与槽位的空置地块。",
                def.name
            ));
        };
        self.build_on_tile(planet_id, tile_index, building_id, Some(slot))
    }

    pub fn demolish_on_tile(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let mut found = false;
        let mut demolished_liquidated: Option<(Workshop, CashSink)> = None;
        let mut demolished_contracts: Option<(SectorId, Vec<StaffContract>)> = None;
        for (_, (planet, planet_tiles)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            found = true;
            if tile_index >= planet_tiles.tiles.len() {
                return Err(format!("地块序号 {tile_index} 超出范围。"));
            }
            let Some(tid) = planet_tiles.tiles[tile_index].slots.get(slot) else {
                return Err("该建筑槽当前为空，无需拆除。".to_string());
            };
            let def = BuildingDef::by_type_id(tid);
            if def.tags.has(BuildingTag::PlanetaryCapital) {
                return Err("「行星首府」不可直接拆除。".to_string());
            }
            if slot.is_primary() && planet_tiles.tiles[tile_index].slots.has_any_secondary() {
                return Err(
                    "次建筑槽仍有设施依托运行：请先拆除次槽建筑，或直接将主槽改建为其他主导建筑。"
                        .to_string(),
                );
            }
            if def.tags.has(BuildingTag::Commercial) {
                let sec_raw = planet_tiles.tiles[tile_index].sector_id;
                if sec_raw != u16::MAX {
                    let (_, hosted, cap, _) =
                        planet_tiles.sector_cbd_hosting_stats(SectorId(sec_raw));
                    if cap.saturating_sub(def.hosting_capacity) < hosted {
                        return Err(format!(
                            "该商业街正在托管私营建筑（当前本区托管 {hosted}/{cap}），请先赎买国有化或扩建商业容量后再拆除。"
                        ));
                    }
                }
            }
            if def.tags.has(BuildingTag::CorporateCampus) {
                if let BuildingOwner::Enterprise { enterprise_id } =
                    planet_tiles.slot_owner(tile_index, slot)
                {
                    let hosted = planet_tiles.enterprise_hosted_count(enterprise_id);
                    if hosted > 0 {
                        return Err(format!(
                            "该企业园区名下仍托管有 {hosted} 座企业建筑，请先赎买国有化后再拆除。"
                        ));
                    }
                }
            }

            let sec_raw = planet_tiles.tiles[tile_index].sector_id;
            if sec_raw != u16::MAX {
                let c = std::mem::take(&mut planet_tiles.slot_contracts[tile_index][slot.as_index()]);
                if !c.is_empty() {
                    demolished_contracts = Some((SectorId(sec_raw), c));
                }
            }
            if let Some(old_ws) = planet_tiles.workshops[tile_index][slot.as_index()].take() {
                let fallback = SectorId(if sec_raw != u16::MAX { sec_raw } else { 0 });
                let sink = planet_tiles.owner_cash_sink(
                    planet_tiles.slot_owner(tile_index, slot),
                    planet.ruling_polity,
                    fallback,
                );
                demolished_liquidated = Some((old_ws, sink));
            }
            planet_tiles.tiles[tile_index].slots.set(slot, None);
            planet_tiles.set_slot_owner(tile_index, slot, BuildingOwner::Unowned);
            break;
        }
        if !found {
            return Err(format!("找不到星球：{planet_id}"));
        }
        if let Some((ws, sink)) = demolished_liquidated {
            let cash = ws.inv.total(self.goods.cash_id());
            self.adjust_cash(planet_id, sink, cash);
        }
        if let Some((sid, contracts)) = demolished_contracts {
            self.release_contracts_to_between_jobs(planet_id, sid, contracts, 6);
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn privatize_slot(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (ruling_polity, local_sector_opt, price, is_ag) = {
            let mut info = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该槽位当前没有建筑。".to_string());
                };
                let def = BuildingDef::by_type_id(tid);
                if !def.is_privatizable_industry() {
                    return Err(format!("「{}」属于公共或产权中枢，不可挂牌私有化。", def.name));
                }
                if !matches!(pt.slot_owner(tile_index, slot), BuildingOwner::State { .. }) {
                    return Err("该建筑已属民间或企业产权，无需重复私有化。".to_string());
                }
                let sale_price = (def.slot_build_cost(slot) * 80) / 100;
                let sec_raw = pt.tiles[tile_index].sector_id;
                let sec_opt = (sec_raw != u16::MAX).then_some(SectorId(sec_raw));
                info = Some((
                    planet.ruling_polity,
                    sec_opt,
                    sale_price,
                    def.tags.has(BuildingTag::Agriculture),
                ));
                break;
            }
            info.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let mut pools: BTreeMap<SectorId, i64> = BTreeMap::new();
        for (_, (sec, cap)) in self.world.query::<(&Sector, &SectorCapital)>().iter() {
            if sec.planet_id == planet_id && sec.owner_polity == ruling_polity {
                pools.insert(sec.id, cap.pool_cents(&self.goods) / CASH_SCALE);
            }
        }

        let mut chosen_owner: Option<(BuildingOwner, SectorId, Option<(usize, TileSlotKey)>)> = None;

        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id != planet_id {
                continue;
            }
            if is_ag {
                for ent in &pt.enterprises {
                    if ent.industry_tag == BuildingTag::Agriculture.as_str() {
                        let hosted = pt.enterprise_hosted_count(ent.id);
                        let pool = pools.get(&ent.hq_sector).copied().unwrap_or(0);
                        if hosted < ent.hosting_capacity && pool >= price {
                            chosen_owner = Some((
                                BuildingOwner::Enterprise {
                                    enterprise_id: ent.id,
                                },
                                ent.hq_sector,
                                None,
                            ));
                            break;
                        }
                    }
                }
            }
            if chosen_owner.is_some() {
                break;
            }

            let mut candidate_sectors: Vec<(SectorId, i64)> =
                pools.iter().map(|(&k, &v)| (k, v)).collect();
            candidate_sectors.sort_by(|a, b| {
                let a_local = Some(a.0) == local_sector_opt;
                let b_local = Some(b.0) == local_sector_opt;
                b_local
                    .cmp(&a_local)
                    .then_with(|| b.1.cmp(&a.1))
                    .then_with(|| a.0.cmp(&b.0))
            });

            for (cand_sec, pool) in candidate_sectors {
                if pool < price {
                    continue;
                }
                let (cbd_count, hosted, cap, _) = pt.sector_cbd_hosting_stats(cand_sec);
                if hosted < cap {
                    chosen_owner = Some((
                        BuildingOwner::LocalPrivate {
                            home_sector: cand_sec,
                        },
                        cand_sec,
                        None,
                    ));
                    break;
                }
                if cbd_count == 0 {
                    let auto_cbd_slot = pt
                        .tiles
                        .iter()
                        .filter(|t| t.sector_id == cand_sec.0 && t.tags.has(TileTag::Land))
                        .find_map(|t| {
                            let is_hub = t
                                .slots
                                .primary
                                .map(|ptid| {
                                    let pdef = BuildingDef::by_type_id(ptid);
                                    pdef.tags.has(BuildingTag::RegionalCore)
                                        || pdef.tags.has(BuildingTag::CorporateCampus)
                                })
                                .unwrap_or(false);
                            if !is_hub {
                                return None;
                            }
                            [
                                TileSlotKey::Secondary0,
                                TileSlotKey::Secondary1,
                                TileSlotKey::Secondary2,
                            ]
                            .into_iter()
                            .find(|&sk| {
                                t.slots.get(sk).is_none()
                                    && !(usize::from(t.index) == tile_index && sk == slot)
                            })
                            .map(|sk| (usize::from(t.index), sk))
                        });
                    if let Some(cbd_pos) = auto_cbd_slot {
                        chosen_owner = Some((
                            BuildingOwner::LocalPrivate {
                                home_sector: cand_sec,
                            },
                            cand_sec,
                            Some(cbd_pos),
                        ));
                        break;
                    }
                }
            }
            break;
        }

        let Some((new_owner, buyer_sec, auto_cbd)) = chosen_owner else {
            return Err(format!(
                "挂牌私有化未成交：需民间投资池至少 {price} 资金，且买方辖区拥有空闲的「商业街」（6座上限）或「企业园区」（18座上限）托管名额。"
            ));
        };

        let cbd_tid = BuildingDef::by_id(BUILDING_COMMERCIAL_STREET)?.type_id;
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            if let Some((cbd_tile, cbd_slot)) = auto_cbd {
                pt.tiles[cbd_tile].slots.set(cbd_slot, Some(cbd_tid));
                pt.set_slot_owner(
                    cbd_tile,
                    cbd_slot,
                    BuildingOwner::LocalPrivate {
                        home_sector: buyer_sec,
                    },
                );
            }
            pt.set_slot_owner(tile_index, slot, new_owner);
            break;
        }

        self.migrate_slot_contracts_owners(planet_id, &[(tile_index, slot, new_owner)]);
        self.adjust_cash(planet_id, CashSink::Capital(buyer_sec), -price * CASH_SCALE);
        self.adjust_cash(planet_id, CashSink::Polity(ruling_polity), price * CASH_SCALE);

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn nationalize_slot(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (ruling_polity, prev_owner_sector, buyout_cost) = {
            let mut info = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该槽位当前没有建筑。".to_string());
                };
                let def = BuildingDef::by_type_id(tid);
                if !def.is_privatizable_industry() {
                    return Err(format!("「{}」不支持赎买国有化操作。", def.name));
                }
                let owner_sec = match pt.slot_owner(tile_index, slot) {
                    BuildingOwner::LocalPrivate { home_sector } => home_sector,
                    BuildingOwner::Enterprise { enterprise_id } => pt
                        .enterprises
                        .get(usize::from(enterprise_id.0))
                        .map(|e| e.hq_sector)
                        .unwrap_or(SectorId(0)),
                    _ => return Err("该建筑已是国有产权。".to_string()),
                };
                info = Some((planet.ruling_polity, owner_sec, def.slot_build_cost(slot)));
                break;
            }
            info.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let treasury_avail = self.polity_cash_cents(ruling_polity) / CASH_SCALE;
        if treasury_avail < buyout_cost {
            return Err(format!(
                "国库资金不足：赎买国有化需 {buyout_cost} 资金（当前国库 {treasury_avail}）。"
            ));
        }

        let state_owner = BuildingOwner::State {
            polity_id: ruling_polity,
        };
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                pt.set_slot_owner(tile_index, slot, state_owner);
                break;
            }
        }
        self.migrate_slot_contracts_owners(planet_id, &[(tile_index, slot, state_owner)]);
        self.adjust_cash(planet_id, CashSink::Polity(ruling_polity), -buyout_cost * CASH_SCALE);
        self.adjust_cash(planet_id, CashSink::Capital(prev_owner_sector), buyout_cost / 2 * CASH_SCALE);
        let div_recipients = self.pop_catalog.dividend_recipients();
        for (_, (sec, cap, pops)) in
            self.world
                .query_mut::<(&Sector, &mut SectorCapital, &mut SectorPops)>()
        {
            if sec.planet_id == planet_id && sec.id == prev_owner_sector {
                let _ = cap;
                for g in &mut pops.groups {
                    if div_recipients.contains(&g.pop_type) {
                        g.satisfaction = g.satisfaction.saturating_sub(6);
                    }
                }
                break;
            }
        }

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn toggle_private_land_use(
        &mut self,
        planet_id: &str,
        sector_id: u16,
        allow: bool,
    ) -> Result<PlanetSnapshot, String> {
        let mut updated = false;
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == SectorId(sector_id) {
                sec.allow_private_land_use = allow;
                updated = true;
                break;
            }
        }
        if !updated {
            return Err(format!("找不到辖区编号：{sector_id}"));
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
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

    /// Purely data-driven baseline private investment accounting guard (zero hardcoded building IDs!):
    /// Evaluates every privatizable industry in `BUILTIN_BUILDINGS` against local commodity `MA`,
    /// local `RoleWageBook` `MA`, unsold inventory overhang, available seekers, and a strict 6-month
    /// HQ payroll reserve so private capital never builds into a glut or starves `商业街` / `企业园区`.
    fn run_autonomous_private_investment(
        &mut self,
        planet_id: &str,
        sec_allow_land: &BTreeMap<SectorId, bool>,
    ) {
        struct SectorEconSnap {
            pool_cents: i64,
            apple_ma_cents: i64,
            unsold_asks: i64,
            apple_demand: i64,
            unowned_seekers: u32,
            wage_ma_by_role: Vec<i64>,
        }

        let type_count = self.pop_catalog.len();
        let mut sec_snaps: BTreeMap<SectorId, SectorEconSnap> = BTreeMap::new();
        for (_, (sec, market, cap, pops)) in self
            .world
            .query::<(&Sector, &SectorMarket, &SectorCapital, &SectorPops)>()
            .iter()
        {
            if sec.planet_id != planet_id {
                continue;
            }
            let unsold_asks: i64 = market
                .closing_depth
                .as_ref()
                .map(|ob| ob.asks.iter().map(|l| l.qty).sum())
                .unwrap_or_else(|| market.apple_book.asks.iter().map(|o| o.remaining_qty).sum());
            let pop_total = pops.total_population();
            let unowned_seekers: u32 = pops
                .groups
                .iter()
                .filter(|g| {
                    g.employer == BuildingOwner::Unowned
                        && !self.pop_catalog.get(g.pop_type).receives_dividends
                })
                .map(|g| g.count)
                .sum();
            let wage_ma_by_role: Vec<i64> = (0..type_count)
                .map(|i| {
                    market
                        .wage_books
                        .get(i)
                        .map(|wb| wb.ma_wage_cents)
                        .unwrap_or(self.pop_catalog.defs()[i].base_wage_per_100 * CASH_SCALE / 100)
                })
                .collect();
            sec_snaps.insert(
                sec.id,
                SectorEconSnap {
                    pool_cents: cap.pool_cents(&self.goods),
                    apple_ma_cents: market.apple_book.ma_price_cents,
                    unsold_asks,
                    apple_demand: economy::compute_apple_demand(pop_total).max(1),
                    unowned_seekers,
                    wage_ma_by_role,
                },
            );
        }

        let mut investor_order: Vec<(SectorId, i64)> = sec_snaps
            .iter()
            .map(|(&sid, s)| (sid, s.pool_cents))
            .collect();
        investor_order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let mut spent_cents_by_sector: Vec<(SectorId, i64)> = Vec::new();

        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            // Compute monthly HQ payroll commitments (`商业街` + `企业园区`) per funding sector.
            let mut hq_monthly_payroll_cents: BTreeMap<SectorId, i64> = BTreeMap::new();
            for tile in &pt.tiles {
                if tile.sector_id == u16::MAX {
                    continue;
                }
                let t_idx = usize::from(tile.index);
                for (sk, tid) in tile.slots.iter_occupied() {
                    let bdef = BuildingDef::by_type_id(tid);
                    if pt.workshops[t_idx][sk.as_index()].is_none()
                        && !bdef.is_state_core_or_public()
                    {
                        let pay_sec = match pt.slot_owner(t_idx, sk) {
                            BuildingOwner::Enterprise { enterprise_id } => pt
                                .enterprises
                                .get(usize::from(enterprise_id.0))
                                .map(|e| e.hq_sector)
                                .unwrap_or(SectorId(tile.sector_id)),
                            _ => SectorId(tile.sector_id),
                        };
                        let bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .map(|c| c.monthly_bill_cents)
                            .sum();
                        *hq_monthly_payroll_cents.entry(pay_sec).or_insert(0) += bill;
                    }
                }
            }

            for &(inv_sec, pool_cents) in &investor_order {
                let hq_reserve_cents = hq_monthly_payroll_cents
                    .get(&inv_sec)
                    .copied()
                    .unwrap_or(0)
                    * 6
                    + 60 * CASH_SCALE;

                let mut best_project: Option<(
                    &'static BuildingDef,
                    BuildingOwner,
                    usize,
                    TileSlotKey,
                    i64,
                    i64,
                )> = None;

                for def in BUILTIN_BUILDINGS
                    .iter()
                    .filter(|d| d.is_privatizable_industry() && !d.monthly_output.is_empty())
                {
                    let mut owner_choice: Option<(BuildingOwner, i64)> = None;
                    for ent in &pt.enterprises {
                        if ent.hq_sector == inv_sec
                            && def
                                .tags
                                .iter()
                                .any(|tag| tag.as_str() == ent.industry_tag)
                            && pt.enterprise_hosted_count(ent.id) < ent.hosting_capacity
                        {
                            owner_choice = Some((
                                BuildingOwner::Enterprise {
                                    enterprise_id: ent.id,
                                },
                                ENTERPRISE_OUTPUT_BONUS_PERMILLE,
                            ));
                            break;
                        }
                    }
                    if owner_choice.is_none() {
                        let (_, hosted, cap, _) = pt.sector_cbd_hosting_stats(inv_sec);
                        if hosted < cap {
                            owner_choice = Some((
                                BuildingOwner::LocalPrivate {
                                    home_sector: inv_sec,
                                },
                                0,
                            ));
                        }
                    }
                    let Some((owner_tag, bonus_permille)) = owner_choice else {
                        continue;
                    };

                    for t in &pt.tiles {
                        if t.sector_id == u16::MAX
                            || !def.matches_tags(t.tags)
                            || t.slots.primary.is_none()
                            || !sec_allow_land
                                .get(&SectorId(t.sector_id))
                                .copied()
                                .unwrap_or(false)
                        {
                            continue;
                        }
                        let Some(sk) = [
                            TileSlotKey::Secondary0,
                            TileSlotKey::Secondary1,
                            TileSlotKey::Secondary2,
                        ]
                        .into_iter()
                        .find(|&k| t.slots.get(k).is_none()) else {
                            continue;
                        };
                        let build_cost_cents = def.slot_build_cost(sk) * CASH_SCALE;
                        if pool_cents < build_cost_cents + hq_reserve_cents {
                            continue;
                        }
                        let t_sec = SectorId(t.sector_id);
                        let Some(ts) = sec_snaps.get(&t_sec) else {
                            continue;
                        };
                        // Refuse to expand capacity into a glutted local market!
                        if ts.unsold_asks * 2 > ts.apple_demand * 3 {
                            continue;
                        }
                        let req_jobs = def.slot_jobs_required(sk);
                        if ts.unowned_seekers < (req_jobs / 2).max(1) {
                            continue;
                        }
                        // Generic accounting ROI check across `monthly_output` and `jobs_by_role`:
                        let scale = def.scale_for_slot(sk);
                        let mut est_rev_cents = 0i64;
                        for &(good_str, base_out) in def.monthly_output {
                            let unit_px = if good_str == GOOD_APPLE {
                                ts.apple_ma_cents
                            } else {
                                0
                            };
                            let out = ((base_out * i64::from(scale)) * (1000 + bonus_permille)
                                + 500)
                                / 1000;
                            est_rev_cents += out * unit_px;
                        }
                        let mut est_wage_cents = 0i64;
                        for &(role_str, base_req) in def.jobs_by_role {
                            let rid = def.resolve_role_id(&self.pop_catalog, role_str);
                            let w = ts
                                .wage_ma_by_role
                                .get(usize::from(rid.0))
                                .copied()
                                .unwrap_or(0);
                            est_wage_cents += i64::from(base_req * scale) * w;
                        }
                        let net_margin_cents = est_rev_cents - est_wage_cents;
                        // Require positive margin & static payback <= 20 months!
                        if net_margin_cents <= 0 || net_margin_cents * 20 < build_cost_cents {
                            continue;
                        }
                        let same_sec_bonus = if t_sec == inv_sec { 200i64 } else { 0 };
                        let score = net_margin_cents
                            + i64::from(t.tags.habitability_permille()) / 4
                            + same_sec_bonus;
                        if best_project.as_ref().map_or(true, |b| score > b.5) {
                            best_project = Some((
                                def,
                                owner_tag,
                                usize::from(t.index),
                                sk,
                                build_cost_cents,
                                score,
                            ));
                        }
                    }
                }

                if let Some((def, owner_tag, target_tile, target_sk, build_cost_cents, _)) =
                    best_project
                {
                    pt.tiles[target_tile]
                        .slots
                        .set(target_sk, Some(def.type_id));
                    pt.set_slot_owner(target_tile, target_sk, owner_tag);
                    pt.workshops[target_tile][target_sk.as_index()] =
                        make_workshop(&self.goods, &self.pop_catalog, def, target_sk);
                    spent_cents_by_sector.push((inv_sec, build_cost_cents));
                    break;
                }
            }
            break;
        }

        for (sid, cost_cents) in spent_cents_by_sector {
            self.adjust_cash(planet_id, CashSink::Capital(sid), -cost_cents);
        }
    }

    pub fn time(&self) -> TimeSnapshot {
        let elapsed_days = self.elapsed_subticks / SUBTICKS_PER_DAY;
        let subtick = (self.elapsed_subticks % SUBTICKS_PER_DAY) as u8;
        let elapsed_years = (elapsed_days / DAYS_PER_YEAR) as i32;
        let day_of_year = elapsed_days % DAYS_PER_YEAR;
        let month = ((day_of_year / DAYS_PER_MONTH) + 1) as u8;
        let day = ((day_of_year % DAYS_PER_MONTH) + 1) as u8;

        let raw_year = self.calendar.start_year + elapsed_years;
        let year = if self.calendar.start_year < 0 && raw_year >= 0 {
            raw_year + 1
        } else {
            raw_year
        };

        let formatted = if year < 0 {
            format!("前{:04}.{:02}.{:02}", -year, month, day)
        } else {
            format!("{:04}.{:02}.{:02}", year, month, day)
        };

        TimeSnapshot {
            epoch_name: self.calendar.epoch_name.clone(),
            year,
            month,
            day,
            subtick,
            formatted,
            elapsed_days,
            speed: self.speed,
            active_combats: self.active_combat_count(),
        }
    }

    pub fn default_planet_id(&self) -> Option<String> {
        self.world
            .query::<&Planet>()
            .iter()
            .map(|(_, planet)| planet.id.as_str())
            .min()
            .map(str::to_owned)
    }

    pub fn planet_surface_by_id(&self, target_id: &str) -> Option<PlanetSurfaceSnapshot> {
        self.world
            .query::<(&Planet, &Appearance)>()
            .iter()
            .find_map(|(_, (planet, appearance))| {
                (planet.id == target_id)
                    .then(|| surface::generate_planet_surface(&planet.id, appearance.seed))
            })
    }

    pub fn polities(&self) -> Vec<PolitySnapshot> {
        let mut list: Vec<PolitySnapshot> = self
            .world
            .query::<&Polity>()
            .iter()
            .map(|(_, p)| p.to_snapshot(&self.goods))
            .collect();
        list.sort_by_key(|p| p.id);
        list
    }

    pub fn planet_by_id(&self, target_id: &str) -> Option<PlanetSnapshot> {
        self.world
            .query::<(&Planet, &Position2d, &Appearance, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (planet, position, appearance, planet_tiles))| {
                (planet.id == target_id).then(|| {
                    build_planet_snapshot(
                        &self.world,
                        &self.pop_catalog,
                        &self.goods,
                        planet,
                        position,
                        appearance,
                        planet_tiles,
                    )
                })
            })
    }

    pub fn planets(&self) -> Vec<PlanetSnapshot> {
        let mut planets: Vec<_> = self
            .world
            .query::<(&Planet, &Position2d, &Appearance, &PlanetTiles)>()
            .iter()
            .map(|(_, (planet, position, appearance, planet_tiles))| {
                build_planet_snapshot(
                    &self.world,
                    &self.pop_catalog,
                    &self.goods,
                    planet,
                    position,
                    appearance,
                    planet_tiles,
                )
            })
            .collect();
        planets.sort_by(|a, b| a.id.cmp(&b.id));
        planets
    }

    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot {
            time: self.time(),
            polities: self.polities(),
            planets: self.planets(),
        }
    }
}

fn employer_kind_and_label(
    employer: BuildingOwner,
    current_sector: SectorId,
    sector_names: &BTreeMap<u16, String>,
    enterprises: &[Enterprise],
) -> (String, String) {
    match employer {
        BuildingOwner::Unowned => ("unowned".into(), String::new()),
        BuildingOwner::State { .. } => ("state".into(), "国有".into()),
        BuildingOwner::LocalPrivate { home_sector } => {
            if home_sector == current_sector {
                ("local_private".into(), "民间私产".into())
            } else {
                let short = sector_names
                    .get(&home_sector.0)
                    .and_then(|s| s.split(" · ").next())
                    .unwrap_or("外区");
                ("local_private".into(), format!("{short}区私产"))
            }
        }
        BuildingOwner::Enterprise { enterprise_id } => {
            let ent_name = enterprises
                .get(usize::from(enterprise_id.0))
                .map(|e| e.name.as_str())
                .unwrap_or("本土企业");
            ("enterprise".into(), ent_name.to_string())
        }
        BuildingOwner::Trust { trust_id } => {
            ("trust".into(), format!("托拉斯#{}", trust_id.0 + 1))
        }
    }
}

fn describe_slot_owner(
    owner: BuildingOwner,
    def: &BuildingDef,
    slot: TileSlotKey,
    sector_names: &BTreeMap<u16, String>,
    enterprises: &[Enterprise],
) -> BuildingOwnerSnapshot {
    let can_priv = def.is_privatizable_industry() && matches!(owner, BuildingOwner::State { .. });
    let can_nat = def.is_privatizable_industry()
        && matches!(
            owner,
            BuildingOwner::LocalPrivate { .. } | BuildingOwner::Enterprise { .. }
        );
    let priv_price = (def.slot_build_cost(slot) * 80) / 100;
    let nat_cost = def.slot_build_cost(slot);

    match owner {
        BuildingOwner::State { .. } | BuildingOwner::Unowned => BuildingOwnerSnapshot {
            kind: "state".into(),
            label: "国有".into(),
            home_sector_id: None,
            enterprise_id: None,
            can_privatize: can_priv,
            privatize_price: priv_price,
            can_nationalize: false,
            nationalize_cost: 0,
        },
        BuildingOwner::LocalPrivate { home_sector } => {
            let short = sector_names
                .get(&home_sector.0)
                .and_then(|s| s.split(" · ").next())
                .unwrap_or("本地");
            BuildingOwnerSnapshot {
                kind: "local_private".into(),
                label: format!("民间私产（{short}区商业街）"),
                home_sector_id: Some(home_sector.0),
                enterprise_id: None,
                can_privatize: false,
                privatize_price: 0,
                can_nationalize: can_nat,
                nationalize_cost: nat_cost,
            }
        }
        BuildingOwner::Enterprise { enterprise_id } => {
            let ent_name = enterprises
                .get(usize::from(enterprise_id.0))
                .map(|e| e.name.as_str())
                .unwrap_or("本土企业");
            BuildingOwnerSnapshot {
                kind: "enterprise".into(),
                label: ent_name.to_string(),
                home_sector_id: enterprises
                    .get(usize::from(enterprise_id.0))
                    .map(|e| e.hq_sector.0),
                enterprise_id: Some(enterprise_id.0),
                can_privatize: false,
                privatize_price: 0,
                can_nationalize: can_nat,
                nationalize_cost: nat_cost,
            }
        }
        BuildingOwner::Trust { trust_id } => BuildingOwnerSnapshot {
            kind: "trust".into(),
            label: format!("星际托拉斯 #{}", trust_id.0 + 1),
            home_sector_id: None,
            enterprise_id: None,
            can_privatize: false,
            privatize_price: 0,
            can_nationalize: true,
            nationalize_cost: nat_cost,
        },
    }
}

fn build_planet_snapshot(
    world: &World,
    pop_catalog: &PopTypeCatalog,
    goods: &GoodCatalog,
    planet: &Planet,
    position: &Position2d,
    appearance: &Appearance,
    planet_tiles: &PlanetTiles,
) -> PlanetSnapshot {
    let polity_snap = world
        .query::<&Polity>()
        .iter()
        .find_map(|(_, pol)| (pol.id == planet.ruling_polity).then(|| pol.to_snapshot(goods)))
        .unwrap_or_else(|| Polity::earth_default(goods).to_snapshot(goods));

    let mut raw_sectors: Vec<(Sector, SectorMarket, SectorCapital, SectorPops)> = world
        .query::<(&Sector, &SectorMarket, &SectorCapital, &SectorPops)>()
        .iter()
        .filter(|(_, (s, _, _, _))| s.planet_id == planet.id)
        .map(|(_, (s, m, c, p))| (s.clone(), m.clone(), c.clone(), p.clone()))
        .collect();
    raw_sectors.sort_by_key(|(s, _, _, _)| s.id);

    let sec_views: Vec<SectorSimView> = raw_sectors
        .iter()
        .map(|(s, _, _, p)| SectorSimView {
            sector_id: s.id,
            pops: p.clone(),
        })
        .collect();

    let eval = evaluate_planet_economy(pop_catalog, planet_tiles, &sec_views);

    let mut sector_names = BTreeMap::new();
    for (s, _, _, _) in &raw_sectors {
        sector_names.insert(s.id.0, s.name.clone());
    }

    // Physical goods held anywhere on the planet (workshops, state depots, Pop pantries).
    // `monthly_delta` = last month's production - consumption (spoilage excluded).
    let ruling_vault = world
        .query::<&Polity>()
        .iter()
        .find_map(|(_, pol)| (pol.id == planet.ruling_polity).then(|| pol.vault.clone()));
    let mut stockpile: BTreeMap<String, i64> = BTreeMap::new();
    let mut resources = Vec::new();
    for (g_i, gdef) in goods.defs().iter().enumerate() {
        if gdef.is_currency {
            continue;
        }
        let gid = GoodId(g_i as u16);
        let mut amount = ruling_vault.as_ref().map(|v| v.total(gid)).unwrap_or(0);
        for row in &planet_tiles.workshops {
            for ws in row.iter().flatten() {
                amount += ws.inv.total(gid);
            }
        }
        for (s, _, c, p) in &raw_sectors {
            amount += s.state_depot.total(gid) + c.vault.total(gid);
            amount += p.groups.iter().map(|g| g.inv.total(gid)).sum::<i64>();
        }
        let monthly_delta = if gdef.id == GOOD_APPLE {
            raw_sectors
                .iter()
                .map(|(_, m, _, _)| m.last_production + m.last_subsistence_output - m.last_consumption)
                .sum()
        } else {
            0
        };
        stockpile.insert(gdef.id.clone(), amount);
        resources.push(ResourceStockSnapshot {
            id: gdef.id.clone(),
            name: gdef.name.clone(),
            amount,
            monthly_delta,
        });
    }
    let apple_id = goods.id_of(GOOD_APPLE).unwrap_or(GoodId(1));

    let pop_catalog_snap = pop_catalog
        .defs()
        .iter()
        .map(PopTypeDef::to_snapshot)
        .collect();
    let building_catalog = BUILTIN_BUILDINGS.iter().map(BuildingDef::to_snapshot).collect();

    let mut total_population = 0u32;
    let mut monthly_population_growth = 0u32;
    let mut total_jobs_required = 0u32;
    let mut total_jobs_filled = 0u32;

    let default_pool_id = pop_catalog.default_pool_id();
    let vag_pool_id = pop_catalog.vagrant_pool_id();
    let has_vag_pool = pop_catalog.has_dedicated_vagrant_pool();
    let mut sectors_snap: Vec<SectorSnapshot> = Vec::with_capacity(raw_sectors.len());
    let mut region_sector_map: BTreeMap<u16, usize> = BTreeMap::new();

    for (s_idx, (sec, market, cap, pops)) in raw_sectors.iter().enumerate() {
        let stats = &eval.sector_stats[s_idx];
        let growth = pops.monthly_growth(sec.habitability_sum_permille, stats.has_core);
        let pop_total = pops.total_population();

        total_population += pop_total;
        monthly_population_growth += growth;
        total_jobs_required += stats.jobs_required;
        total_jobs_filled += stats.jobs_filled;

        let (cbd_count, cbd_hosted, cbd_capacity, ag_private) =
            planet_tiles.sector_cbd_hosting_stats(sec.id);

        let mut pop_groups_snap: Vec<PopGroupSnapshot> = pops
            .groups
            .iter()
            .enumerate()
            .filter(|&(gi, g)| {
                let (c_emp, _) = eval.pop_contracted[s_idx].get(gi).copied().unwrap_or((0, 0));
                g.count > 0
                    || c_emp > 0
                    || g.inv.total(goods.cash_id()) > 0
                    || g.inv.total(apple_id) > 0
                    || (g.culture_id == 0
                        && g.employer == BuildingOwner::Unowned
                        && g.pop_type == default_pool_id)
            })
            .map(|(gi, g)| {
                let def = pop_catalog.get(g.pop_type);
                let (c_emp, c_bill) = eval.pop_contracted[s_idx].get(gi).copied().unwrap_or((0, 0));
                let employed = if def.is_default_pool {
                    g.count
                } else if def.is_vagrant_pool || g.employer == BuildingOwner::Unowned {
                    0
                } else {
                    c_emp.min(g.count)
                };
                let contract_wage_per_100_cents = if c_emp > 0 {
                    (c_bill * 100) / i64::from(c_emp)
                } else {
                    0
                };
                let (employer_kind, mut employer_label) = employer_kind_and_label(
                    g.employer,
                    sec.id,
                    &sector_names,
                    &planet_tiles.enterprises,
                );
                if g.employer == BuildingOwner::Unowned
                    && !def.is_default_pool
                    && !def.is_vagrant_pool
                {
                    employer_label = "待业".into();
                }
                let mut name = def.name.clone();
                if !employer_label.is_empty() {
                    name.push_str(" · ");
                    name.push_str(&employer_label);
                }
                if g.culture_id > 0 {
                    name.push_str(&format!("·族群{}", g.culture_id + 1));
                }
                PopGroupSnapshot {
                    pop_id: g.id.0,
                    id: def.id.clone(),
                    culture_id: g.culture_id,
                    employer_kind,
                    employer_label,
                    name,
                    count: g.count,
                    employed,
                    contract_wage_per_100_cents,
                    savings: cents_to_credits_round(g.inv.total(goods.cash_id())),
                    pantry_apples: g.inv.total(apple_id),
                    pantry_capacity: g.inv.capacity,
                    satisfaction: g.satisfaction,
                    last_income: cents_to_credits_round(g.last_income_cents),
                    food_met_percent: (g.last_food_met_permille + 5) / 10,
                }
            })
            .collect();
        let employer_order = |k: &str| match k {
            "unowned" => 0u8,
            "state" => 1,
            "local_private" => 2,
            "enterprise" => 3,
            _ => 4,
        };
        pop_groups_snap.sort_by(|a, b| {
            pop_catalog
                .id_of(&a.id)
                .cmp(&pop_catalog.id_of(&b.id))
                .then_with(|| employer_order(&a.employer_kind).cmp(&employer_order(&b.employer_kind)))
                .then_with(|| a.employer_label.cmp(&b.employer_label))
                .then_with(|| a.culture_id.cmp(&b.culture_id))
                .then_with(|| a.pop_id.cmp(&b.pop_id))
        });

        let wage_quotes: Vec<RoleWageSnapshot> = pop_catalog
            .defs()
            .iter()
            .enumerate()
            .filter(|(_, d)| d.base_wage_per_100 > 0)
            .map(|(i, d)| {
                let wb = market.wage_books.get(i);
                let last_c = wb
                    .map(|b| b.last_wage_cents)
                    .unwrap_or(d.base_wage_per_100 * CASH_SCALE / 100);
                let ma_c = wb
                    .map(|b| b.ma_wage_cents)
                    .unwrap_or(d.base_wage_per_100 * CASH_SCALE / 100);
                RoleWageSnapshot {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    wage_per_100_cents: last_c * 100,
                    ma_wage_per_100_cents: ma_c * 100,
                    last_hired: wb.map(|b| b.last_hired).unwrap_or(0),
                    last_laid_off: wb.map(|b| b.last_laid_off).unwrap_or(0),
                }
            })
            .collect();

        let vagrant_count = if has_vag_pool {
            pops.count_of(vag_pool_id)
        } else {
            0
        };
        let unemployed_count: u32 = pops
            .groups
            .iter()
            .filter(|g| {
                let d = pop_catalog.get(g.pop_type);
                g.employer == BuildingOwner::Unowned && !d.is_default_pool && !d.receives_dividends
            })
            .map(|g| g.count)
            .sum();

        // Count active solvent vacancies (excluding overstocked/bankrupt slots that voluntarily paused hiring).
        let mut active_vacancies = 0u32;
        for tile in &planet_tiles.tiles {
            if tile.sector_id != sec.id.0 {
                continue;
            }
            let t_idx = usize::from(tile.index);
            for (sk, tid) in tile.slots.iter_occupied() {
                let def = BuildingDef::by_type_id(tid);
                let req = def.slot_jobs_required(sk);
                let emp = eval.slot_employed[t_idx][sk.as_index()];
                if emp >= req {
                    continue;
                }
                let vac = req - emp;
                if let Some(ws) = planet_tiles.workshops[t_idx][sk.as_index()].as_ref() {
                    let fill_pm = (ws.inv.total(apple_id) * 1000) / ws.inv.capacity.max(1);
                    if !ws.had_wage_arrears && fill_pm < 900 && ws.inv.avail(goods.cash_id()) > 0 {
                        active_vacancies += vac;
                    }
                } else if def.is_state_core_or_public() {
                    if polity_snap.treasury_cents > 0 {
                        active_vacancies += vac;
                    }
                } else if cap.pool_cents(goods) > 0 {
                    active_vacancies += vac;
                }
            }
        }

        region_sector_map.insert(sec.geo_region_id, sectors_snap.len());
        sectors_snap.push(SectorSnapshot {
            id: sec.id.0,
            geo_region_id: sec.geo_region_id,
            owner_polity_id: sec.owner_polity.0,
            name: sec.name.clone(),
            allow_private_land_use: sec.allow_private_land_use,
            population: pop_total,
            monthly_growth: growth,
            jobs_required: stats.jobs_required,
            jobs_filled: stats.jobs_filled,
            free_labor: pop_total.saturating_sub(stats.jobs_filled),
            idle_pool_count: pops.count_of(default_pool_id),
            unemployed_count,
            vagrant_count,
            active_vacancies,
            average_satisfaction: pops.average_satisfaction(),
            apple_price: cents_to_credits_round(market.apple_book.last_price_cents),
            apple_price_cents: market.apple_book.last_price_cents,
            ma_apple_price_cents: market.apple_book.ma_price_cents,
            order_book: market
                .closing_depth
                .clone()
                .unwrap_or_else(|| market.apple_book.to_snapshot(5)),
            wage_quotes,
            depot_apples: sec.state_depot.total(apple_id),
            depot_capacity: sec.state_depot.capacity,
            depot_policy: sec.depot_policy,
            depot_policy_label: sec.depot_policy.label().to_string(),
            local_apple_stockpile: sec.state_depot.total(apple_id),
            apple_supply: market.last_production + market.last_subsistence_output,
            apple_demand: economy::compute_apple_demand(pop_total),
            net_trade_in: market.apple_book.last_net_import,
            investment_pool: cents_to_credits_round(cap.pool_cents(goods)),
            last_dividends: cents_to_credits_round(cap.last_dividends_cents),
            cbd_count,
            cbd_hosted,
            cbd_capacity,
            unincorporated_ag_private: ag_private,
            can_found_enterprise: usize::from(ag_private) >= ENTERPRISE_FOUNDING_MIN_PRIVATE,
            pops: pop_groups_snap,
        });
    }

    let enterprises_snap: Vec<EnterpriseSnapshot> = planet_tiles
        .enterprises
        .iter()
        .enumerate()
        .map(|(e_i, ent)| EnterpriseSnapshot {
            id: ent.id.0,
            name: ent.name.clone(),
            industry_tag: ent.industry_tag.clone(),
            hq_sector_id: ent.hq_sector.0,
            campus_tile_index: ent.campus_tile_index,
            hosted_count: planet_tiles.enterprise_hosted_count(ent.id),
            hosting_capacity: ent.hosting_capacity,
            output_bonus_percent: if eval.enterprise_active.get(e_i).copied().unwrap_or(false) {
                (ENTERPRISE_OUTPUT_BONUS_PERMILLE / 10) as u16
            } else {
                0
            },
            last_revenue: cents_to_credits_round(ent.last_revenue_cents),
            last_profit: cents_to_credits_round(ent.last_profit_cents),
        })
        .collect();

    let mut region_summaries: Vec<RegionSummarySnapshot> = planet_tiles
        .regions
        .iter()
        .map(|r| {
            if let Some(&sec_vec_idx) = region_sector_map.get(&r.id) {
                let s = &sectors_snap[sec_vec_idx];
                RegionSummarySnapshot {
                    region_id: r.id,
                    sector_id: Some(s.id),
                    population: s.population,
                    monthly_growth: s.monthly_growth,
                    jobs_required: s.jobs_required,
                    jobs_filled: s.jobs_filled,
                    free_labor: s.free_labor,
                    primary_used: 0,
                    primary_total: r.tile_count,
                    land_primary_used: 0,
                    land_primary_total: r.land_tile_count,
                    secondary_used: 0,
                    secondary_total: r.tile_count.saturating_mul(SECONDARY_SLOT_COUNT as u16),
                    unlocked_secondary_empty: 0,
                    core_tile_index: None,
                    core_building_name: None,
                    building_counts: BTreeMap::new(),
                }
            } else {
                RegionSummarySnapshot {
                    region_id: r.id,
                    sector_id: None,
                    population: 0,
                    monthly_growth: 0,
                    jobs_required: 0,
                    jobs_filled: 0,
                    free_labor: 0,
                    primary_used: 0,
                    primary_total: r.tile_count,
                    land_primary_used: 0,
                    land_primary_total: r.land_tile_count,
                    secondary_used: 0,
                    secondary_total: r.tile_count.saturating_mul(SECONDARY_SLOT_COUNT as u16),
                    unlocked_secondary_empty: 0,
                    core_tile_index: None,
                    core_building_name: None,
                    building_counts: BTreeMap::new(),
                }
            }
        })
        .collect();

    let mut capital_tile_index: Option<usize> = None;
    let mut total_building_count = 0usize;
    let mut occupied_tiles = Vec::new();

    for tile in &planet_tiles.tiles {
        let tile_idx = usize::from(tile.index);
        if let Some(summary) = region_summaries.get_mut(usize::from(tile.region_id)) {
            if let Some(tid) = tile.slots.primary {
                summary.primary_used += 1;
                if tile.tags.has(TileTag::Land) {
                    summary.land_primary_used += 1;
                }
                let def = BuildingDef::by_type_id(tid);
                *summary.building_counts.entry(def.id.to_string()).or_insert(0) += 1;
                if def.tags.has(BuildingTag::PlanetaryCapital) && capital_tile_index.is_none() {
                    capital_tile_index = Some(tile_idx);
                }
                if def.tags.has(BuildingTag::RegionalCore) && summary.core_tile_index.is_none() {
                    summary.core_tile_index = Some(tile_idx);
                    summary.core_building_name = Some(def.name.to_string());
                }
                for &sec_opt in &tile.slots.secondary {
                    if sec_opt.is_none() {
                        summary.unlocked_secondary_empty += 1;
                    }
                }
            }
            for &sec in &tile.slots.secondary {
                if let Some(tid) = sec {
                    summary.secondary_used += 1;
                    let def = BuildingDef::by_type_id(tid);
                    *summary.building_counts.entry(def.id.to_string()).or_insert(0) += 1;
                }
            }
        }

        if !tile.slots.is_empty() {
            total_building_count += tile.slots.used_count();
            let slot_emp = eval.slot_employed[tile_idx];
            let slot_bonus = eval.slot_bonus_permille[tile_idx];

            let build_slot_snap = |sk: TileSlotKey| -> Option<PlacedBuildingSnapshot> {
                tile.slots.get(sk).map(|tid| {
                    let def = BuildingDef::by_type_id(tid);
                    let owner = describe_slot_owner(
                        planet_tiles.slot_owner(tile_idx, sk),
                        def,
                        sk,
                        &sector_names,
                        &planet_tiles.enterprises,
                    );
                    def.to_placed_snapshot(
                        sk,
                        slot_emp[sk.as_index()],
                        slot_bonus[sk.as_index()],
                        owner,
                    )
                })
            };

            occupied_tiles.push(OccupiedTileSnapshot {
                index: tile_idx,
                region_id: tile.region_id,
                sector_id: (tile.sector_id != u16::MAX).then_some(tile.sector_id),
                tags: tile.tags,
                habitability: tile.tags.habitability_percent(),
                primary: build_slot_snap(TileSlotKey::Primary),
                secondary: [
                    build_slot_snap(TileSlotKey::Secondary0),
                    build_slot_snap(TileSlotKey::Secondary1),
                    build_slot_snap(TileSlotKey::Secondary2),
                ],
            });
        }
    }

    PlanetSnapshot {
        id: planet.id.clone(),
        name: planet.name.clone(),
        radius_km: planet.radius_km,
        position: *position,
        appearance: *appearance,
        polity: polity_snap,
        pop_catalog: pop_catalog_snap,
        stockpile,
        resources,
        building_catalog,
        tile_count: planet_tiles.tiles.len(),
        land_tile_count: planet_tiles.land_tile_count,
        sea_tile_count: planet_tiles.sea_tile_count,
        region_count: planet_tiles.regions.len(),
        starter_tile_index: planet_tiles.starter_tile_index,
        capital_tile_index,
        total_population,
        monthly_population_growth,
        total_jobs_required,
        total_jobs_filled,
        total_building_count,
        sectors: sectors_snap,
        enterprises: enterprises_snap,
        region_summaries,
        occupied_tiles,
    }
}

struct Subscriber {
    id: u64,
    tx: mpsc::SyncSender<StreamEvent>,
    watched_planet: Option<String>,
}

enum RuntimeCommand {
    SetSpeed(GameSpeed, Option<mpsc::SyncSender<Arc<GameSnapshot>>>),
    BuildOnTile {
        planet_id: String,
        tile_index: usize,
        building_id: String,
        slot: Option<TileSlotKey>,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    BuildInRegion {
        planet_id: String,
        region_id: u16,
        building_id: String,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    DemolishOnTile {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    PrivatizeSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    NationalizeSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    TogglePrivateLandUse {
        planet_id: String,
        sector_id: u16,
        allow: bool,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    SetPolicy {
        planet_id: String,
        change: PolicyChange,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    Subscribe {
        id: u64,
        watched_planet: Option<String>,
        tx: mpsc::SyncSender<StreamEvent>,
    },
    WatchPlanet {
        id: u64,
        planet_id: Option<String>,
    },
    GetPlanetSurface {
        planet_id: String,
        reply: mpsc::SyncSender<Option<PlanetSurfaceSnapshot>>,
    },
    Unsubscribe {
        id: u64,
    },
    Shutdown,
}

const STREAM_BUFFER_CAPACITY: usize = 128;

pub struct GameRuntime {
    snapshot: Arc<Mutex<Arc<GameSnapshot>>>,
    cmd_tx: mpsc::Sender<RuntimeCommand>,
    next_sub_id: AtomicU64,
    worker: Option<JoinHandle<()>>,
}

impl Default for GameRuntime {
    fn default() -> Self {
        Self::new(Game::default())
    }
}

impl GameRuntime {
    pub fn new(mut game: Game) -> Self {
        let initial = Arc::new(game.snapshot());
        let snapshot = Arc::new(Mutex::new(initial));
        let worker_snapshot = Arc::clone(&snapshot);
        let (cmd_tx, cmd_rx) = mpsc::channel::<RuntimeCommand>();

        let worker = thread::Builder::new()
            .name("space-strategy-sim".into())
            .spawn(move || {
                let mut last_step = Instant::now();
                let mut subscribers: Vec<Subscriber> = Vec::new();

                let publish_snapshot = |game: &Game| -> Arc<GameSnapshot> {
                    let next = Arc::new(game.snapshot());
                    if let Ok(mut slot) = worker_snapshot.lock() {
                        *slot = Arc::clone(&next);
                    }
                    next
                };

                let broadcast_tick = |subscribers: &mut Vec<Subscriber>, time: TimeSnapshot| {
                    subscribers.retain(|sub| {
                        match sub.tx.try_send(StreamEvent::Tick { time: time.clone() }) {
                            Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
                            Err(mpsc::TrySendError::Disconnected(_)) => false,
                        }
                    });
                };

                let broadcast_watched_planets =
                    |subscribers: &mut Vec<Subscriber>, game: &Game, filter_id: Option<&str>| {
                        subscribers.retain(|sub| {
                            let Some(watched_id) = sub.watched_planet.as_deref() else {
                                return true;
                            };
                            if let Some(only_id) = filter_id {
                                if watched_id != only_id {
                                    return true;
                                }
                            }
                            let event = StreamEvent::PlanetView {
                                planet: game.planet_by_id(watched_id),
                            };
                            match sub.tx.try_send(event) {
                                Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
                                Err(mpsc::TrySendError::Disconnected(_)) => false,
                            }
                        });
                    };

                let notify_mutation_outcome =
                    |result: &Result<PlanetSnapshot, String>,
                     planet_id: &str,
                     origin_sub_id: Option<u64>,
                     game: &Game,
                     subscribers: &mut Vec<Subscriber>| {
                        match result {
                            Ok(_) => {
                                publish_snapshot(game);
                                broadcast_watched_planets(subscribers, game, Some(planet_id));
                            }
                            Err(err) => {
                                if let Some(target_sub) = origin_sub_id {
                                    for sub in subscribers.iter() {
                                        if sub.id == target_sub {
                                            let _ = sub.tx.try_send(StreamEvent::CommandError {
                                                message: err.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    };

                let handle_cmd = |cmd: RuntimeCommand,
                                  game: &mut Game,
                                  last_step: &mut Instant,
                                  subscribers: &mut Vec<Subscriber>|
                 -> bool {
                    match cmd {
                        RuntimeCommand::SetSpeed(next_speed, reply) => {
                            let was_paused = game.speed() == GameSpeed::Paused;
                            game.set_speed(next_speed);
                            if was_paused && next_speed != GameSpeed::Paused {
                                *last_step = Instant::now();
                            }
                            let snap = publish_snapshot(game);
                            broadcast_tick(subscribers, snap.time.clone());
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(snap);
                            }
                            true
                        }
                        RuntimeCommand::BuildOnTile {
                            planet_id,
                            tile_index,
                            building_id,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result =
                                game.build_on_tile(&planet_id, tile_index, &building_id, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::BuildInRegion {
                            planet_id,
                            region_id,
                            building_id,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.build_in_region(&planet_id, region_id, &building_id);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::DemolishOnTile {
                            planet_id,
                            tile_index,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.demolish_on_tile(&planet_id, tile_index, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::PrivatizeSlot {
                            planet_id,
                            tile_index,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.privatize_slot(&planet_id, tile_index, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::NationalizeSlot {
                            planet_id,
                            tile_index,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.nationalize_slot(&planet_id, tile_index, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::TogglePrivateLandUse {
                            planet_id,
                            sector_id,
                            allow,
                            origin_sub_id,
                            reply,
                        } => {
                            let result =
                                game.toggle_private_land_use(&planet_id, sector_id, allow);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::SetPolicy {
                            planet_id,
                            change,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = match change {
                                PolicyChange::Monetary(p) => game.set_monetary_policy(&planet_id, p),
                                PolicyChange::Depot { sector_id, policy } => {
                                    game.set_depot_policy(&planet_id, sector_id, policy)
                                }
                            };
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::Subscribe {
                            id,
                            watched_planet,
                            tx,
                        } => {
                            let default_planet_id = game.default_planet_id();
                            let target = watched_planet.or_else(|| default_planet_id.clone());
                            let init = StreamEvent::Init {
                                time: game.time(),
                                default_planet_id,
                            };
                            if tx.try_send(init).is_ok() {
                                let surface = target
                                    .as_deref()
                                    .and_then(|pid| game.planet_surface_by_id(pid));
                                let planet =
                                    target.as_deref().and_then(|pid| game.planet_by_id(pid));
                                if tx.try_send(StreamEvent::PlanetSurface { surface }).is_ok()
                                    && tx.try_send(StreamEvent::PlanetView { planet }).is_ok()
                                {
                                    subscribers.push(Subscriber {
                                        id,
                                        tx,
                                        watched_planet: target,
                                    });
                                }
                            }
                            true
                        }
                        RuntimeCommand::WatchPlanet { id, planet_id } => {
                            let surface = planet_id
                                .as_deref()
                                .and_then(|pid| game.planet_surface_by_id(pid));
                            let planet =
                                planet_id.as_deref().and_then(|pid| game.planet_by_id(pid));
                            subscribers.retain_mut(|sub| {
                                if sub.id != id {
                                    return true;
                                }
                                sub.watched_planet = planet_id.clone();
                                !matches!(
                                    sub.tx.try_send(StreamEvent::PlanetSurface {
                                        surface: surface.clone()
                                    }),
                                    Err(mpsc::TrySendError::Disconnected(_))
                                ) && !matches!(
                                    sub.tx.try_send(StreamEvent::PlanetView {
                                        planet: planet.clone()
                                    }),
                                    Err(mpsc::TrySendError::Disconnected(_))
                                )
                            });
                            true
                        }
                        RuntimeCommand::GetPlanetSurface { planet_id, reply } => {
                            let _ = reply.send(game.planet_surface_by_id(&planet_id));
                            true
                        }
                        RuntimeCommand::Unsubscribe { id } => {
                            subscribers.retain(|sub| sub.id != id);
                            true
                        }
                        RuntimeCommand::Shutdown => false,
                    }
                };

                loop {
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        if !handle_cmd(cmd, &mut game, &mut last_step, &mut subscribers) {
                            return;
                        }
                    }

                    let Some(interval) = game.next_step_interval() else {
                        match cmd_rx.recv() {
                            Ok(cmd) => {
                                if !handle_cmd(cmd, &mut game, &mut last_step, &mut subscribers) {
                                    return;
                                }
                            }
                            Err(_) => return,
                        }
                        continue;
                    };

                    let elapsed = Instant::now().saturating_duration_since(last_step);
                    if elapsed >= interval {
                        let outcome = game.step_once();
                        last_step = Instant::now();
                        let snap = publish_snapshot(&game);
                        broadcast_tick(&mut subscribers, snap.time.clone());
                        if outcome.month_advanced {
                            broadcast_watched_planets(&mut subscribers, &game, None);
                        }
                    } else {
                        let wait = interval - elapsed;
                        match cmd_rx.recv_timeout(wait) {
                            Ok(cmd) => {
                                if !handle_cmd(cmd, &mut game, &mut last_step, &mut subscribers) {
                                    return;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
            })
            .expect("启动模拟线程失败");

        Self {
            snapshot,
            cmd_tx,
            next_sub_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    pub fn snapshot(&self) -> Result<Arc<GameSnapshot>, String> {
        let slot = self
            .snapshot
            .lock()
            .map_err(|_| "游戏状态不可用，请重新启动。".to_string())?;
        Ok(Arc::clone(&slot))
    }

    pub fn set_speed(&self, speed: GameSpeed) -> Result<Arc<GameSnapshot>, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::SetSpeed(speed, Some(reply_tx)))
            .map_err(|_| "模拟线程已停止，无法修改游戏速度。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应速度变更超时。".to_string())
    }

    pub fn set_speed_async(&self, speed: GameSpeed) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::SetSpeed(speed, None))
            .map_err(|_| "模拟线程已停止，无法修改游戏速度。".to_string())
    }

    pub fn build_on_tile(
        &self,
        planet_id: String,
        tile_index: usize,
        building_id: String,
        slot: Option<TileSlotKey>,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::BuildOnTile {
                planet_id,
                tile_index,
                building_id,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法建造建筑。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应建造请求超时。".to_string())?
    }

    pub fn build_on_tile_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        building_id: String,
        slot: Option<TileSlotKey>,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::BuildOnTile {
                planet_id,
                tile_index,
                building_id,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法建造建筑。".to_string())
    }

    pub fn build_in_region(
        &self,
        planet_id: String,
        region_id: u16,
        building_id: String,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::BuildInRegion {
                planet_id,
                region_id,
                building_id,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法在大区建造建筑。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应大区建造请求超时。".to_string())?
    }

    pub fn build_in_region_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        region_id: u16,
        building_id: String,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::BuildInRegion {
                planet_id,
                region_id,
                building_id,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法在大区建造建筑。".to_string())
    }

    pub fn demolish_on_tile(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::DemolishOnTile {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法拆除建筑。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应拆除请求超时。".to_string())?
    }

    pub fn demolish_on_tile_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::DemolishOnTile {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法拆除建筑。".to_string())
    }

    pub fn privatize_slot(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::PrivatizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法执行私有化。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应私有化请求超时。".to_string())?
    }

    pub fn privatize_slot_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::PrivatizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法执行私有化。".to_string())
    }

    pub fn nationalize_slot(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::NationalizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法执行国有化。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应国有化请求超时。".to_string())?
    }

    pub fn nationalize_slot_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::NationalizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法执行国有化。".to_string())
    }

    pub fn toggle_private_land_use(
        &self,
        planet_id: String,
        sector_id: u16,
        allow: bool,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::TogglePrivateLandUse {
                planet_id,
                sector_id,
                allow,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法切换用地政策。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应用地政策变更超时。".to_string())?
    }

    pub fn set_policy(
        &self,
        planet_id: String,
        change: PolicyChange,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::SetPolicy {
                planet_id,
                change,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法调整政策。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应政策变更超时。".to_string())?
    }

    pub fn set_policy_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        change: PolicyChange,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::SetPolicy {
                planet_id,
                change,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法调整政策。".to_string())
    }

    pub fn toggle_private_land_use_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        sector_id: u16,
        allow: bool,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::TogglePrivateLandUse {
                planet_id,
                sector_id,
                allow,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法切换用地政策。".to_string())
    }

    pub fn planet_surface(&self, planet_id: String) -> Result<Option<PlanetSurfaceSnapshot>, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::GetPlanetSurface {
                planet_id,
                reply: reply_tx,
            })
            .map_err(|_| "模拟线程已停止，无法查询星球地表。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程生成地表数据超时。".to_string())
    }

    pub fn subscribe(
        &self,
        watched_planet: Option<String>,
    ) -> Result<(u64, mpsc::Receiver<StreamEvent>), String> {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(STREAM_BUFFER_CAPACITY);
        self.cmd_tx
            .send(RuntimeCommand::Subscribe {
                id,
                watched_planet,
                tx,
            })
            .map_err(|_| "模拟线程已停止，无法订阅状态流。".to_string())?;
        Ok((id, rx))
    }

    pub fn watch_planet(&self, id: u64, planet_id: Option<String>) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::WatchPlanet { id, planet_id })
            .map_err(|_| "模拟线程已停止，无法切换观察目标。".to_string())
    }

    pub fn unsubscribe(&self, id: u64) {
        let _ = self.cmd_tx.send(RuntimeCommand::Unsubscribe { id });
    }
}

impl Drop for GameRuntime {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(RuntimeCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_earth_has_polity_sectors_data_driven_pops_and_compact_tile() {
        assert!(
            std::mem::size_of::<Tile>() <= 24,
            "Tile struct should stay <= 24 bytes in ECS, got {}",
            std::mem::size_of::<Tile>()
        );

        let game = Game::default();
        let snap = game.snapshot();
        assert_eq!(snap.polities.len(), 1);
        assert_eq!(snap.polities[0].treasury, 2_600);

        let earth = &snap.planets[0];
        assert_eq!(earth.id, "base:earth");
        assert_eq!(earth.sectors.len(), surface::DEFAULT_LAND_REGIONS);
        assert_eq!(earth.pop_catalog.len(), 8);
        assert_eq!(earth.occupied_tiles.len(), 2);
        assert_eq!(earth.total_building_count, 4);
        assert_eq!(earth.total_jobs_required, 540);
        assert_eq!(earth.total_jobs_filled, 540);

        // Verify unified single-count Pop invariant: sector population == sum of PopGroup counts (no dependents split!).
        for sec in &earth.sectors {
            let sum_counts: u32 = sec.pops.iter().map(|g| g.count).sum();
            assert_eq!(sec.population, sum_counts);
        }
    }

    #[test]
    fn custom_pop_type_catalog_runs_purely_from_data() {
        let custom_defs = vec![
            PopTypeDef {
                id: "mod:freeholder".into(),
                name: "自由拓荒民".into(),
                base_wage_per_100: 0,
                subsistence_apple_per_100: 8,
                is_default_pool: true,
                is_vagrant_pool: false,
                receives_dividends: false,
                can_demote: false,
                demote_to: None,
                expected_sol: 3,
                promote_from: Vec::new(),
                capital_starter_count: 0,
                rural_share_permille: 0,
                rural_min: 100,
                rural_max: u32::MAX,
                initial_savings: 40,
            },
            PopTypeDef {
                id: "base:laborer".into(),
                name: "合成劳工".into(),
                base_wage_per_100: 50,
                subsistence_apple_per_100: 0,
                is_default_pool: false,
                is_vagrant_pool: false,
                receives_dividends: false,
                can_demote: true,
                demote_to: None,
                expected_sol: 5,
                promote_from: vec!["mod:freeholder".into()],
                capital_starter_count: 350,
                rural_share_permille: 200,
                rural_min: 100,
                rural_max: 200,
                initial_savings: 50,
            },
            PopTypeDef {
                id: "mod:guild_shareholder".into(),
                name: "行会股东".into(),
                base_wage_per_100: 0,
                subsistence_apple_per_100: 0,
                is_default_pool: false,
                is_vagrant_pool: false,
                receives_dividends: true,
                can_demote: false,
                demote_to: None,
                expected_sol: 15,
                promote_from: vec!["mod:freeholder".into()],
                capital_starter_count: 25,
                rural_share_permille: 0,
                rural_min: 0,
                rural_max: 0,
                initial_savings: 200,
            },
        ];
        let catalog = PopTypeCatalog::from_defs(custom_defs).unwrap();
        let mut game = Game::with_pop_catalog(CalendarConfig::default(), catalog);
        let earth0 = game.planet_by_id("base:earth").unwrap();
        assert_eq!(earth0.pop_catalog.len(), 3);
        assert!(!earth0.sectors[0].pops.is_empty());
        assert_eq!(earth0.sectors[0].pops[0].name, "自由拓荒民");

        for _ in 0..30 {
            game.step_once();
        }
        let earth1 = game.planet_by_id("base:earth").unwrap();
        assert!(earth1.total_population > earth0.total_population);
    }

    #[test]
    fn primary_first_rule_2x_dominant_scale_and_redevelopment_work() {
        let mut game = Game::default();
        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_region = surf.tiles[earth0.starter_tile_index].region_id;

        let other_region = surf
            .regions
            .iter()
            .find(|r| !r.is_ocean && r.id != starter_region)
            .unwrap()
            .id;
        let empty_land_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| t.region_id == other_region && t.tags.has(TileTag::Land))
                .unwrap()
                .index,
        );

        assert!(game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_APPLE_ORCHARD,
                Some(TileSlotKey::Secondary0),
            )
            .is_err());

        let treasury_before = game.planet_by_id("base:earth").unwrap().polity.treasury;
        let after_primary = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_APPLE_ORCHARD,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        assert_eq!(after_primary.polity.treasury, treasury_before - 300);

        assert!(game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_COMMERCIAL_STREET,
                Some(TileSlotKey::Secondary0),
            )
            .is_err());

        game.build_on_tile(
            "base:earth",
            empty_land_tile,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary0),
        )
        .unwrap();

        assert!(game
            .demolish_on_tile("base:earth", empty_land_tile, TileSlotKey::Primary)
            .is_err());

        let after_redev = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_REGIONAL_CORE,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        let redev_occ = after_redev
            .occupied_tiles
            .iter()
            .find(|t| t.index == empty_land_tile)
            .unwrap();
        assert_eq!(
            redev_occ.primary.as_ref().unwrap().id,
            BUILDING_REGIONAL_CORE
        );

        let with_cbd = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_COMMERCIAL_STREET,
                Some(TileSlotKey::Secondary1),
            )
            .unwrap();
        let cbd_occ = with_cbd
            .occupied_tiles
            .iter()
            .find(|t| t.index == empty_land_tile)
            .unwrap();
        assert_eq!(
            cbd_occ.secondary[1].as_ref().unwrap().id,
            BUILDING_COMMERCIAL_STREET
        );
    }

    #[test]
    fn privatization_enterprise_campus_founding_and_nationalization_work() {
        let mut game = Game::default();
        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let cap_region = surf.tiles[starter_idx].region_id;

        game.build_on_tile(
            "base:earth",
            starter_idx,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary2),
        )
        .unwrap();

        game.privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        for _ in 0..60 {
            game.step_once();
        }
        let after_priv2 = game
            .privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary2)
            .unwrap();

        let cap_sec_id = after_priv2.region_summaries[usize::from(cap_region)]
            .sector_id
            .unwrap();
        let cap_sec = after_priv2
            .sectors
            .iter()
            .find(|s| s.id == cap_sec_id)
            .unwrap();
        assert!(cap_sec.can_found_enterprise);

        let occupied_set: std::collections::HashSet<usize> = after_priv2
            .occupied_tiles
            .iter()
            .map(|t| t.index)
            .collect();
        let campus_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| {
                    t.region_id == cap_region
                        && t.tags.has(TileTag::Land)
                        && !occupied_set.contains(&usize::from(t.index))
                })
                .unwrap()
                .index,
        );

        let after_campus = game
            .build_on_tile(
                "base:earth",
                campus_tile,
                BUILDING_CORPORATE_CAMPUS,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        assert_eq!(after_campus.enterprises.len(), 1);
        assert!(after_campus.enterprises[0].hosted_count >= 3);

        let after_nat = game
            .nationalize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        let starter_occ = after_nat
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap();
        assert_eq!(starter_occ.secondary[0].as_ref().unwrap().owner.kind, "state");
    }

    #[test]
    fn monthly_cycle_and_calendar_work() {
        let mut game = Game::with_calendar(CalendarConfig::new("纪元", -1).unwrap());
        assert_eq!(game.time().formatted, "前0001.01.01");
        let initial_pop = game.planet_by_id("base:earth").unwrap().total_population;
        for _ in 0..360 {
            game.step_once();
        }
        assert_eq!(game.time().year, 1);
        assert_eq!(game.time().formatted, "0001.01.01");
        let after_year = game.planet_by_id("base:earth").unwrap();
        assert!(after_year.total_population > initial_pop);
        assert!(after_year.stockpile[GOOD_APPLE] > 0);
    }

    #[test]
    fn monthly_settlement_conserves_cash_except_minting() {
        let mut game = Game::default();
        let ruling = PolityId(0);
        for month in 0..6 {
            let before = game.planet_money_supply_cents("base:earth");
            game.settle_planet_month("base:earth", ruling);
            let after = game.planet_money_supply_cents("base:earth");
            let minted = game
                .world
                .query::<&Polity>()
                .iter()
                .map(|(_, p)| p.last_seigniorage_cents)
                .sum::<i64>();
            assert_eq!(after, before + minted, "cash leaked in month {month}");
        }
        // No inventory may hold locked balances between settlements.
        for (_, (_, pops)) in game.world.query::<(&Sector, &SectorPops)>().iter() {
            for g in &pops.groups {
                assert!(g.inv.locked.iter().all(|&x| x == 0));
            }
        }
        let earth = game.planet_by_id("base:earth").unwrap();
        let traded: i64 = earth
            .sectors
            .iter()
            .map(|s| s.order_book.last_contract_volume + s.order_book.last_spot_volume)
            .sum();
        assert!(traded > 0, "apples should change hands on the order books");
    }

    #[test]
    fn multi_culture_pops_promotion_carries_savings_and_full_warehouse_clears() {
        let mut game = Game::default();
        let goods = game.goods().clone();
        let pop_cat = game.pop_catalog().clone();
        let cash_id = goods.cash_id();
        let apple_id = goods.id_of(GOOD_APPLE).unwrap();
        let sub_id = pop_cat.id_of(POP_SUBSISTENCE).unwrap();
        let lab_id = pop_cat.id_of(POP_LABORER).unwrap();

        // 1. Verify promotion carries proportional cash & food and preserves non-zero `culture_id`!
        let mut pops = SectorPops {
            groups: vec![PopGroup::with_identity(
                &goods,
                PopId(1),
                sub_id,
                2,
                BuildingOwner::Unowned,
                100,
                200,
                10,
            )],
            next_pop_id: 2,
            last_growth: 0,
        };
        let total_cash_before = pops.groups[0].inv.total(cash_id);
        let total_food_before = pops.groups[0].inv.total(apple_id);
        let moved = pops.promote_into(&goods, &pop_cat, lab_id, 40);
        assert_eq!(moved, 40);
        assert_eq!(pops.groups.len(), 2, "must spawn `(laborer, culture=2)` row dynamically");
        assert_eq!(pops.groups[1].pop_type, lab_id);
        assert_eq!(pops.groups[1].culture_id, 2);
        assert_eq!(pops.groups[0].count, 60);
        assert_eq!(pops.groups[1].count, 40);
        assert_eq!(
            pops.groups[1].inv.total(cash_id),
            (total_cash_before * 40) / 100,
            "promoted workers must carry 40% of savings"
        );
        assert_eq!(
            pops.groups[0].inv.total(cash_id) + pops.groups[1].inv.total(cash_id),
            total_cash_before
        );
        assert_eq!(
            pops.groups[0].inv.total(apple_id) + pops.groups[1].inv.total(apple_id),
            total_food_before
        );

        // 2. Inject a second culture row (`culture_id = 1`) into Capital Sector on Earth and verify
        //    both cultures coexist, get hired via Labor Order Book, and conserve cash across settlements!
        let cap_sec_u16 = game
            .planet_by_id("base:earth")
            .unwrap()
            .occupied_tiles[0]
            .sector_id
            .unwrap();
        for (_, (sec, spops)) in game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == SectorId(cap_sec_u16) {
                spops.push_group_with_culture(
                    &goods,
                    lab_id,
                    1,
                    BuildingOwner::Unowned,
                    80,
                    120,
                    4,
                );
                break;
            }
        }
        let snap_multi = game.planet_by_id("base:earth").unwrap();
        let cap_sec_snap = snap_multi
            .sectors
            .iter()
            .find(|s| s.id == cap_sec_u16)
            .unwrap();
        assert!(
            cap_sec_snap.pops.len() >= 10,
            "capital sector with employer-partitioned Pops + 2 cultures must expose >= 10 rows, got {}",
            cap_sec_snap.pops.len()
        );

        // 3. Verify newly built orchards receive 1 month of starter working capital (`60.00 CR`),
        //    and a 100% full orchard discounts below cost instead of spiking to `12.00 CR`!
        let ask_normal = settlement::compute_workshop_ask_cents(1_000, 300, 20, 120);
        assert_eq!(ask_normal, 1_000, "low-inventory orchard quotes at 100% MA, not 92%");
        let ask_full = settlement::compute_workshop_ask_cents(320, 300, 120, 120);
        assert!(
            ask_full < 300,
            "100% full orchard must discount below unit cost to clear stock, got {ask_full}"
        );

        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let empty_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| t.tags.has(TileTag::Land) && usize::from(t.index) != snap_multi.starter_tile_index)
                .unwrap()
                .index,
        );
        let starter_idx = snap_multi.starter_tile_index;
        game.build_on_tile(
            "base:earth",
            starter_idx,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary2),
        )
        .unwrap();
        for (_, (_p, pt)) in game.world.query::<(&Planet, &PlanetTiles)>().iter() {
            let ws = pt.workshops[starter_idx][TileSlotKey::Secondary2.as_index()]
                .as_ref()
                .unwrap();
            assert_eq!(
                ws.inv.avail(cash_id),
                60 * CASH_SCALE,
                "newly built 1x orchard must hold 60 CR working capital"
            );
        }
        let _ = empty_tile;

        let ruling = PolityId(0);
        for m in 0..4 {
            let before = game.planet_money_supply_cents("base:earth");
            game.settle_planet_month("base:earth", ruling);
            let after = game.planet_money_supply_cents("base:earth");
            let minted: i64 = game
                .world
                .query::<&Polity>()
                .iter()
                .map(|(_, p)| p.last_seigniorage_cents)
                .sum();
            assert_eq!(after, before + minted, "multi-culture settlement cash leak in month {m}");
        }
    }

    #[test]
    fn between_jobs_preserves_role_wallet_slides_to_vagrant_on_destitution_and_fixes_zombie_orchards() {
        let mut game = Game::default();
        let ruling = PolityId(0);
        let cash_id = game.goods().cash_id();
        let apple_id = game.goods().id_of(market::GOOD_APPLE).unwrap();

        // 1. Starter capital sector separates State vs LocalPrivate vs `劳工 · 待业` (`Unowned`) + `流民`!
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let cap_sec_u16 = earth0
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .sector_id
            .unwrap();
        let cap0 = earth0.sectors.iter().find(|s| s.id == cap_sec_u16).unwrap();

        let state_lab = cap0
            .pops
            .iter()
            .find(|p| p.id == POP_LABORER && p.employer_kind == "state")
            .expect("must have State laborers");
        let priv_lab = cap0
            .pops
            .iter()
            .find(|p| p.id == POP_LABORER && p.employer_kind == "local_private")
            .expect("must have LocalPrivate laborers");
        let wait_lab = cap0
            .pops
            .iter()
            .find(|p| p.id == POP_LABORER && p.employer_kind == "unowned")
            .expect("must have between-jobs `劳工 · 待业`");
        assert_eq!(state_lab.count, 100);
        assert_eq!(priv_lab.count, 200);
        assert_eq!(wait_lab.count, 50);
        assert_eq!(wait_lab.name, "劳工 · 待业");
        assert_eq!(cap0.vagrant_count, 15, "capital starts with 15 destitute `流民`");

        // 2. Massive laborer layoffs + vagrants + cheap apples MUST NEVER drag Scholar wage down!
        let vag_pid = game.pop_catalog().id_of(POP_VAGRANT).unwrap();
        let goods_for_vag = game.goods().clone();
        for (_, (sec, mkt, spops)) in
            game.world.query_mut::<(&Sector, &mut SectorMarket, &mut SectorPops)>()
        {
            if sec.id == SectorId(cap_sec_u16) {
                mkt.apple_book.ma_price_cents = 320;
                mkt.apple_book.last_price_cents = 320;
                let gi = spops.ensure_group_index(&goods_for_vag, vag_pid, 0, BuildingOwner::Unowned);
                spops.groups[gi].count = 250; // 250 hungry vagrants!
                spops.groups[gi].last_food_met_permille = 300;
            }
        }
        game.settle_planet_month("base:earth", ruling);
        let earth1 = game.planet_by_id("base:earth").unwrap();
        let cap1 = earth1.sectors.iter().find(|s| s.id == cap_sec_u16).unwrap();
        let scholar_quote = cap1
            .wage_quotes
            .iter()
            .find(|q| q.id == POP_ACADEMIC)
            .unwrap();
        assert_eq!(
            scholar_quote.ma_wage_per_100_cents,
            130 * CASH_SCALE,
            "250 hungry vagrants & cheap apples must NEVER cut scholar nominal wage!"
        );
        let soldier_quote = cap1
            .wage_quotes
            .iter()
            .find(|q| q.id == POP_SOLDIER)
            .unwrap();
        assert_eq!(
            soldier_quote.ma_wage_per_100_cents,
            80 * CASH_SCALE,
            "zero soldier vacancies must NOT open labor book or depress soldier MA wage!"
        );
        let bureaucrat_quote = cap1
            .wage_quotes
            .iter()
            .find(|q| q.id == POP_BUREAUCRAT)
            .unwrap();
        assert_eq!(
            bureaucrat_quote.ma_wage_per_100_cents,
            90 * CASH_SCALE,
            "zero bureaucrat vacancies must NOT open labor book or depress bureaucrat MA wage!"
        );

        // 3. Privatize Secondary0 -> staff migrates to `local_private`; then simulate a jammed full
        //    warehouse (`120/120` apples) + `0` cash + wage arrears -> must lay off into `劳工 · 待业`!
        game.privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        for (_, (_p, pt)) in game.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[starter_idx][TileSlotKey::Secondary0.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 0;
                ws.inv.available[usize::from(apple_id.0)] = ws.inv.capacity; // 100% jammed full!
                ws.last_revenue_cents = 0;
                ws.had_wage_arrears = true;
            }
        }
        game.settle_planet_month("base:earth", ruling);
        let after_jam = game.planet_by_id("base:earth").unwrap();
        let sec0_b = after_jam
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .secondary[0]
            .as_ref()
            .unwrap();
        assert_eq!(
            sec0_b.jobs_filled, 0,
            "jammed unpaid zombie orchard must immediately release all 100 workers, got {}",
            sec0_b.jobs_filled
        );

        // 4. Verify `学者 · 待业` keeps own savings initially, and ONLY slides down `demote_to` -> `流民`
        //    once savings hit 0 and food is short!
        let acad_pid = game.pop_catalog().id_of(POP_ACADEMIC).unwrap();
        let goods_clone = game.goods().clone();
        for (_, (sec, spops)) in game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == SectorId(cap_sec_u16) {
                let gi =
                    spops.ensure_group_index(&goods_clone, acad_pid, 0, BuildingOwner::Unowned);
                spops.groups[gi].count = 16;
                spops.groups[gi].sync_capacity();
                spops.groups[gi].inv.available[usize::from(cash_id.0)] = 0; // broke!
                spops.groups[gi].inv.available[usize::from(apple_id.0)] = 0; // empty pantry!
            }
        }
        game.settle_planet_month("base:earth", ruling);
        let after_demote = game.planet_by_id("base:earth").unwrap();
        let cap_dem = after_demote
            .sectors
            .iter()
            .find(|s| s.id == cap_sec_u16)
            .unwrap();
        let rem_acad_wait = cap_dem
            .pops
            .iter()
            .find(|p| p.id == POP_ACADEMIC && p.employer_kind == "unowned")
            .map(|p| p.count)
            .unwrap_or(0);
        assert!(
            rem_acad_wait < 16,
            "destitute hungry `学者 · 待业` must demote along `demote_to`, remaining={rem_acad_wait}"
        );
    }

    #[test]
    fn policies_can_be_changed() {
        let mut game = Game::default();
        let snap = game
            .set_monetary_policy("base:earth", MonetaryPolicy::Loose)
            .unwrap();
        assert_eq!(snap.polity.monetary_policy, MonetaryPolicy::Loose);
        let sid = snap.sectors[0].id;
        let snap = game
            .set_depot_policy("base:earth", sid, DepotPolicy::Stockpile)
            .unwrap();
        assert_eq!(snap.sectors[0].depot_policy, DepotPolicy::Stockpile);
    }

    #[test]
    fn subticks_expand_only_during_combat_and_realign_when_combat_ends() {
        let mut game = Game::default();
        game.set_speed(GameSpeed::Speed2);
        game.step_once();
        assert_eq!(game.counters().combat_subticks, 0);
        assert_eq!(game.counters().daily_ticks, 1);

        let fleet = game.world.spawn((InCombat::default(),));
        for _ in 0..3 {
            game.step_once();
        }
        assert_eq!(game.time().subtick, 3);
        game.world.despawn(fleet).unwrap();

        let out = game.step_once();
        assert!(out.day_advanced);
        assert_eq!(game.time().subtick, 0);
    }

    #[test]
    fn stream_subscriber_receives_surface_once_and_sparse_planet_updates() {
        let runtime = GameRuntime::default();
        let (sub_id, rx) = runtime.subscribe(None).unwrap();

        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(first, StreamEvent::Init { .. }));
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(second, StreamEvent::PlanetSurface { surface: Some(_) }));
        let third = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(third, StreamEvent::PlanetView { planet: Some(_) }));

        runtime
            .build_in_region_async(sub_id, "base:earth".into(), 0, BUILDING_APPLE_ORCHARD.into())
            .unwrap();
        let built_evt = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match built_evt {
            StreamEvent::PlanetView { planet: Some(p) } => {
                assert_eq!(p.total_building_count, 5);
            }
            other => panic!("expected PlanetView after build_in_region_async, got {other:?}"),
        }

        runtime.unsubscribe(sub_id);
    }

    #[test]
    fn spot_driven_ma_unlocks_contract_pool_and_private_investment_guards_cbd_payroll() {
        let goods = GoodCatalog::default();
        let apple = goods.id_of(market::GOOD_APPLE).unwrap();
        let cash = goods.cash_id();

        // 1. Simulate the exact glut from user screenshot: 470 apples offered at `6.80` (`680` cents)
        //    while `MA` starts at `8.40` (`840` cents) and buyers only want `30` apples!
        let mut seller_inv = Inventory::new(goods.len(), 600);
        seller_inv.available[usize::from(apple.0)] = 470;
        let mut buyer_inv = Inventory::new(goods.len(), 200);
        buyer_inv.available[usize::from(cash.0)] = 50_000;
        let mut vault_inv = Inventory::new(goods.len(), 0);
        let mut invs = vec![vault_inv.clone(), seller_inv, buyer_inv];
        let _ = &mut vault_inv;

        let mut books = vec![market::SectorOrderBook::new(apple, 840)];
        let logistics = LogisticsMatrix::from_links(1, &[], &[false]);
        market::clear_planetary_good_market(
            &goods,
            apple,
            &mut books,
            &logistics,
            &mut invs,
            0,
            vec![market::SellSubmission {
                sector_id: 0,
                goods_inv: 1,
                cash_inv: 1,
                ask_price_cents: 680,
                qty: 470,
                allow_contract_pool: true,
            }],
            vec![market::BuySubmission {
                sector_id: 0,
                goods_inv: 2,
                cash_inv: 2,
                bid_price_cents: 900,
                qty: 30,
                allow_contract_pool: true,
            }],
        );
        market::release_resting_orders(&goods, apple, &mut books, &mut invs);
        // Spot trades happen at `6.80`, and `MA` MUST step down below `8.00` on the very first month
        // (instead of staying self-locked near `8.40` from 70% contract-pool volume!).
        assert!(
            books[0].ma_price_cents <= 790,
            "spot-driven MA must step down toward 680 during a 470-apple glut, got {}",
            books[0].ma_price_cents
        );

        // 2. Private autonomous investment MUST protect 6 months of `商业街` payroll AND skip glutted sectors!
        let mut game = Game::default();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let cap_sec_u16 = earth0
            .occupied_tiles
            .iter()
            .find(|t| t.index == earth0.starter_tile_index)
            .unwrap()
            .sector_id
            .unwrap();
        // Give `SectorCapital` 280 CR (enough for old `150 + 120 = 270`, NOT enough for `150 + 6*22.5 + 60 = 345`!)
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == SectorId(cap_sec_u16) {
                cap.vault.available[usize::from(cash.0)] = 280 * CASH_SCALE;
            }
        }
        let b_before = game.planet_by_id("base:earth").unwrap().total_building_count;
        game.run_monthly_political_economy();
        let after_guard = game.planet_by_id("base:earth").unwrap();
        assert_eq!(
            after_guard.total_building_count, b_before,
            "private capital must NOT raid 6-month CBD clerk payroll reserve to overbuild!"
        );
        let cbd_slot = after_guard
            .occupied_tiles
            .iter()
            .find(|t| t.index == earth0.starter_tile_index)
            .unwrap()
            .secondary[1]
            .as_ref()
            .unwrap();
        assert_eq!(
            cbd_slot.jobs_filled, 40,
            "CBD must retain all 40/40 staff (`15 资本家 + 25 官僚`) without going broke!"
        );
    }
}
