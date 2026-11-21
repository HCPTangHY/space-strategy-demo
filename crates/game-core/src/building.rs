//! Building & Tile-Slot Object Layer (`建筑与槽位对象及修正算式层`).

use crate::economy::{BuildingOwnerSnapshot, PopTypeCatalog, PopTypeId, POP_CAPITALIST};
#[cfg(test)]
use crate::economy::{
    CAMPUS_HOSTING_CAPACITY, CBD_HOSTING_CAPACITY, POP_ACADEMIC, POP_BUREAUCRAT, POP_ENGINEER,
    POP_LABORER, POP_SOLDIER,
};
use crate::market::{GoodCatalog, GoodId, CASH_SCALE};
#[cfg(test)]
use crate::market::{GOOD_APPLE, GOOD_SERVICE};
use crate::protocol::{BuildingSnapshot, PlacedBuildingSnapshot};
use crate::surface::TileTags;
#[cfg(test)]
use crate::surface::TileTag;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroU16;

pub const SECONDARY_SLOT_COUNT: usize = 3;
pub const SLOTS_PER_TILE: usize = 1 + SECONDARY_SLOT_COUNT; // 1 primary + 3 secondary

pub const BUILDING_APPLE_ORCHARD: &str = "base:apple_orchard";
pub const BUILDING_REGIONAL_CORE: &str = "base:regional_core";
pub const BUILDING_PLANETARY_CAPITAL: &str = "base:planetary_capital";
pub const BUILDING_COLONIAL_OUTPOST: &str = "base:colonial_outpost";
pub const BUILDING_COMMERCIAL_STREET: &str = "base:commercial_street";
pub const BUILDING_CORPORATE_CAMPUS: &str = "base:corporate_campus";

/// Compact 2-byte building catalog handle (`Option<BuildingTypeId>` is still 2 bytes via `NonZeroU16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    Power = 5,
    Water = 6,
    Mining = 7,
    Refining = 8,
    Manufacturing = 9,
    Military = 10,
    Infrastructure = 11,
}

impl BuildingTag {
    pub const ALL: [Self; 12] = [
        Self::PlanetaryCapital,
        Self::RegionalCore,
        Self::Agriculture,
        Self::Commercial,
        Self::CorporateCampus,
        Self::Power,
        Self::Water,
        Self::Mining,
        Self::Refining,
        Self::Manufacturing,
        Self::Military,
        Self::Infrastructure,
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
            Self::Power => "power",
            Self::Water => "water",
            Self::Mining => "mining",
            Self::Refining => "refining",
            Self::Manufacturing => "manufacturing",
            Self::Military => "military",
            Self::Infrastructure => "infrastructure",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "planetary_capital" => Some(Self::PlanetaryCapital),
            "regional_core" => Some(Self::RegionalCore),
            "agriculture" => Some(Self::Agriculture),
            "commercial" => Some(Self::Commercial),
            "corporate_campus" => Some(Self::CorporateCampus),
            "power" => Some(Self::Power),
            "water" => Some(Self::Water),
            "mining" => Some(Self::Mining),
            "refining" => Some(Self::Refining),
            "manufacturing" => Some(Self::Manufacturing),
            "military" => Some(Self::Military),
            "infrastructure" => Some(Self::Infrastructure),
            _ => None,
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::PlanetaryCapital => "行星首府",
            Self::RegionalCore => "大区核心",
            Self::Agriculture => "农业设施",
            Self::Commercial => "商业街",
            Self::CorporateCampus => "企业园区",
            Self::Power => "电力设施",
            Self::Water => "水务设施",
            Self::Mining => "采掘设施",
            Self::Refining => "精炼化工",
            Self::Manufacturing => "高端制造",
            Self::Military => "军工防务",
            Self::Infrastructure => "跨区基建",
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

impl<'de> Deserialize<'de> for BuildingTags {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let list = Vec::<String>::deserialize(deserializer)?;
        let mut tags = BuildingTags::EMPTY;
        for s in list {
            if let Some(tag) = BuildingTag::parse(&s) {
                tags = tags.with(tag);
            }
        }
        Ok(tags)
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum JobPriority {
    Core = 0,
    Commercial = 1,
    #[default]
    Standard = 2,
}

const fn default_building_type_id() -> BuildingTypeId {
    BuildingTypeId(NonZeroU16::MIN)
}

/// Pure editable data-serializable building definition (`jobs_by_role`, `monthly_inputs`, and
/// `monthly_output` reference string Pop type IDs and Good IDs; `Tile` stores 2-byte `BuildingTypeId`!).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingDef {
    #[serde(default = "default_building_type_id")]
    pub type_id: BuildingTypeId,
    pub id: String,
    pub name: String,
    pub slot_rule: SlotRule,
    #[serde(default)]
    pub job_priority: JobPriority,
    pub build_cost: i64,
    #[serde(default)]
    pub hosting_capacity: u16,
    #[serde(default)]
    pub jobs_by_role: Vec<(String, u32)>,
    #[serde(default)]
    pub tags: BuildingTags,
    #[serde(default)]
    pub unique_planet_tags: BuildingTags,
    #[serde(default)]
    pub unique_region_tags: BuildingTags,
    #[serde(default)]
    pub unique_tile_tags: BuildingTags,
    /// Required tags on the same tile's `Primary` slot when placing on a secondary slot (`EMPTY` = any primary).
    #[serde(default)]
    pub required_primary_tags: BuildingTags,
    #[serde(default)]
    pub required_tags: TileTags,
    #[serde(default)]
    pub forbidden_tags: TileTags,
    #[serde(default)]
    pub monthly_inputs: Vec<(String, i64)>,
    #[serde(default)]
    pub monthly_output: Vec<(String, i64)>,
}

#[cfg(test)]
pub fn default_building_defs() -> Vec<BuildingDef> {
    vec![
        BuildingDef {
            type_id: BuildingTypeId(NonZeroU16::new(1).unwrap()),
            id: BUILDING_APPLE_ORCHARD.into(),
            name: "苹果园".into(),
            slot_rule: SlotRule::Any,
            job_priority: JobPriority::Standard,
            build_cost: 150,
            hosting_capacity: 0,
            jobs_by_role: vec![(POP_LABORER.into(), 100)],
            tags: BuildingTags::EMPTY.with(BuildingTag::Agriculture),
            unique_planet_tags: BuildingTags::EMPTY,
            unique_region_tags: BuildingTags::EMPTY,
            unique_tile_tags: BuildingTags::EMPTY,
            required_primary_tags: BuildingTags::EMPTY,
            required_tags: TileTags::EMPTY,
            forbidden_tags: TileTags::EMPTY,
            monthly_inputs: Vec::new(),
            monthly_output: vec![(GOOD_APPLE.into(), 20)],
        },
        BuildingDef {
            type_id: BuildingTypeId(NonZeroU16::new(2).unwrap()),
            id: BUILDING_REGIONAL_CORE.into(),
            name: "大区核心".into(),
            slot_rule: SlotRule::PrimaryOnly,
            job_priority: JobPriority::Core,
            build_cost: 400,
            hosting_capacity: 0,
            jobs_by_role: vec![
                (POP_BUREAUCRAT.into(), 70),
                (POP_ENGINEER.into(), 30),
                (POP_SOLDIER.into(), 30),
                (POP_LABORER.into(), 20),
            ],
            tags: BuildingTags::EMPTY.with(BuildingTag::RegionalCore),
            unique_planet_tags: BuildingTags::EMPTY,
            unique_region_tags: BuildingTags::EMPTY.with(BuildingTag::RegionalCore),
            unique_tile_tags: BuildingTags::EMPTY,
            required_primary_tags: BuildingTags::EMPTY,
            required_tags: TileTags::EMPTY.with(TileTag::Land),
            forbidden_tags: TileTags::EMPTY,
            monthly_inputs: Vec::new(),
            monthly_output: Vec::new(),
        },
        BuildingDef {
            type_id: BuildingTypeId(NonZeroU16::new(3).unwrap()),
            id: BUILDING_PLANETARY_CAPITAL.into(),
            name: "行星首府".into(),
            slot_rule: SlotRule::PrimaryOnly,
            job_priority: JobPriority::Core,
            build_cost: 800,
            hosting_capacity: 0,
            jobs_by_role: vec![
                (POP_BUREAUCRAT.into(), 80),
                (POP_ACADEMIC.into(), 40),
                (POP_ENGINEER.into(), 40),
                (POP_SOLDIER.into(), 40),
            ],
            tags: BuildingTags::EMPTY
                .with(BuildingTag::PlanetaryCapital)
                .with(BuildingTag::RegionalCore),
            unique_planet_tags: BuildingTags::EMPTY.with(BuildingTag::PlanetaryCapital),
            unique_region_tags: BuildingTags::EMPTY.with(BuildingTag::RegionalCore),
            unique_tile_tags: BuildingTags::EMPTY,
            required_primary_tags: BuildingTags::EMPTY,
            required_tags: TileTags::EMPTY.with(TileTag::Land),
            forbidden_tags: TileTags::EMPTY,
            monthly_inputs: Vec::new(),
            monthly_output: Vec::new(),
        },
        BuildingDef {
            type_id: BuildingTypeId(NonZeroU16::new(4).unwrap()),
            id: BUILDING_COMMERCIAL_STREET.into(),
            name: "商业街".into(),
            slot_rule: SlotRule::SecondaryOnly,
            job_priority: JobPriority::Commercial,
            build_cost: 200,
            hosting_capacity: CBD_HOSTING_CAPACITY,
            jobs_by_role: vec![(POP_CAPITALIST.into(), 15), (POP_BUREAUCRAT.into(), 25)],
            tags: BuildingTags::EMPTY.with(BuildingTag::Commercial),
            unique_planet_tags: BuildingTags::EMPTY,
            unique_region_tags: BuildingTags::EMPTY,
            unique_tile_tags: BuildingTags::EMPTY,
            required_primary_tags: BuildingTags::EMPTY
                .with(BuildingTag::RegionalCore)
                .with(BuildingTag::CorporateCampus),
            required_tags: TileTags::EMPTY.with(TileTag::Land),
            forbidden_tags: TileTags::EMPTY,
            monthly_inputs: Vec::new(),
            monthly_output: vec![(GOOD_SERVICE.into(), 15)],
        },
        BuildingDef {
            type_id: BuildingTypeId(NonZeroU16::new(5).unwrap()),
            id: BUILDING_CORPORATE_CAMPUS.into(),
            name: "企业园区".into(),
            slot_rule: SlotRule::PrimaryOnly,
            job_priority: JobPriority::Commercial,
            build_cost: 350,
            hosting_capacity: CAMPUS_HOSTING_CAPACITY,
            jobs_by_role: vec![
                (POP_CAPITALIST.into(), 30),
                (POP_ENGINEER.into(), 40),
                (POP_BUREAUCRAT.into(), 30),
            ],
            tags: BuildingTags::EMPTY.with(BuildingTag::CorporateCampus),
            unique_planet_tags: BuildingTags::EMPTY,
            unique_region_tags: BuildingTags::EMPTY,
            unique_tile_tags: BuildingTags::EMPTY,
            required_primary_tags: BuildingTags::EMPTY,
            required_tags: TileTags::EMPTY.with(TileTag::Land),
            forbidden_tags: TileTags::EMPTY,
            monthly_inputs: Vec::new(),
            monthly_output: Vec::new(),
        },
    ]
}

/// Editable dynamic catalog of `BuildingDef`s indexed by compact `BuildingTypeId(NonZeroU16)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingCatalog {
    pub(crate) defs: Vec<BuildingDef>,
    pub(crate) by_id: BTreeMap<String, BuildingTypeId>,
}

impl Default for BuildingCatalog {
    fn default() -> Self {
        crate::data_bundle::load_economy_v1_bundle()
            .expect("从外部 gamedata/catalog 加载建筑数据表失败")
            .buildings
    }
}

impl BuildingCatalog {
    pub fn from_defs(mut defs: Vec<BuildingDef>) -> Result<Self, String> {
        if defs.is_empty() {
            return Err("建筑数据表不能为空。".into());
        }
        if defs.len() >= usize::from(u16::MAX) {
            return Err("建筑数量超出 u16 上限。".into());
        }
        let mut by_id = BTreeMap::new();
        for (i, d) in defs.iter_mut().enumerate() {
            if d.id.trim().is_empty() {
                return Err("建筑 ID 不能为空。".into());
            }
            let tid = BuildingTypeId(NonZeroU16::new((i + 1) as u16).unwrap());
            d.type_id = tid;
            if by_id.insert(d.id.clone(), tid).is_some() {
                return Err(format!("重复的建筑 ID：{}", d.id));
            }
        }
        Ok(Self { defs, by_id })
    }

    #[inline]
    pub fn defs(&self) -> &[BuildingDef] {
        &self.defs
    }

    #[inline]
    pub fn get(&self, type_id: BuildingTypeId) -> &BuildingDef {
        let idx = (type_id.0.get() as usize).saturating_sub(1);
        &self.defs[idx]
    }

    #[inline]
    pub fn by_id(&self, id: &str) -> Result<&BuildingDef, String> {
        let tid = self
            .by_id
            .get(id)
            .copied()
            .ok_or_else(|| format!("未知的建筑类型：{id}"))?;
        Ok(self.get(tid))
    }

    /// Hot-patches building definitions in place by string `id`, preserving existing `BuildingTypeId`s
    /// and appending newly added buildings. Rejects deletion of currently placed `active_ids`.
    pub fn patch_in_place(
        &mut self,
        incoming: BuildingCatalog,
        active_ids: &std::collections::BTreeSet<BuildingTypeId>,
    ) -> Result<(), String> {
        for &used_tid in active_ids {
            let cur_def = self.get(used_tid);
            if !incoming.by_id.contains_key(&cur_def.id) {
                return Err(format!(
                    "当前地图已建有建筑 {}（{}），无法原位删除；请使用「重载重开局」。",
                    cur_def.name, cur_def.id
                ));
            }
        }
        for inc_def in incoming.defs {
            if let Some(&tid) = self.by_id.get(&inc_def.id) {
                let idx = (tid.0.get() as usize).saturating_sub(1);
                let mut patched = inc_def;
                patched.type_id = tid;
                self.defs[idx] = patched;
            } else {
                self.register_building(inc_def)?;
            }
        }
        Ok(())
    }

    pub fn register_building(&mut self, mut def: BuildingDef) -> Result<BuildingTypeId, String> {
        if def.id.trim().is_empty() {
            return Err("建筑 ID 不能为空。".into());
        }
        if self.by_id.contains_key(&def.id) {
            return Err(format!("重复的建筑 ID：{}", def.id));
        }
        if self.defs.len() >= usize::from(u16::MAX) - 1 {
            return Err("建筑数量超出 u16 上限。".into());
        }
        let tid = BuildingTypeId(NonZeroU16::new((self.defs.len() + 1) as u16).unwrap());
        def.type_id = tid;
        self.by_id.insert(def.id.clone(), tid);
        self.defs.push(def);
        Ok(tid)
    }
}

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
        for (role_str, base_req) in &self.jobs_by_role {
            let pid = self.resolve_role_id(catalog, role_str);
            let wage_100 = catalog.get(pid).base_wage_per_100;
            if wage_100 > 0 {
                total += (i64::from(*base_req) * scale * wage_100 * CASH_SCALE) / 100;
            }
        }
        total
    }

    /// Generic multi-good expected net output value (`outputs * MA - inputs * MA`) for `employed` staff,
    /// bounded by `free_warehouse_volume` (zero hardcoded `GOOD_APPLE`!).
    pub fn slot_expected_net_value_cents(
        &self,
        goods: &GoodCatalog,
        good_ma: &BTreeMap<GoodId, i64>,
        slot: TileSlotKey,
        employed: u32,
        bonus_permille: i64,
        free_warehouse_volume: i64,
    ) -> i64 {
        let scale = i64::from(self.scale_for_slot(slot));
        let req = self.slot_jobs_required(slot);
        let mut rem_vol = free_warehouse_volume.max(0);
        let mut out_val = 0i64;
        for (good_str, base_out) in &self.monthly_output {
            let Some(gid) = goods.id_of(good_str) else {
                continue;
            };
            let gdef = goods.get(gid);
            let with_bonus = ((*base_out * scale) * (1000 + bonus_permille) + 500) / 1000;
            let raw = scale_output_by_staffing(with_bonus, employed, req).max(0);
            let fit = if gdef.unit_volume > 0 {
                let f = (rem_vol / i64::from(gdef.unit_volume)).min(raw);
                rem_vol -= f * i64::from(gdef.unit_volume);
                f
            } else {
                raw
            };
            let px = good_ma.get(&gid).copied().unwrap_or(gdef.genesis_price_cents);
            out_val += fit * px;
        }
        let mut in_cost = 0i64;
        for (good_str, base_in) in &self.monthly_inputs {
            let Some(gid) = goods.id_of(good_str) else {
                continue;
            };
            let needed = scale_output_by_staffing(*base_in * scale, employed, req).max(0);
            let px = good_ma
                .get(&gid)
                .copied()
                .unwrap_or(goods.get(gid).genesis_price_cents);
            in_cost += needed * px;
        }
        out_val - in_cost
    }

    pub fn is_state_core_or_public(&self) -> bool {
        self.tags.has(BuildingTag::PlanetaryCapital) || self.tags.has(BuildingTag::RegionalCore)
    }

    pub fn is_privatizable_industry(&self) -> bool {
        !self.is_state_core_or_public()
            && !self.tags.has(BuildingTag::CorporateCampus)
            && !self.monthly_output.is_empty()
    }

    pub fn can_use_slot(&self, slot: TileSlotKey) -> bool {
        match self.slot_rule {
            SlotRule::PrimaryOnly => slot.is_primary(),
            SlotRule::SecondaryOnly => !slot.is_primary(),
            SlotRule::Any => true,
        }
    }

    /// Generic secondary-slot prerequisite check against the tile's `Primary` building tags.
    pub fn matches_primary_slot(
        &self,
        buildings: &BuildingCatalog,
        slots: &TileSlots,
        slot: TileSlotKey,
    ) -> bool {
        if slot.is_primary() || self.required_primary_tags == BuildingTags::EMPTY {
            return true;
        }
        slots
            .primary
            .map(|ptid| buildings.get(ptid).tags.intersects(self.required_primary_tags))
            .unwrap_or(false)
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
        let mut monthly_inputs = BTreeMap::new();
        for (good, delta) in &self.monthly_inputs {
            monthly_inputs.insert(good.clone(), *delta);
        }
        let mut monthly_output = BTreeMap::new();
        for (good, delta) in &self.monthly_output {
            monthly_output.insert(good.clone(), *delta);
        }
        let mut role_jobs = BTreeMap::new();
        for (role_id, count) in &self.jobs_by_role {
            role_jobs.insert(role_id.clone(), *count);
        }
        BuildingSnapshot {
            id: self.id.clone(),
            name: self.name.clone(),
            slot_rule: self.slot_rule,
            job_priority: self.job_priority,
            build_cost: self.build_cost,
            hosting_capacity: self.hosting_capacity,
            jobs_required: self.base_jobs_required(),
            role_jobs,
            tags: self.tags,
            required_tile_tags: self.required_tags,
            forbidden_tile_tags: self.forbidden_tags,
            required_primary_tags: self.required_primary_tags,
            unique_planet_tags: self.unique_planet_tags,
            unique_region_tags: self.unique_region_tags,
            unique_tile_tags: self.unique_tile_tags,
            monthly_inputs,
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
        let mut monthly_inputs = BTreeMap::new();
        for (good, unit_in) in &self.monthly_inputs {
            monthly_inputs.insert(good.clone(), *unit_in * i64::from(scale));
        }
        let mut base_monthly_output = BTreeMap::new();
        let mut effective_monthly_output = BTreeMap::new();
        for (good, unit_delta) in &self.monthly_output {
            let scaled_base = *unit_delta * i64::from(scale);
            let with_bonus = (scaled_base * (1000 + bonus_permille) + 500) / 1000;
            base_monthly_output.insert(good.clone(), with_bonus);
            effective_monthly_output.insert(
                good.clone(),
                scale_output_by_staffing(with_bonus, jobs_filled, jobs_required),
            );
        }
        PlacedBuildingSnapshot {
            id: self.id.clone(),
            name: self.name.clone(),
            slot_rule: self.slot_rule,
            scale_multiplier: scale as u8,
            job_priority: self.job_priority,
            tags: self.tags,
            jobs_required,
            jobs_filled,
            hosting_capacity: self.hosting_capacity,
            owner,
            monthly_inputs,
            base_monthly_output,
            effective_monthly_output,
        }
    }
}

pub fn good_display_name<'a>(goods: &'a GoodCatalog, good_id: &'a str) -> &'a str {
    goods
        .id_of(good_id)
        .map(|gid| goods.get(gid).name.as_str())
        .unwrap_or(good_id)
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

    pub fn first_compatible_empty_slot(
        &self,
        buildings: &BuildingCatalog,
        def: &BuildingDef,
    ) -> Option<TileSlotKey> {
        TileSlotKey::ALL.into_iter().find(|&k| {
            self.get(k).is_none()
                && def.can_use_slot(k)
                && def.matches_primary_slot(buildings, self, k)
        })
    }

    /// Up to two distinct empty slot candidates (`Primary` + first empty `Secondary`) compatible with `def`.
    pub fn candidate_empty_slots(
        &self,
        buildings: &BuildingCatalog,
        def: &BuildingDef,
    ) -> [Option<TileSlotKey>; 2] {
        let pri = (self.primary.is_none()
            && def.can_use_slot(TileSlotKey::Primary)
            && def.matches_primary_slot(buildings, self, TileSlotKey::Primary))
        .then_some(TileSlotKey::Primary);
        let sec = [
            TileSlotKey::Secondary0,
            TileSlotKey::Secondary1,
            TileSlotKey::Secondary2,
        ]
        .into_iter()
        .find(|&k| {
            self.get(k).is_none()
                && def.can_use_slot(k)
                && def.matches_primary_slot(buildings, self, k)
        });
        [pri, sec]
    }

    pub fn iter_occupied(&self) -> impl Iterator<Item = (TileSlotKey, BuildingTypeId)> + '_ {
        TileSlotKey::ALL
            .into_iter()
            .filter_map(move |k| self.get(k).map(|tid| (k, tid)))
    }
}
