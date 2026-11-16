//! Political-economy entities backed by unified `Inventory` containers and `GoodCatalog`:
//! - `PopTypeDef` & `PopTypeCatalog`: pure editable data definitions
//! - `Polity`: sovereign state holding central `vault: Inventory` + `MonetaryPolicy` (central bank inflation lever)
//! - `Sector`: controlled territory holding distributed `state_depot: Inventory`, `SectorOrderBook`,
//!   `SectorCapital` (`vault: Inventory`), and `SectorPops` (each `PopGroup` has its own `inv: Inventory`)
//! - `Workshop`: per-built-slot independent economic agent with its own capacity-bounded `inv: Inventory`

use crate::market::{
    cents_to_credits_round, GoodCatalog, GoodId, Inventory, OrderBookSnapshot, RoleWageBook,
    SectorOrderBook, CASH_SCALE, GENESIS_APPLE_PRICE_CENTS, GOOD_APPLE,
};
use crate::surface::{TileTag, TileTags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const POP_SUBSISTENCE: &str = "base:subsistence";
pub const POP_VAGRANT: &str = "base:vagrant";
pub const POP_LABORER: &str = "base:laborer";
pub const POP_ENGINEER: &str = "base:engineer";
pub const POP_ACADEMIC: &str = "base:academic";
pub const POP_BUREAUCRAT: &str = "base:bureaucrat";
pub const POP_SOLDIER: &str = "base:soldier";
pub const POP_CAPITALIST: &str = "base:capitalist";

pub const CBD_HOSTING_CAPACITY: u16 = 6;
pub const CAMPUS_HOSTING_CAPACITY: u16 = 18;
pub const ENTERPRISE_FOUNDING_MIN_PRIVATE: usize = 3;
pub const ENTERPRISE_OUTPUT_BONUS_PERMILLE: i64 = 150; // +15% output for staffed enterprise buildings

pub const WORKSHOP_BASE_CAPACITY: i64 = 120;
pub const BASE_SECTOR_DEPOT_CAPACITY: i64 = 120;
pub const CORE_DEPOT_CAPACITY_BONUS: i64 = 280;
pub const CAPITAL_DEPOT_CAPACITY_BONUS: i64 = 480;
pub const DEPOT_STOCKPILE_TARGET: i64 = 80;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PolityId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SectorId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EnterpriseId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TrustId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PopTypeId(pub u16);

/// Stable per-sector handle uniquely identifying one `(pop_type, culture_id, employer)` `PopGroup`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PopId(pub u32);

/// Persistent locked labor contract stored on an individual building slot.
/// `monthly_bill_cents` stays locked until `1/24` rolling contract renewal or distress downsizing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffContract {
    pub pop_id: PopId,
    pub count: u32,
    pub monthly_bill_cents: i64,
}

/// Central bank monetary expansion stance (controls new `base:cash` minted into `Polity.vault`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonetaryPolicy {
    /// `0%/yr` strict hard-money cap (zero new `base:cash` minted).
    Tight,
    /// `~3.6%/yr` (`3 permille/month`) gentle liquidity expansion matching growth.
    #[default]
    Moderate,
    /// `~12%/yr` (`10 permille/month`) aggressive fiscal monetization (drives order-book inflation).
    Loose,
}

impl MonetaryPolicy {
    pub const fn monthly_permille(self) -> i64 {
        match self {
            Self::Tight => 0,
            Self::Moderate => 3,
            Self::Loose => 10,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Tight => "紧缩 0%",
            Self::Moderate => "温和通胀 3.6%",
            Self::Loose => "宽松增发 12%",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Tight => Self::Moderate,
            Self::Moderate => Self::Loose,
            Self::Loose => Self::Tight,
        }
    }
}

/// Macro operating stance for a `Sector`'s distributed state depot (avoids micro-trading exploits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepotPolicy {
    /// Hold existing strategic stock without placing market orders.
    #[default]
    Idle,
    /// Gently accumulate up to `DEPOT_STOCKPILE_TARGET` at `MA30` price using central cash.
    Stockpile,
    /// Release stored goods onto the local book slightly below `MA30` to cool local shortages.
    Stabilize,
}

impl DepotPolicy {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "封存待命",
            Self::Stockpile => "备荒收储",
            Self::Stabilize => "开仓平粜",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Idle => Self::Stockpile,
            Self::Stockpile => Self::Stabilize,
            Self::Stabilize => Self::Idle,
        }
    }
}

/// Pure data definition for a Pop occupation (no hardcoded social strata; lifestyle expectation &
/// bankruptcy/demotion chain live purely in data).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopTypeDef {
    pub id: String,
    pub name: String,
    pub base_wage_per_100: i64,
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
    pub promote_from: Vec<String>,
    pub capital_starter_count: u32,
    pub rural_share_permille: u16,
    pub rural_min: u32,
    pub rural_max: u32,
    pub initial_savings: i64,
}

const fn default_expected_sol() -> u8 {
    5
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
        Self::from_defs(default_pop_type_defs()).expect("默认 Pop 数据表必须合法")
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

    pub fn dividend_recipients(&self) -> Vec<PopTypeId> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.receives_dividends.then_some(PopTypeId(i as u16)))
            .collect()
    }
}

pub fn default_pop_type_defs() -> Vec<PopTypeDef> {
    vec![
        PopTypeDef {
            id: POP_SUBSISTENCE.into(),
            name: "个体民".into(),
            base_wage_per_100: 0,
            subsistence_apple_per_100: 6,
            is_default_pool: true,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: false,
            demote_to: None,
            expected_sol: 3,
            promote_from: Vec::new(),
            capital_starter_count: 0,
            rural_share_permille: 0,
            rural_min: 80,
            rural_max: u32::MAX,
            initial_savings: 45,
        },
        PopTypeDef {
            id: POP_VAGRANT.into(),
            name: "流民".into(),
            base_wage_per_100: 0,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: true,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_SUBSISTENCE.into()),
            expected_sol: 1,
            promote_from: Vec::new(),
            capital_starter_count: 15,
            rural_share_permille: 0,
            rural_min: 0,
            rural_max: 0,
            initial_savings: 0,
        },
        PopTypeDef {
            id: POP_LABORER.into(),
            name: "劳工".into(),
            base_wage_per_100: 60,
            subsistence_apple_per_100: 0,
            is_default_pool: false,
            is_vagrant_pool: false,
            receives_dividends: false,
            can_demote: true,
            demote_to: Some(POP_VAGRANT.into()),
            expected_sol: 5,
            promote_from: vec![POP_VAGRANT.into(), POP_SUBSISTENCE.into()],
            capital_starter_count: 350,
            rural_share_permille: 60,
            rural_min: 15,
            rural_max: 40,
            initial_savings: 210,
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
            promote_from: vec![POP_BUREAUCRAT.into(), POP_SUBSISTENCE.into()],
            capital_starter_count: 40,
            rural_share_permille: 0,
            rural_min: 0,
            rural_max: 0,
            initial_savings: 120,
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
            promote_from: vec![
                POP_BUREAUCRAT.into(),
                POP_ENGINEER.into(),
                POP_SUBSISTENCE.into(),
            ],
            capital_starter_count: 40,
            rural_share_permille: 0,
            rural_min: 0,
            rural_max: 0,
            initial_savings: 120,
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
            promote_from: vec![POP_SUBSISTENCE.into(), POP_LABORER.into()],
            capital_starter_count: 105,
            rural_share_permille: 0,
            rural_min: 0,
            rural_max: 0,
            initial_savings: 150,
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
            promote_from: vec![
                POP_LABORER.into(),
                POP_VAGRANT.into(),
                POP_SUBSISTENCE.into(),
            ],
            capital_starter_count: 40,
            rural_share_permille: 0,
            rural_min: 0,
            rural_max: 0,
            initial_savings: 100,
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
            promote_from: vec![
                POP_BUREAUCRAT.into(),
                POP_ENGINEER.into(),
                POP_SUBSISTENCE.into(),
            ],
            capital_starter_count: 15,
            rural_share_permille: 0,
            rural_min: 0,
            rural_max: 0,
            initial_savings: 260,
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
        if let Some(apple_id) = good_catalog.id_of(GOOD_APPLE) {
            inv.produce_capped(good_catalog, apple_id, initial_food_units);
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

/// Per-built-slot independent economic unit with its own capacity-bounded `Inventory`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workshop {
    pub inv: Inventory,
    pub last_unit_cost_cents: i64,
    pub last_output: i64,
    pub last_wage_paid_cents: i64,
    /// Net market cash revenue earned last month (`-1` before the workshop's first settlement cycle).
    #[serde(default = "default_unsettled_revenue")]
    pub last_revenue_cents: i64,
    /// True if last month's actual wage payout fell short of locked `StaffContract` bills.
    #[serde(default)]
    pub had_wage_arrears: bool,
}

const fn default_unsettled_revenue() -> i64 {
    -1
}

impl Workshop {
    pub fn new(good_catalog: &GoodCatalog, scale: u32, starter_cash_cents: i64, starter_apples: i64) -> Self {
        let cap = WORKSHOP_BASE_CAPACITY * i64::from(scale.max(1));
        let mut inv = Inventory::new(good_catalog.len(), cap);
        let cash_id = good_catalog.cash_id();
        inv.available[usize::from(cash_id.0)] = starter_cash_cents.max(0);
        if let Some(apple_id) = good_catalog.id_of(GOOD_APPLE) {
            inv.produce_capped(good_catalog, apple_id, starter_apples);
        }
        Self {
            inv,
            last_unit_cost_cents: (GENESIS_APPLE_PRICE_CENTS * 6) / 10, // 6.00 CR initial unit cost seed
            last_output: 0,
            last_wage_paid_cents: 0,
            last_revenue_cents: -1,
            had_wage_arrears: false,
        }
    }
}

/// Compact ownership tag stored per tile building slot and on employed `PopGroup`s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuildingOwner {
    #[default]
    Unowned,
    State {
        polity_id: PolityId,
    },
    LocalPrivate {
        home_sector: SectorId,
    },
    Enterprise {
        enterprise_id: EnterpriseId,
    },
    Trust {
        trust_id: TrustId,
    },
}

/// Sovereign state ECS component (`vault` holds `base:cash` as an ordinary resource).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Polity {
    pub id: PolityId,
    pub name: String,
    pub currency_good: String,
    pub vault: Inventory,
    pub monetary_policy: MonetaryPolicy,
    pub tax_rate_permille: u16,
    pub last_tax_income_cents: i64,
    pub last_state_enterprise_net_cents: i64,
    pub last_freight_revenue_cents: i64,
    pub last_seigniorage_cents: i64,
    pub last_admin_upkeep_cents: i64,
    pub last_net_cashflow_cents: i64,
}

impl Polity {
    pub fn earth_default(good_catalog: &GoodCatalog) -> Self {
        let mut vault = Inventory::new(good_catalog.len(), i64::MAX / 8);
        let cash_id = good_catalog.cash_id();
        vault.available[usize::from(cash_id.0)] = 2_600 * CASH_SCALE;
        Self {
            id: PolityId(0),
            name: "地球联合政府".into(),
            currency_good: good_catalog.get(cash_id).id.clone(),
            vault,
            monetary_policy: MonetaryPolicy::Moderate,
            tax_rate_permille: 150,
            last_tax_income_cents: 0,
            last_state_enterprise_net_cents: 0,
            last_freight_revenue_cents: 0,
            last_seigniorage_cents: 0,
            last_admin_upkeep_cents: 0,
            last_net_cashflow_cents: 0,
        }
    }

    #[inline]
    pub fn cash_cents(&self, good_catalog: &GoodCatalog) -> i64 {
        self.vault.avail(good_catalog.cash_id())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enterprise {
    pub id: EnterpriseId,
    pub name: String,
    pub industry_tag: String,
    pub hq_sector: SectorId,
    pub campus_tile_index: usize,
    pub hosting_capacity: u16,
    pub last_revenue_cents: i64,
    pub last_profit_cents: i64,
}

/// Controlled sub-regional ECS entity (`实控辖区` + distributed `state_depot`!).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sector {
    pub id: SectorId,
    pub planet_id: String,
    pub geo_region_id: u16,
    pub owner_polity: PolityId,
    pub name: String,
    pub allow_private_land_use: bool,
    pub habitability_sum_permille: u32,
    pub is_coastal: bool,
    pub state_depot: Inventory,
    pub depot_policy: DepotPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorMarket {
    pub apple_book: SectorOrderBook,
    pub wage_books: Vec<RoleWageBook>,
    pub last_production: i64,
    pub last_subsistence_output: i64,
    pub last_consumption: i64,
    /// Order-book depth at the close of the last settlement (resting orders are released afterwards).
    #[serde(skip)]
    pub closing_depth: Option<OrderBookSnapshot>,
}

impl SectorMarket {
    pub fn new(good_catalog: &GoodCatalog, pop_catalog: &PopTypeCatalog) -> Self {
        let apple_id = good_catalog.id_of(GOOD_APPLE).unwrap_or(GoodId(1));
        let genesis = good_catalog.get(apple_id).genesis_price_cents;
        let wage_books = pop_catalog
            .defs()
            .iter()
            .enumerate()
            .map(|(i, d)| RoleWageBook::new(i as u16, d.base_wage_per_100 * CASH_SCALE / 100))
            .collect();
        Self {
            apple_book: SectorOrderBook::new(apple_id, genesis),
            wage_books,
            last_production: 0,
            last_subsistence_output: 0,
            last_consumption: 0,
            closing_depth: None,
        }
    }
}

/// Local private investment pool backed by a unified `Inventory` (`base:cash` in cents).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorCapital {
    pub vault: Inventory,
    pub last_dividends_cents: i64,
    pub last_reinvested_cents: i64,
}

impl SectorCapital {
    pub fn new(good_catalog: &GoodCatalog, initial_credits: i64) -> Self {
        let mut vault = Inventory::new(good_catalog.len(), 0);
        let cash_id = good_catalog.cash_id();
        vault.available[usize::from(cash_id.0)] = initial_credits.max(0) * CASH_SCALE;
        Self {
            vault,
            last_dividends_cents: 0,
            last_reinvested_cents: 0,
        }
    }

    #[inline]
    pub fn pool_cents(&self, good_catalog: &GoodCatalog) -> i64 {
        self.vault.avail(good_catalog.cash_id())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorPops {
    pub groups: Vec<PopGroup>,
    pub next_pop_id: u32,
    pub last_growth: u32,
}

impl SectorPops {
    /// Spawns unattached baseline demographics (`default_pool` + optional `unemployed_pool`)
    /// summing to `initial_population` (or `>= starter_jobs + reserve` in the capital sector).
    /// Employed occupation `PopGroup`s (`employer != Unowned`) are formed when building slots sign contracts!
    pub fn from_catalog(
        good_catalog: &GoodCatalog,
        catalog: &PopTypeCatalog,
        initial_population: u32,
        is_capital_sector: bool,
    ) -> Self {
        let default_pid = catalog.default_pool_id();
        let vag_pid = catalog.vagrant_pool_id();
        let has_vag = catalog.has_dedicated_vagrant_pool();

        if initial_population == 0 {
            return Self {
                groups: vec![PopGroup::with_identity(
                    good_catalog,
                    PopId(1),
                    default_pid,
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

        // Sum `capital_starter_count` so capital total population accommodates starter buildings + subsistence pool!
        let starter_non_default: u32 = catalog
            .defs()
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != usize::from(default_pid.0))
            .map(|(_, d)| d.capital_starter_count)
            .sum();

        let total_target = if is_capital_sector {
            initial_population.max(starter_non_default + 120)
        } else {
            initial_population.max(catalog.get(default_pid).rural_min)
        };

        let mut vag_count = if has_vag {
            let vdef = catalog.get(vag_pid);
            if is_capital_sector {
                vdef.capital_starter_count.min(total_target)
            } else if vdef.rural_share_permille > 0 || vdef.rural_min > 0 {
                let raw = (u64::from(total_target) * u64::from(vdef.rural_share_permille)) / 1000;
                (raw as u32)
                    .clamp(vdef.rural_min, vdef.rural_max)
                    .min(total_target.saturating_sub(catalog.get(default_pid).rural_min))
            } else {
                0
            }
        } else {
            0
        };
        vag_count = vag_count.min(total_target);

        // Rural unowned wage-seeker buffer (e.g. `劳工 · 待业`) when a dedicated vagrant pool exists.
        let mut rural_seekers: Vec<(PopTypeId, u32, i64)> = Vec::new();
        let mut rural_seeker_sum = 0u32;
        if !is_capital_sector && has_vag {
            for (idx, d) in catalog.defs().iter().enumerate() {
                let pid = PopTypeId(idx as u16);
                if pid == default_pid || pid == vag_pid || d.rural_share_permille == 0 {
                    continue;
                }
                let raw = (u64::from(total_target) * u64::from(d.rural_share_permille)) / 1000;
                let max_allow = total_target
                    .saturating_sub(vag_count + rural_seeker_sum + catalog.get(default_pid).rural_min);
                let cnt = (raw as u32).clamp(d.rural_min, d.rural_max).min(max_allow);
                if cnt > 0 {
                    let sav = (d.initial_savings * i64::from(cnt)) / 300;
                    rural_seekers.push((pid, cnt, sav.max(8)));
                    rural_seeker_sum += cnt;
                }
            }
        }

        let sub_count = total_target.saturating_sub(vag_count + rural_seeker_sum);

        let mut groups = Vec::with_capacity(4);
        let mut next_id = 1u32;
        let sub_def = catalog.get(default_pid);
        groups.push(PopGroup::with_identity(
            good_catalog,
            PopId(next_id),
            default_pid,
            0,
            BuildingOwner::Unowned,
            sub_count,
            if sub_count > 0 { sub_def.initial_savings } else { 0 },
            compute_apple_demand(sub_count),
        ));
        next_id += 1;

        if has_vag && vag_count > 0 {
            let vdef = catalog.get(vag_pid);
            let mut g = PopGroup::with_identity(
                good_catalog,
                PopId(next_id),
                vag_pid,
                0,
                BuildingOwner::Unowned,
                vag_count,
                vdef.initial_savings,
                compute_apple_demand(vag_count),
            );
            g.satisfaction = 42;
            groups.push(g);
            next_id += 1;
        }

        for (pid, cnt, sav) in rural_seekers {
            groups.push(PopGroup::with_identity(
                good_catalog,
                PopId(next_id),
                pid,
                0,
                BuildingOwner::Unowned,
                cnt,
                sav,
                compute_apple_demand(cnt),
            ));
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
        let hab_term = (habitability_sum_permille * 4 + 5000) / 10000;
        let pop_term = pop / 220;
        let core_bonus = if has_core { 2 } else { 0 };
        let sat = u32::from(self.average_satisfaction());
        let raw = (hab_term + pop_term + core_bonus).max(2);
        let scaled = if sat >= 70 {
            raw + (sat - 70) / 20
        } else if sat < 40 {
            raw / 2
        } else {
            raw
        };
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
                || (g.culture_id == 0
                    && g.employer == BuildingOwner::Unowned
                    && g.pop_type == default_pool)
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorTradeLink {
    pub sector_a: SectorId,
    pub sector_b: SectorId,
    pub monthly_capacity: i64,
    pub transport_fee: i64,
    pub is_maritime: bool,
}

#[inline]
pub fn compute_apple_demand(population: u32) -> i64 {
    if population == 0 {
        return 0;
    }
    ((i64::from(population) * 4 + 99) / 100).max(1)
}

pub fn derive_sector_trade_links(
    tile_sectors: &[u16],
    tile_tags: &[TileTags],
    tile_neighbors: &[Vec<u16>],
    sector_count: usize,
) -> Vec<SectorTradeLink> {
    if sector_count <= 1 {
        return Vec::new();
    }
    let mut pair_acc: BTreeMap<(u16, u16), (i64, i64, i64, i64)> = BTreeMap::new();

    for (idx, nbs) in tile_neighbors.iter().enumerate() {
        let sa = tile_sectors[idx];
        if sa == u16::MAX {
            continue;
        }
        let ta = tile_tags[idx];
        for &nb_u16 in nbs {
            let nb = usize::from(nb_u16);
            if nb <= idx {
                continue;
            }
            let sb = tile_sectors[nb];
            if sb == u16::MAX || sa == sb {
                continue;
            }
            let tb = tile_tags[nb];
            let key = if sa < sb { (sa, sb) } else { (sb, sa) };
            let entry = pair_acc.entry(key).or_insert((0, 0, 0, 0));

            if !ta.is_water() && !tb.is_water() {
                let (edge_cap, edge_fee) = if ta.has(TileTag::Mountains) || tb.has(TileTag::Mountains)
                {
                    (6, 4)
                } else if ta.has(TileTag::Arid)
                    || tb.has(TileTag::Arid)
                    || ta.has(TileTag::Tundra)
                    || tb.has(TileTag::Tundra)
                {
                    (10, 3)
                } else if ta.has(TileTag::Hills)
                    || tb.has(TileTag::Hills)
                    || ta.has(TileTag::Forest)
                    || tb.has(TileTag::Forest)
                {
                    (16, 2)
                } else {
                    (26, 1)
                };
                entry.0 += edge_cap;
                entry.1 += edge_fee;
                entry.2 += 1;
            } else {
                entry.3 += 1;
            }
        }
    }

    let mut links = Vec::with_capacity(pair_acc.len());
    for ((a, b), (land_cap, fee_sum, land_edges, sea_edges)) in pair_acc {
        if land_edges > 0 {
            let avg_fee = ((fee_sum + land_edges / 2) / land_edges).clamp(1, 5);
            let maritime_bonus = if sea_edges > 0 { 16 } else { 0 };
            links.push(SectorTradeLink {
                sector_a: SectorId(a),
                sector_b: SectorId(b),
                monthly_capacity: (land_cap + maritime_bonus).min(140),
                transport_fee: avg_fee,
                is_maritime: false,
            });
        } else if sea_edges > 0 {
            links.push(SectorTradeLink {
                sector_a: SectorId(a),
                sector_b: SectorId(b),
                monthly_capacity: (sea_edges * 10).clamp(16, 64),
                transport_fee: 2,
                is_maritime: true,
            });
        }
    }
    links
}

// ── UI / Wire Snapshots ─────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PolitySnapshot {
    pub id: u16,
    pub name: String,
    pub treasury: i64,
    pub treasury_cents: i64,
    pub monetary_policy: MonetaryPolicy,
    pub monetary_policy_label: String,
    pub tax_rate_percent: u16,
    pub last_tax_income: i64,
    pub last_state_enterprise_net: i64,
    pub last_freight_revenue: i64,
    pub last_seigniorage: i64,
    pub last_admin_upkeep: i64,
    pub last_net_cashflow: i64,
}

impl Polity {
    pub fn to_snapshot(&self, good_catalog: &GoodCatalog) -> PolitySnapshot {
        let cash_cents = self.vault.total(good_catalog.cash_id());
        PolitySnapshot {
            id: self.id.0,
            name: self.name.clone(),
            treasury: cents_to_credits_round(cash_cents),
            treasury_cents: cash_cents,
            monetary_policy: self.monetary_policy,
            monetary_policy_label: self.monetary_policy.label().to_string(),
            tax_rate_percent: (self.tax_rate_permille + 5) / 10,
            last_tax_income: cents_to_credits_round(self.last_tax_income_cents),
            last_state_enterprise_net: cents_to_credits_round(self.last_state_enterprise_net_cents),
            last_freight_revenue: cents_to_credits_round(self.last_freight_revenue_cents),
            last_seigniorage: cents_to_credits_round(self.last_seigniorage_cents),
            last_admin_upkeep: cents_to_credits_round(self.last_admin_upkeep_cents),
            last_net_cashflow: cents_to_credits_round(self.last_net_cashflow_cents),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PopTypeSnapshot {
    pub id: String,
    pub name: String,
    pub base_wage_per_100: i64,
    pub is_default_pool: bool,
    pub is_vagrant_pool: bool,
    pub receives_dividends: bool,
}

impl PopTypeDef {
    pub fn to_snapshot(&self) -> PopTypeSnapshot {
        PopTypeSnapshot {
            id: self.id.clone(),
            name: self.name.clone(),
            base_wage_per_100: self.base_wage_per_100,
            is_default_pool: self.is_default_pool,
            is_vagrant_pool: self.is_vagrant_pool,
            receives_dividends: self.receives_dividends,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PopGroupSnapshot {
    pub pop_id: u32,
    pub id: String,
    pub culture_id: u16,
    pub employer_kind: String,
    pub employer_label: String,
    pub name: String,
    pub count: u32,
    pub employed: u32,
    /// Locked monthly contract wage per 100 workers in cents (`6000` => `60.00` CR/百人; `0` if non-salaried).
    pub contract_wage_per_100_cents: i64,
    pub savings: i64,
    pub pantry_apples: i64,
    pub pantry_capacity: i64,
    pub satisfaction: u8,
    pub last_income: i64,
    pub food_met_percent: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RoleWageSnapshot {
    pub id: String,
    pub name: String,
    pub wage_per_100_cents: i64,
    pub ma_wage_per_100_cents: i64,
    pub last_hired: u32,
    pub last_laid_off: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EnterpriseSnapshot {
    pub id: u16,
    pub name: String,
    pub industry_tag: String,
    pub hq_sector_id: u16,
    pub campus_tile_index: usize,
    pub hosted_count: u16,
    pub hosting_capacity: u16,
    pub output_bonus_percent: u16,
    pub last_revenue: i64,
    pub last_profit: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BuildingOwnerSnapshot {
    pub kind: String,
    pub label: String,
    pub home_sector_id: Option<u16>,
    pub enterprise_id: Option<u16>,
    pub can_privatize: bool,
    pub privatize_price: i64,
    pub can_nationalize: bool,
    pub nationalize_cost: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectorSnapshot {
    pub id: u16,
    pub geo_region_id: u16,
    pub owner_polity_id: u16,
    pub name: String,
    pub allow_private_land_use: bool,
    pub population: u32,
    pub monthly_growth: u32,
    pub jobs_required: u32,
    pub jobs_filled: u32,
    pub free_labor: u32,
    pub idle_pool_count: u32,
    pub unemployed_count: u32,
    pub vagrant_count: u32,
    pub active_vacancies: u32,
    pub average_satisfaction: u8,
    pub apple_price: i64,
    pub apple_price_cents: i64,
    pub ma_apple_price_cents: i64,
    pub order_book: OrderBookSnapshot,
    pub wage_quotes: Vec<RoleWageSnapshot>,
    pub depot_apples: i64,
    pub depot_capacity: i64,
    pub depot_policy: DepotPolicy,
    pub depot_policy_label: String,
    pub local_apple_stockpile: i64,
    pub apple_supply: i64,
    pub apple_demand: i64,
    pub net_trade_in: i64,
    pub investment_pool: i64,
    pub last_dividends: i64,
    pub cbd_count: u16,
    pub cbd_hosted: u16,
    pub cbd_capacity: u16,
    pub unincorporated_ag_private: u16,
    pub can_found_enterprise: bool,
    pub pops: Vec<PopGroupSnapshot>,
}
