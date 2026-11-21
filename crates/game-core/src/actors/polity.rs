//! Sovereign & Local Polity Tier (`政权、财政金库与货币/官仓政策层`).

use super::ids::{default_credit_rating, PolityId};
use crate::market::{GoodCatalog, Inventory, CASH_SCALE};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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


/// Homogeneous public-authority entity (`中央政府 / 星区总督 / 星球政府 / 大区地方政府`),
/// holding its own independent `vault: Inventory` and optional `parent_polity` tax-sharing link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Polity {
    pub id: PolityId,
    pub name: String,
    #[serde(default)]
    pub parent_polity: Option<PolityId>,
    /// Share (`0..=1000` permille) of taxes collected under this polity remitted directly at source to `parent_polity`.
    #[serde(default)]
    pub upward_tax_share_permille: u16,
    pub currency_good: String,
    pub vault: Inventory,
    pub monetary_policy: MonetaryPolicy,
    pub tax_rate_permille: u16,
    /// Customs tariff charged in cents per imported cargo volume unit from external polities.
    #[serde(default)]
    pub import_tariff_cents_per_vol: i64,
    /// External polities barred from importing physical or grid goods into this polity's sectors.
    #[serde(default)]
    pub embargoed_polities: BTreeSet<PolityId>,
    #[serde(default = "default_credit_rating")]
    pub credit_rating: i16,
    /// Baseline sovereign/municipal direct-rule tile quota before administrative buildings (`base:admin`).
    #[serde(default)]
    pub base_admin_capacity: i64,
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
            parent_polity: None,
            upward_tax_share_permille: 0,
            currency_good: good_catalog.get(cash_id).id.clone(),
            vault,
            monetary_policy: MonetaryPolicy::Moderate,
            tax_rate_permille: 150,
            import_tariff_cents_per_vol: 0,
            embargoed_polities: BTreeSet::new(),
            credit_rating: 100,
            base_admin_capacity: 520,
            last_tax_income_cents: 0,
            last_state_enterprise_net_cents: 0,
            last_freight_revenue_cents: 0,
            last_seigniorage_cents: 0,
            last_admin_upkeep_cents: 0,
            last_net_cashflow_cents: 0,
        }
    }

    pub fn colony_default(good_catalog: &GoodCatalog) -> Self {
        let mut vault = Inventory::new(good_catalog.len(), i64::MAX / 8);
        let cash_id = good_catalog.cash_id();
        vault.available[usize::from(cash_id.0)] = 2_200 * CASH_SCALE;
        Self {
            id: PolityId(0),
            name: "星际拓荒公署".into(),
            parent_polity: None,
            upward_tax_share_permille: 0,
            currency_good: good_catalog.get(cash_id).id.clone(),
            vault,
            monetary_policy: MonetaryPolicy::Moderate,
            tax_rate_permille: 150,
            import_tariff_cents_per_vol: 0,
            embargoed_polities: BTreeSet::new(),
            credit_rating: 100,
            base_admin_capacity: 4,
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
