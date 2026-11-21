//! Corporate & Financial Entities Tier (`具名企业、特许托拉斯与双边供货契约层`).

use super::ids::{default_credit_rating, CounterpartyId, EnterpriseId, SectorId, TrustId};
use crate::market::{GoodCatalog, GoodId, Inventory};
use serde::{Deserialize, Serialize};

/// Point-to-point bilateral supply agreement between two named Channel-3 entities (`Polity`, `Enterprise`, `Trust`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilateralSupplyContract {
    pub id: u32,
    pub seller: CounterpartyId,
    pub buyer: CounterpartyId,
    pub sector_id: SectorId,
    pub good: GoodId,
    pub monthly_qty: i64,
    pub unit_price_cents: i64,
    pub remaining_months: u16,
    pub seller_honors: bool,
    pub buyer_honors: bool,
    pub default_count: u16,
}

/// Named corporate entity with its own independent corporate `vault: Inventory` (decoupled from `SectorCapital`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enterprise {
    pub id: EnterpriseId,
    pub name: String,
    pub industry_tag: String,
    pub hq_sector: SectorId,
    pub campus_tile_index: usize,
    pub hosting_capacity: u16,
    pub vault: Inventory,
    #[serde(default = "default_credit_rating")]
    pub credit_rating: i16,
    pub last_revenue_cents: i64,
    pub last_profit_cents: i64,
}

impl Enterprise {
    pub fn new(
        good_catalog: &GoodCatalog,
        id: EnterpriseId,
        name: String,
        industry_tag: String,
        hq_sector: SectorId,
        campus_tile_index: usize,
        hosting_capacity: u16,
        starter_cash_cents: i64,
    ) -> Self {
        let mut vault = Inventory::new(good_catalog.len(), i64::MAX / 8);
        let cash_id = good_catalog.cash_id();
        vault.available[usize::from(cash_id.0)] = starter_cash_cents.max(0);
        Self {
            id,
            name,
            industry_tag,
            hq_sector,
            campus_tile_index,
            hosting_capacity,
            vault,
            credit_rating: 100,
            last_revenue_cents: 0,
            last_profit_cents: 0,
        }
    }

    #[inline]
    pub fn cash_cents(&self, good_catalog: &GoodCatalog) -> i64 {
        self.vault.avail(good_catalog.cash_id())
    }
}

/// Cross-sector / cross-polity conglomerate (`星际托拉斯`) with its own corporate `vault: Inventory`
/// and optional standardized `Telecom` equity good in `GoodCatalog`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trust {
    pub id: TrustId,
    pub name: String,
    pub industry_tag: String,
    pub equity_good: Option<GoodId>,
    pub total_shares: i64,
    pub hosting_capacity: u16,
    pub vault: Inventory,
    #[serde(default = "default_credit_rating")]
    pub credit_rating: i16,
    pub last_revenue_cents: i64,
    pub last_profit_cents: i64,
}

impl Trust {
    pub fn new(
        good_catalog: &GoodCatalog,
        id: TrustId,
        name: String,
        industry_tag: String,
        equity_good: Option<GoodId>,
        total_shares: i64,
        hosting_capacity: u16,
        starter_cash_cents: i64,
    ) -> Self {
        let mut vault = Inventory::new(good_catalog.len(), i64::MAX / 8);
        let cash_id = good_catalog.cash_id();
        vault.available[usize::from(cash_id.0)] = starter_cash_cents.max(0);
        Self {
            id,
            name,
            industry_tag,
            equity_good,
            total_shares,
            hosting_capacity,
            vault,
            credit_rating: 100,
            last_revenue_cents: 0,
            last_profit_cents: 0,
        }
    }

    #[inline]
    pub fn cash_cents(&self, good_catalog: &GoodCatalog) -> i64 {
        self.vault.avail(good_catalog.cash_id())
    }
}
