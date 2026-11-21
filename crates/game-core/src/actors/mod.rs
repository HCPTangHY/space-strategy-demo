//! Economic Actors & Tiers (`经济主体与层级模块组`: 家庭 / 工场 / 大区 / 企业托拉斯 / 政权).

use crate::market::CASH_SCALE;

pub const POP_VAGRANT: &str = "base:vagrant";
pub const POP_LABORER: &str = "base:laborer";
pub const POP_ENGINEER: &str = "base:engineer";
pub const POP_ACADEMIC: &str = "base:academic";
pub const POP_BUREAUCRAT: &str = "base:bureaucrat";
pub const POP_SOLDIER: &str = "base:soldier";
pub const POP_CAPITALIST: &str = "base:capitalist";

/// Legacy constants retained for snapshot compatibility; private workshops are no longer hard-locked by count.
pub const BASE_UNINCORPORATED_PRIVATE_CAP: u16 = 2;
pub const CBD_HOSTING_CAPACITY: u16 = 6;
pub const CAMPUS_HOSTING_CAPACITY: u16 = 18;
pub const ENTERPRISE_FOUNDING_MIN_PRIVATE: usize = 3;
pub const ENTERPRISE_OUTPUT_BONUS_PERMILLE: i64 = 150; // +15% output for staffed enterprise buildings
/// Base floor for `SectorCapital` pool ceiling before dynamic service-output scaling (`450 CR`),
/// plus legacy alias `SECTOR_CAPITAL_OVERFLOW_CAP_CENTS`.
pub const BASE_SECTOR_CAPITAL_POOL_CAP_CENTS: i64 = 450 * CASH_SCALE;
pub const SECTOR_CAPITAL_OVERFLOW_CAP_CENTS: i64 = 750 * CASH_SCALE;

pub const WORKSHOP_BASE_CAPACITY: i64 = 120;
pub const BASE_SECTOR_DEPOT_CAPACITY: i64 = 120;
pub const CORE_DEPOT_CAPACITY_BONUS: i64 = 280;
pub const CAPITAL_DEPOT_CAPACITY_BONUS: i64 = 480;
pub const DEPOT_STOCKPILE_TARGET: i64 = 80;

pub mod corporate;
pub mod household;
pub mod ids;
pub mod polity;
pub mod sector;
pub mod snapshots;
pub mod workshop;

pub use corporate::{BilateralSupplyContract, Enterprise, Trust};
pub use household::{
    compute_apple_demand, compute_primary_staple_demand, household_pantry_capacity, PopGroup,
    PopTypeCatalog, PopTypeDef, SectorPops,
};
#[cfg(test)]
pub use household::default_pop_type_defs;
pub use ids::{
    BuildingOwner, CounterpartyId, EnterpriseId, PolityId, PopId, PopTypeId, SectorId, TrustId,
};
pub use polity::{DepotPolicy, MonetaryPolicy, Polity};
pub use sector::{
    derive_sector_trade_links, GeoTransportTopology, Sector, SectorCapital, SectorMarket,
    SectorTradeLink,
};
pub use snapshots::{
    BuildingOwnerSnapshot, EnterpriseSnapshot, GoodInfoSnapshot, PolitySnapshot, PopGroupSnapshot,
    PopTypeSnapshot, RoleWageSnapshot, SectorGoodBookSnapshot, SectorSnapshot,
};
pub use workshop::{StaffContract, UnpaidWageClaim, Workshop};
