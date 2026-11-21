//! Household & Demography Tier (`人群与家庭层`: `PopTypeCatalog`, `PopGroup`, `SectorPops`).

use super::ids::{BuildingOwner, PopId, PopTypeId};
#[cfg(test)]
use super::{
    POP_ACADEMIC, POP_BUREAUCRAT, POP_CAPITALIST, POP_ENGINEER, POP_LABORER, POP_SOLDIER,
    POP_VAGRANT,
};
use crate::market::{compute_per_100_demand, GoodCatalog, GoodId, Inventory, CASH_SCALE};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// bankruptcy/demotion chain live purely in data).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopTypeDef {
    pub id: String,
    pub name: String,
    pub base_wage_per_100: i64,
    #[serde(default)]
    pub subsistence_apple_per_100: i64,
    pub is_default_pool: bool,
    /// Marks the bottom destitute urban pool (`流民`: zero land, zero savings, high unrest).
    #[serde(default)]
    pub is_vagrant_pool: bool,
    pub receives_dividends: bool,
    pub can_demote: bool,
    /// Target occupation when an `Unowned` between-jobs group exhausts savings and goes hungry.
    #[serde(default)]
    pub demote_to: Option<String>,
    /// Baseline lifestyle expectation tier (used by universal consumption ladder & satisfaction).
    #[serde(default = "default_expected_sol")]
    pub expected_sol: u8,
    /// Payroll & contract obligation redemption priority (`0` = highest priority, e.g. military; `3` = standard).
    #[serde(default = "default_redemption_tier")]
    pub redemption_tier: u8,
    /// Months of basic household living expenses retained as emergency reserve before spending/investing surplus cash.
    #[serde(default = "default_pop_reserve_months")]
    pub reserve_months: u8,
    /// Permille of surplus cash above `reserve_months` spent on extra service/amenity consumption each month.
    #[serde(default = "default_surplus_service_permille")]
    pub surplus_service_permille: u16,
    /// Permille of surplus cash above `reserve_months` allocated to securities or local `SectorCapital` each month.
    #[serde(default = "default_surplus_invest_permille")]
    pub surplus_invest_permille: u16,
    pub promote_from: Vec<String>,
}

impl PopTypeDef {
    /// True only if a custom mod explicitly configures building-free subsistence output.
    /// In vanilla modern/interstellar definitions all goods require real building slots (`false`).
    #[inline]
    pub const fn is_self_subsistence(&self) -> bool {
        self.subsistence_apple_per_100 > 0
    }
}

const fn default_expected_sol() -> u8 {
    5
}

const fn default_redemption_tier() -> u8 {
    3
}

const fn default_pop_reserve_months() -> u8 {
    2
}

const fn default_surplus_service_permille() -> u16 {
    200
}

const fn default_surplus_invest_permille() -> u16 {
    150
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopTypeCatalog {
    defs: Vec<PopTypeDef>,
    by_id: BTreeMap<String, PopTypeId>,
    default_pool: PopTypeId,
    vagrant_pool: PopTypeId,
}

impl Default for PopTypeCatalog {
    fn default() -> Self {
        crate::data_bundle::load_economy_v1_bundle()
            .expect("从外部 gamedata/catalog 加载职业数据表失败")
            .pop_types
    }
}

impl PopTypeCatalog {
    pub fn from_defs(defs: Vec<PopTypeDef>) -> Result<Self, String> {
        if defs.is_empty() {
            return Err("Pop 类型数据表不能为空。".into());
        }
        if defs.len() > usize::from(u16::MAX) {
            return Err("Pop 类型数量超出 u16 上限。".into());
        }
        let mut by_id = BTreeMap::new();
        let mut default_pool = None;
        let mut vagrant_pool = None;
        for (idx, def) in defs.iter().enumerate() {
            if def.id.trim().is_empty() {
                return Err("Pop 类型 ID 不能为空。".into());
            }
            let pid = PopTypeId(idx as u16);
            if by_id.insert(def.id.clone(), pid).is_some() {
                return Err(format!("重复的 Pop 类型 ID：{}", def.id));
            }
            if def.is_default_pool && default_pool.is_none() {
                default_pool = Some(pid);
            }
            if def.is_vagrant_pool && vagrant_pool.is_none() {
                vagrant_pool = Some(pid);
            }
        }
        let default_pool = default_pool.unwrap_or(PopTypeId(0));
        let vagrant_pool = vagrant_pool.unwrap_or(default_pool);
        Ok(Self {
            defs,
            by_id,
            default_pool,
            vagrant_pool,
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    #[inline]
    pub fn defs(&self) -> &[PopTypeDef] {
        &self.defs
    }

    #[inline]
    pub fn default_pool_id(&self) -> PopTypeId {
        self.default_pool
    }

    #[inline]
    pub fn vagrant_pool_id(&self) -> PopTypeId {
        self.vagrant_pool
    }

    #[inline]
    pub fn has_dedicated_vagrant_pool(&self) -> bool {
        self.vagrant_pool != self.default_pool
    }

    #[inline]
    pub fn id_of(&self, str_id: &str) -> Option<PopTypeId> {
        self.by_id.get(str_id).copied()
    }

    #[inline]
    pub fn get(&self, id: PopTypeId) -> &PopTypeDef {
        &self.defs[usize::from(id.0)]
    }

    /// Hot-patches occupation definitions in place by string `id`, preserving existing `PopTypeId`s
    /// and appending newly added occupation types.
    pub fn patch_in_place(
        &mut self,
        incoming: PopTypeCatalog,
        active_ids: &std::collections::BTreeSet<PopTypeId>,
    ) -> Result<(), String> {
        for &used_pid in active_ids {
            let cur_def = self.get(used_pid);
            if incoming.id_of(&cur_def.id).is_none() {
                return Err(format!(
                    "当前人口包含职业 {}（{}），无法原位删除；请使用「重载重开局」。",
                    cur_def.name, cur_def.id
                ));
            }
        }
        for inc_def in incoming.defs {
            if let Some(pid) = self.id_of(&inc_def.id) {
                self.defs[usize::from(pid.0)] = inc_def;
            } else {
                let pid = PopTypeId(self.defs.len() as u16);
                self.by_id.insert(inc_def.id.clone(), pid);
                self.defs.push(inc_def);
            }
        }
        let mut default_pool = None;
        let mut vagrant_pool = None;
        for (idx, d) in self.defs.iter().enumerate() {
            let pid = PopTypeId(idx as u16);
            if d.is_default_pool && default_pool.is_none() {
                default_pool = Some(pid);
            }
            if d.is_vagrant_pool && vagrant_pool.is_none() {
                vagrant_pool = Some(pid);
            }
        }
        self.default_pool = default_pool.unwrap_or(PopTypeId(0));
        self.vagrant_pool = vagrant_pool.unwrap_or(self.default_pool);
        Ok(())
    }

    pub fn dividend_recipients(&self) -> Vec<PopTypeId> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.receives_dividends.then_some(PopTypeId(i as u16)))
            .collect()
    }
}

#[cfg(test)]
pub fn default_pop_type_defs() -> Vec<PopTypeDef> {
    vec![
        PopTypeDef {
            id: POP_VAGRANT.into(),
            name: "流民".into(),
            base_wage_per_100: 0,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: true,
            receives_dividends: false,
            can_demote: false,
            demote_to: None,
            expected_sol: 1,
            redemption_tier: 3,
            reserve_months: 1,
            surplus_service_permille: 100,
            surplus_invest_permille: 0,
            promote_from: Vec::new(),
        },
        PopTypeDef {
            id: POP_LABORER.into(),
            name: "劳工".into(),
            base_wage_per_100: 60,
            subsistence_apple_per_100: 0,
            is_default_pool: true,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_VAGRANT.into()),
            expected_sol: 5,
            redemption_tier: 3,
            reserve_months: 2,
            surplus_service_permille: 250,
            surplus_invest_permille: 100,
            promote_from: vec![POP_VAGRANT.into()],
        },
        PopTypeDef {
            id: POP_ENGINEER.into(),
            name: "工程师".into(),
            base_wage_per_100: 120,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_LABORER.into()),
            expected_sol: 11,
            redemption_tier: 1,
            reserve_months: 3,
            surplus_service_permille: 220,
            surplus_invest_permille: 200,
            promote_from: vec![POP_BUREAUCRAT.into(), POP_LABORER.into()],
        },
        PopTypeDef {
            id: POP_ACADEMIC.into(),
            name: "学者".into(),
            base_wage_per_100: 130,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_LABORER.into()),
            expected_sol: 12,
            redemption_tier: 2,
            reserve_months: 3,
            surplus_service_permille: 240,
            surplus_invest_permille: 200,
            promote_from: vec![
                POP_BUREAUCRAT.into(),
                POP_ENGINEER.into(),
                POP_LABORER.into(),
            ],
        },
        PopTypeDef {
            id: POP_BUREAUCRAT.into(),
            name: "官僚".into(),
            base_wage_per_100: 90,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_LABORER.into()),
            expected_sol: 9,
            redemption_tier: 1,
            reserve_months: 2,
            surplus_service_permille: 220,
            surplus_invest_permille: 150,
            promote_from: vec![POP_LABORER.into()],
        },
        PopTypeDef {
            id: POP_SOLDIER.into(),
            name: "军人".into(),
            base_wage_per_100: 80,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_LABORER.into()),
            expected_sol: 7,
            redemption_tier: 0,
            reserve_months: 2,
            surplus_service_permille: 200,
            surplus_invest_permille: 100,
            promote_from: vec![POP_LABORER.into(), POP_VAGRANT.into()],
        },
        PopTypeDef {
            id: POP_CAPITALIST.into(),
            name: "资本家".into(),
            base_wage_per_100: 0,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: false,
            receives_dividends: true,
            can_demote: true,
            demote_to: Some(POP_BUREAUCRAT.into()),
            expected_sol: 16,
            redemption_tier: 2,
            reserve_months: 3,
            surplus_service_permille: 180,
            surplus_invest_permille: 450,
            promote_from: vec![
                POP_BUREAUCRAT.into(),
                POP_ENGINEER.into(),
                POP_LABORER.into(),
            ],
        },
    ]
}

#[inline]
pub fn household_pantry_capacity(count: u32) -> i64 {
    ((i64::from(count) * 14 + 99) / 100).max(24)
}

/// Aggregated demographic group inside a `Sector`, uniquely keyed by `(pop_type, culture_id, employer)`
/// and referenced via stable `PopId` from building `StaffContract`s.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopGroup {
    pub id: PopId,
    pub pop_type: PopTypeId,
    pub culture_id: u16,
    pub employer: BuildingOwner,
    pub count: u32,
    pub inv: Inventory,
    pub satisfaction: u8,
    pub last_income_cents: i64,
    pub last_food_met_permille: u16,
}

impl PopGroup {
    pub fn new(
        good_catalog: &GoodCatalog,
        pop_type: PopTypeId,
        count: u32,
        initial_cash_credits: i64,
        initial_food_units: i64,
    ) -> Self {
        Self::with_identity(
            good_catalog,
            PopId(0),
            pop_type,
            0,
            BuildingOwner::Unowned,
            count,
            initial_cash_credits,
            initial_food_units,
        )
    }

    pub fn with_culture(
        good_catalog: &GoodCatalog,
        pop_type: PopTypeId,
        culture_id: u16,
        count: u32,
        initial_cash_credits: i64,
        initial_food_units: i64,
    ) -> Self {
        Self::with_identity(
            good_catalog,
            PopId(0),
            pop_type,
            culture_id,
            BuildingOwner::Unowned,
            count,
            initial_cash_credits,
            initial_food_units,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_identity(
        good_catalog: &GoodCatalog,
        id: PopId,
        pop_type: PopTypeId,
        culture_id: u16,
        employer: BuildingOwner,
        count: u32,
        initial_cash_credits: i64,
        initial_food_units: i64,
    ) -> Self {
        let cap = household_pantry_capacity(count);
        let mut inv = Inventory::new(good_catalog.len(), cap);
        let cash_id = good_catalog.cash_id();
        inv.available[usize::from(cash_id.0)] = initial_cash_credits.max(0) * CASH_SCALE;
        if let Some((staple_id, _)) = good_catalog.primary_staple() {
            inv.produce_capped(good_catalog, staple_id, initial_food_units);
        }
        Self {
            id,
            pop_type,
            culture_id,
            employer,
            count,
            inv,
            satisfaction: 75,
            last_income_cents: 0,
            last_food_met_permille: 1000,
        }
    }

    pub fn sync_capacity(&mut self) {
        self.inv.capacity = household_pantry_capacity(self.count);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorPops {
    pub groups: Vec<PopGroup>,
    pub next_pop_id: u32,
    pub last_growth: u32,
}

impl SectorPops {
    /// Spawns explicit unattached `(PopTypeId, count, cash_cr, staple_units)` household groups
    /// without coupling `PopTypeDef` to any scenario-specific starter counts or savings.
    pub fn from_unowned_groups(
        good_catalog: &GoodCatalog,
        default_pool: PopTypeId,
        vagrant_pool: Option<PopTypeId>,
        specs: &[(PopTypeId, u32, i64, i64)],
    ) -> Self {
        if specs.is_empty() {
            return Self {
                groups: vec![PopGroup::with_identity(
                    good_catalog,
                    PopId(1),
                    default_pool,
                    0,
                    BuildingOwner::Unowned,
                    0,
                    0,
                    0,
                )],
                next_pop_id: 2,
                last_growth: 0,
            };
        }

        let mut groups = Vec::with_capacity(specs.len());
        let mut next_id = 1u32;
        for &(pid, count, cash_cr, staple_units) in specs {
            if count == 0 && !groups.is_empty() {
                continue;
            }
            let mut g = PopGroup::with_identity(
                good_catalog,
                PopId(next_id),
                pid,
                0,
                BuildingOwner::Unowned,
                count,
                cash_cr,
                staple_units,
            );
            if vagrant_pool == Some(pid) {
                g.satisfaction = 42;
            }
            groups.push(g);
            next_id += 1;
        }

        Self {
            groups,
            next_pop_id: next_id,
            last_growth: 0,
        }
    }

    #[inline]
    pub fn alloc_pop_id(&mut self) -> PopId {
        if self.next_pop_id == 0 {
            self.next_pop_id = (self.groups.len() as u32) + 1;
        }
        let id = PopId(self.next_pop_id);
        self.next_pop_id += 1;
        id
    }

    /// Total population across all cultures & employers holding occupation `id`.
    #[inline]
    pub fn count_of(&self, id: PopTypeId) -> u32 {
        self.groups
            .iter()
            .filter(|g| g.pop_type == id)
            .map(|g| g.count)
            .sum()
    }

    #[inline]
    pub fn index_by_id(&self, pop_id: PopId) -> Option<usize> {
        self.groups.iter().position(|g| g.id == pop_id)
    }

    #[inline]
    pub fn find_index(
        &self,
        pop_type: PopTypeId,
        culture_id: u16,
        employer: BuildingOwner,
    ) -> Option<usize> {
        self.groups.iter().position(|g| {
            g.pop_type == pop_type && g.culture_id == culture_id && g.employer == employer
        })
    }

    /// Ensures a `(pop_type, culture_id, employer)` row exists in `self.groups` and returns its index.
    pub fn ensure_group_index(
        &mut self,
        good_catalog: &GoodCatalog,
        pop_type: PopTypeId,
        culture_id: u16,
        employer: BuildingOwner,
    ) -> usize {
        if let Some(idx) = self.find_index(pop_type, culture_id, employer) {
            return idx;
        }
        let pid = self.alloc_pop_id();
        let idx = self.groups.len();
        self.groups.push(PopGroup::with_identity(
            good_catalog,
            pid,
            pop_type,
            culture_id,
            employer,
            0,
            0,
            0,
        ));
        idx
    }

    /// Adds an additional culture group with its own unique `PopId`.
    pub fn push_group_with_culture(
        &mut self,
        good_catalog: &GoodCatalog,
        pop_type: PopTypeId,
        culture_id: u16,
        employer: BuildingOwner,
        count: u32,
        initial_cash_credits: i64,
        initial_food_units: i64,
    ) -> PopId {
        let pid = self.alloc_pop_id();
        self.groups.push(PopGroup::with_identity(
            good_catalog,
            pid,
            pop_type,
            culture_id,
            employer,
            count,
            initial_cash_credits,
            initial_food_units,
        ));
        pid
    }

    pub fn total_population(&self) -> u32 {
        self.groups.iter().map(|g| g.count).sum()
    }

    pub fn average_satisfaction(&self) -> u8 {
        let total = self.total_population();
        if total == 0 {
            return 75;
        }
        let weighted: u64 = self
            .groups
            .iter()
            .map(|g| u64::from(g.count) * u64::from(g.satisfaction))
            .sum();
        ((weighted + u64::from(total) / 2) / u64::from(total)).min(100) as u8
    }

    pub fn monthly_growth(&self, habitability_sum_permille: u32, has_core: bool) -> u32 {
        let pop = self.total_population();
        if habitability_sum_permille == 0 || pop == 0 {
            return 0;
        }
        let capacity = (habitability_sum_permille * 140) / 1000 + if has_core { 520 } else { 200 };
        if pop >= capacity {
            return 0;
        }
        let hungry_unowned: u32 = self
            .groups
            .iter()
            .filter(|g| g.employer == BuildingOwner::Unowned && g.last_food_met_permille < 850)
            .map(|g| g.count)
            .sum();
        if hungry_unowned * 5 >= pop {
            return 0;
        }
        let hab_term = (habitability_sum_permille * 4 + 5000) / 10000;
        let pop_term = pop / 220;
        let core_bonus = if has_core { 2 } else { 0 };
        let sat = u32::from(self.average_satisfaction());
        let raw = (hab_term + pop_term + core_bonus).max(2);
        let mut scaled = if sat >= 70 {
            raw + (sat - 70) / 20
        } else if sat < 40 {
            raw / 2
        } else {
            raw
        };
        if hungry_unowned * 10 >= pop {
            scaled = (scaled / 3).max(1);
        }
        scaled.max(1).min(capacity - pop)
    }

    /// Distributes natural `growth` into `default_pool` rows proportionally by each culture's share
    /// of total sector population (preserving multi-culture demographics).
    pub fn apply_natural_growth(
        &mut self,
        good_catalog: &GoodCatalog,
        default_pool: PopTypeId,
        growth: u32,
    ) {
        self.last_growth = growth;
        if growth == 0 {
            return;
        }
        let mut by_culture: BTreeMap<u16, u32> = BTreeMap::new();
        for g in &self.groups {
            if g.count > 0 {
                *by_culture.entry(g.culture_id).or_insert(0) += g.count;
            }
        }
        let total: u32 = by_culture.values().sum();
        if total == 0 {
            let idx =
                self.ensure_group_index(good_catalog, default_pool, 0, BuildingOwner::Unowned);
            self.groups[idx].count = self.groups[idx].count.saturating_add(growth);
            self.groups[idx].sync_capacity();
            return;
        }
        let cultures: Vec<(u16, u32)> = by_culture.into_iter().collect();
        let mut assigned = 0u32;
        for &(cid, c_pop) in &cultures {
            let share = ((u64::from(growth) * u64::from(c_pop)) / u64::from(total)) as u32;
            if share > 0 {
                let idx = self.ensure_group_index(
                    good_catalog,
                    default_pool,
                    cid,
                    BuildingOwner::Unowned,
                );
                self.groups[idx].count += share;
                self.groups[idx].sync_capacity();
                assigned += share;
            }
        }
        if assigned < growth {
            let dominant_cid = cultures
                .iter()
                .max_by_key(|&&(cid, c_pop)| (c_pop, std::cmp::Reverse(cid)))
                .map(|&(cid, _)| cid)
                .unwrap_or(0);
            let idx = self.ensure_group_index(
                good_catalog,
                default_pool,
                dominant_cid,
                BuildingOwner::Unowned,
            );
            self.groups[idx].count += growth - assigned;
            self.groups[idx].sync_capacity();
        }
    }

    /// Transfers `take` people from `groups[src_idx]` into `(target, same_culture)`, carrying
    /// a strictly proportional share of every `available` good (including `base:cash` savings & food!).
    pub fn transfer_between_groups(
        &mut self,
        good_catalog: &GoodCatalog,
        src_idx: usize,
        dst_idx: usize,
        take: u32,
    ) -> u32 {
        if src_idx == dst_idx || take == 0 {
            return 0;
        }
        let src_before = self.groups[src_idx].count;
        let actual = take.min(src_before);
        if actual == 0 {
            return 0;
        }
        let good_len = good_catalog.len();
        self.groups[src_idx].inv.ensure_len(good_len);
        self.groups[dst_idx].inv.ensure_len(good_len);

        // Update counts & physical pantry capacities BEFORE moving goods so dst fits its share!
        self.groups[src_idx].count -= actual;
        self.groups[src_idx].sync_capacity();
        self.groups[dst_idx].count += actual;
        self.groups[dst_idx].sync_capacity();

        for g_i in 0..good_len {
            let gid = GoodId(g_i as u16);
            let src_avail = self.groups[src_idx].inv.avail(gid);
            if src_avail <= 0 {
                continue;
            }
            let carry = if actual == src_before {
                src_avail
            } else {
                ((i128::from(src_avail) * i128::from(actual)) / i128::from(src_before)) as i64
            };
            if carry <= 0 {
                continue;
            }
            let fit = self.groups[dst_idx].inv.max_receivable_units(good_catalog, gid);
            let moved = carry.min(fit).max(0);
            if moved > 0 {
                self.groups[src_idx].inv.available[g_i] -= moved;
                self.groups[dst_idx].inv.available[g_i] += moved;
            }
        }
        actual
    }

    /// Promotes up to `needed` people into `(target, employer)`, preserving each person's `culture_id`
    /// and carrying their proportional household cash & pantry food!
    /// Returns `(dst_pop_id, hired_count)` batches actually transferred.
    pub fn promote_into_employer(
        &mut self,
        good_catalog: &GoodCatalog,
        catalog: &PopTypeCatalog,
        target: PopTypeId,
        employer: BuildingOwner,
        needed: u32,
    ) -> Vec<(PopId, u32)> {
        let mut batches = Vec::new();
        if needed == 0 || usize::from(target.0) >= catalog.len() {
            return batches;
        }
        // Same-occupation between-jobs (`(target, Unowned)`) ALWAYS get re-hired first!
        let mut iter_vec: Vec<PopTypeId> = vec![target];
        for sid in &catalog.get(target).promote_from {
            if let Some(pid) = catalog.id_of(sid) {
                if !iter_vec.contains(&pid) {
                    iter_vec.push(pid);
                }
            }
        }
        if iter_vec.len() == 1 {
            for fb in [catalog.vagrant_pool_id(), catalog.default_pool_id()] {
                if !iter_vec.contains(&fb) {
                    iter_vec.push(fb);
                }
            }
        }

        let mut moved_total = 0u32;
        for &src_type in &iter_vec {
            if moved_total >= needed {
                continue;
            }
            // Only recruit from unattached pools (`employer == Unowned`), never poaching locked staff!
            let cand_indices: Vec<usize> = self
                .groups
                .iter()
                .enumerate()
                .filter_map(|(i, g)| {
                    (g.pop_type == src_type && g.employer == BuildingOwner::Unowned && g.count > 0)
                        .then_some(i)
                })
                .collect();
            for src_idx in cand_indices {
                if moved_total >= needed {
                    break;
                }
                let cid = self.groups[src_idx].culture_id;
                let dst_idx = self.ensure_group_index(good_catalog, target, cid, employer);
                let dst_id = self.groups[dst_idx].id;
                let take = self.groups[src_idx].count.min(needed - moved_total);
                let actual = self.transfer_between_groups(good_catalog, src_idx, dst_idx, take);
                if actual > 0 {
                    moved_total += actual;
                    if let Some(prev) = batches.iter_mut().find(|b| b.0 == dst_id) {
                        prev.1 += actual;
                    } else {
                        batches.push((dst_id, actual));
                    }
                }
            }
        }
        batches
    }

    pub fn promote_into(
        &mut self,
        good_catalog: &GoodCatalog,
        catalog: &PopTypeCatalog,
        target: PopTypeId,
        needed: u32,
    ) -> u32 {
        self.promote_into_employer(good_catalog, catalog, target, BuildingOwner::Unowned, needed)
            .into_iter()
            .map(|(_, c)| c)
            .sum()
    }

    /// Demotes up to `amount` unattached people from `src_type` back into `dst_pool`, preserving `culture_id`
    /// and carrying their proportional household cash & pantry food!
    pub fn demote_into_pool(
        &mut self,
        good_catalog: &GoodCatalog,
        src_type: PopTypeId,
        dst_pool: PopTypeId,
        amount: u32,
    ) -> u32 {
        if amount == 0 || src_type == dst_pool {
            return 0;
        }
        let cand_indices: Vec<usize> = self
            .groups
            .iter()
            .enumerate()
            .filter_map(|(i, g)| {
                (g.pop_type == src_type && g.employer == BuildingOwner::Unowned && g.count > 0)
                    .then_some(i)
            })
            .collect();
        let mut moved_total = 0u32;
        for src_idx in cand_indices {
            if moved_total >= amount {
                break;
            }
            let cid = self.groups[src_idx].culture_id;
            let dst_idx =
                self.ensure_group_index(good_catalog, dst_pool, cid, BuildingOwner::Unowned);
            let take = self.groups[src_idx].count.min(amount - moved_total);
            moved_total += self.transfer_between_groups(good_catalog, src_idx, dst_idx, take);
        }
        moved_total
    }

    /// Releases `amount` workers from a specific employed `PopId` into their OWN occupation's
    /// between-jobs row `(same_pop_type, same_culture_id, BuildingOwner::Unowned)`, carrying their
    /// exact proportional savings & food without mixing into destitute `流民`!
    pub fn release_to_between_jobs(
        &mut self,
        good_catalog: &GoodCatalog,
        src_pop_id: PopId,
        amount: u32,
        satisfaction_penalty: u8,
    ) -> u32 {
        if amount == 0 {
            return 0;
        }
        let Some(src_idx) = self.index_by_id(src_pop_id) else {
            return 0;
        };
        if self.groups[src_idx].employer == BuildingOwner::Unowned {
            return 0;
        }
        let role = self.groups[src_idx].pop_type;
        let cid = self.groups[src_idx].culture_id;
        let src_sat = self.groups[src_idx].satisfaction;
        let dst_idx = self.ensure_group_index(good_catalog, role, cid, BuildingOwner::Unowned);
        let dst_before = self.groups[dst_idx].count;
        let actual = self.transfer_between_groups(good_catalog, src_idx, dst_idx, amount);
        if actual > 0 && satisfaction_penalty > 0 {
            let released_sat = src_sat.saturating_sub(satisfaction_penalty);
            let total_after = dst_before + actual;
            let blended = if dst_before == 0 {
                u64::from(released_sat)
            } else {
                (u64::from(self.groups[dst_idx].satisfaction) * u64::from(dst_before)
                    + u64::from(released_sat) * u64::from(actual))
                    / u64::from(total_after)
            };
            self.groups[dst_idx].satisfaction = blended.clamp(10, 98) as u8;
            self.groups[src_idx].satisfaction =
                self.groups[src_idx].satisfaction.saturating_sub(satisfaction_penalty / 2);
        }
        actual
    }

    /// Removes zero-population, zero-asset rows not referenced by any active `StaffContract`
    /// (keeping baseline `default_pool` primary-culture row tidy).
    pub fn prune_empty_groups(
        &mut self,
        good_catalog: &GoodCatalog,
        default_pool: PopTypeId,
        referenced_ids: &BTreeSet<PopId>,
    ) {
        let cash = good_catalog.cash_id();
        self.groups.retain(|g| {
            g.count > 0
                || referenced_ids.contains(&g.id)
                || g.inv.total(cash) > 0
                || g.inv.used_volume(good_catalog) > 0
                || g.inv.available.iter().any(|&v| v > 0)
                || g.inv.locked.iter().any(|&v| v > 0)
                || (g.culture_id == 0
                    && g.employer == BuildingOwner::Unowned
                    && g.pop_type == default_pool)
        });
    }
}

/// Computes household demand for the primary staple in `good_catalog` (`0` if catalog defines no staple).
#[inline]
pub fn compute_primary_staple_demand(good_catalog: &GoodCatalog, population: u32) -> i64 {
    match good_catalog.primary_staple() {
        Some((_, per_100)) => compute_per_100_demand(population, per_100),
        None => 0,
    }
}

#[inline]
pub fn compute_apple_demand(population: u32) -> i64 {
    compute_per_100_demand(population, 4)
}
