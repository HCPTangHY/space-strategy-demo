//! Actor & Sector Read-Only Snapshot DTOs.

use super::household::PopTypeDef;
use super::polity::{DepotPolicy, MonetaryPolicy, Polity};
use crate::market::{cents_to_credits_round, GoodCatalog, OrderBookSnapshot};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PolitySnapshot {
    pub id: u16,
    pub name: String,
    pub treasury: i64,
    pub treasury_cents: i64,
    pub credit_rating: i16,
    pub admin_used: i64,
    pub admin_capacity: i64,
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
        self.to_snapshot_with_admin(good_catalog, 0, self.base_admin_capacity)
    }

    pub fn to_snapshot_with_admin(
        &self,
        good_catalog: &GoodCatalog,
        admin_used: i64,
        admin_capacity: i64,
    ) -> PolitySnapshot {
        let cash_cents = self.vault.total(good_catalog.cash_id());
        PolitySnapshot {
            id: self.id.0,
            name: self.name.clone(),
            treasury: cents_to_credits_round(cash_cents),
            treasury_cents: cash_cents,
            credit_rating: self.credit_rating,
            admin_used,
            admin_capacity,
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
    pub treasury: i64,
    pub credit_rating: i16,
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
pub struct GoodInfoSnapshot {
    pub id: String,
    pub name: String,
    pub unit_volume: u16,
    pub retention_permille: u16,
    pub transport_channel: crate::market::TransportChannel,
    pub genesis_price_cents: i64,
    pub is_flow: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectorGoodBookSnapshot {
    pub good_id: String,
    pub good_name: String,
    pub depot_stock: i64,
    pub order_book: OrderBookSnapshot,
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
    pub market_books: Vec<SectorGoodBookSnapshot>,
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
