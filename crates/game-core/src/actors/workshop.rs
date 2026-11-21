//! Tile-Slot Workshop Tier (`单槽工场与欠薪债权层`: `StaffContract`, `UnpaidWageClaim`, `Workshop`).

use super::ids::PopId;
use super::WORKSHOP_BASE_CAPACITY;
use crate::market::{GoodCatalog, Inventory};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffContract {
    pub pop_id: PopId,
    pub count: u32,
    pub monthly_bill_cents: i64,
}

/// Central bank monetary expansion stance (controls new `base:cash` minted into `Polity.vault`).

/// Persistent unpaid wage claim owed by a defaulting workplace to a `PopId`.
/// Senior to owner profit sweeps: must be redeemed in `base:cash` before any dividend/profit extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnpaidWageClaim {
    pub pop_id: PopId,
    pub owed_cents: i64,
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
    /// Consecutive months with unresolved wage arrears (`>= 3` triggers private bankruptcy liquidation).
    #[serde(default)]
    pub consecutive_arrears_months: u8,
    /// Outstanding unpaid wage claims owed to current or departed staff.
    #[serde(default)]
    pub unpaid_claims: Vec<UnpaidWageClaim>,
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
        if let Some((staple_id, _)) = good_catalog.primary_staple() {
            inv.produce_capped(good_catalog, staple_id, starter_apples);
        }
        Self {
            inv,
            last_unit_cost_cents: 0, // unset before first settlement cycle (quotes off `MA`, then tracks real recipe + wage unit cost)
            last_output: 0,
            last_wage_paid_cents: 0,
            last_revenue_cents: -1,
            had_wage_arrears: false,
            consecutive_arrears_months: 0,
            unpaid_claims: Vec::new(),
        }
    }

    pub fn record_unpaid_claim(&mut self, pop_id: PopId, owed_cents: i64) {
        if owed_cents <= 0 {
            return;
        }
        if let Some(c) = self.unpaid_claims.iter_mut().find(|c| c.pop_id == pop_id) {
            c.owed_cents = c.owed_cents.saturating_add(owed_cents);
        } else {
            self.unpaid_claims.push(UnpaidWageClaim { pop_id, owed_cents });
        }
    }

    pub fn total_unpaid_claims_cents(&self) -> i64 {
        self.unpaid_claims.iter().map(|c| c.owed_cents.max(0)).sum()
    }
}
