//! Commodity & Security Definitions (`GoodCatalog`) and Conservation `Inventory` Primitives.

use super::{
    CASH_SCALE, CONTRACT_POOL_SHARE_PERCENT, MAX_ORDER_PRICE_CENTS, MIN_ORDER_PRICE_CENTS,
    PRICE_TICK_CENTS,
};
#[cfg(test)]
use super::{GENESIS_APPLE_PRICE_CENTS, GOOD_APPLE, GOOD_CASH, GOOD_SERVICE};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GoodId(pub u16);

/// Pure data attribute describing how a good can move across spatial network nodes (`Sector`s or `Planet`s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportChannel {
    /// Non-tradable outside its local node (`N × N` diagonal only, e.g. municipal services, local labor).
    LocalOnly,
    /// Flow good transmitted over interconnected power grids.
    PowerGrid,
    /// Weightless digital/financial transfer (e.g. `base:cash`, sovereign bonds, `Trust` equity).
    Telecom,
    /// Physical bulk cargo requiring land/maritime/orbital freight capacity on a `NodeMatrix`.
    Physical,
}

/// Customizable spot execution pricing formula for `clear_spatial_good_market`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PricingMode {
    /// Two-step spatial call auction: uniform marginal FOB price per origin node + route cost.
    #[default]
    UniformMarginalFob,
    /// Pay-as-quoted maker ask price (`ask_price_cents + route_cost`).
    MakerQuote,
    /// Regulated / administered fixed origin price (`fixed_price_cents + route_cost`).
    FixedPrice { price_cents: i64 },
}

const fn default_contract_pool_percent() -> u8 {
    CONTRACT_POOL_SHARE_PERCENT as u8
}

const fn default_ma_inertia_weight() -> u8 {
    2
}

/// Data-configurable market clearing institution rules attached to each tradable `GoodDef`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketClearingRule {
    /// Share (`0..=90%`) matched in the intra-node `MA` contract pool before spot call auction (`0` for securities).
    #[serde(default = "default_contract_pool_percent")]
    pub contract_pool_percent: u8,
    /// Spot price determination rule.
    #[serde(default)]
    pub pricing_mode: PricingMode,
    /// Rolling `MA` inertia weight `w` (`new_ma = (old_ma * w + spot_vwap) / (w + 1)`).
    #[serde(default = "default_ma_inertia_weight")]
    pub ma_inertia_weight: u8,
}

impl Default for MarketClearingRule {
    fn default() -> Self {
        Self {
            contract_pool_percent: default_contract_pool_percent(),
            pricing_mode: PricingMode::UniformMarginalFob,
            ma_inertia_weight: default_ma_inertia_weight(),
        }
    }
}

impl MarketClearingRule {
    /// Pure spot call-auction rule without long-term contract pool (used for bonds & `Trust` shares).
    pub const fn spot_security() -> Self {
        Self {
            contract_pool_percent: 0,
            pricing_mode: PricingMode::UniformMarginalFob,
            ma_inertia_weight: 1,
        }
    }
}

const fn default_retention_permille() -> u16 {
    1000
}

/// Pure editable data definition for any resource, flow capacity, standardized claim, or currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoodDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_currency: bool,
    /// Administrative / institutional quota good (`true` for `base:admin`: never creates order books or enters spot clearing).
    #[serde(default)]
    pub non_market: bool,
    /// Physical warehouse space per unit (`0` for `base:cash`, flow goods, and financial claims; `>= 1` for cargo).
    #[serde(default)]
    pub unit_volume: u16,
    /// End-of-month carryover in permille (`1000` = non-perishable, `950` = 5% spoilage, `0` = flow good).
    #[serde(default = "default_retention_permille")]
    pub retention_permille: u16,
    pub transport_channel: TransportChannel,
    pub genesis_price_cents: i64,
    /// Monthly household staple demand per 100 people (`0` = non-staple; `> 0` drives survival `food_met`).
    #[serde(default)]
    pub staple_demand_per_100: i64,
    /// Monthly household service/amenity demand per 100 people (`0` = non-service; `> 0` drives lifestyle satisfaction without causing hunger).
    #[serde(default)]
    pub service_demand_per_100: i64,
    /// Extra satisfaction points granted per `+100%` service consumed above baseline `service_demand_per_100`.
    #[serde(default)]
    pub excess_satisfaction_bonus: i16,
    /// Dynamic `SectorCapital` pool ceiling added (in cents) per unit of this good produced in the sector this month.
    #[serde(default)]
    pub capital_pool_cap_cents_per_unit: i64,
    /// Default per-sector strategic depot stockpile target (`0` = not managed by strategic depot policy).
    #[serde(default)]
    pub strategic_depot_target: i64,
    /// Monthly coupon obligation in permille of `genesis_price_cents` (`> 0` for coupon-bearing bonds/claims).
    #[serde(default)]
    pub coupon_permille_per_month: i64,
    /// Optional named issuer key (`"polity:0"`, `"enterprise:1"`, `"trust:0"`) for standardized debt/equity claims.
    #[serde(default)]
    pub issuer_key: Option<String>,
    /// Customizable market clearing institution parameters for this good's order books.
    #[serde(default)]
    pub market_rule: MarketClearingRule,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoodCatalog {
    defs: Vec<GoodDef>,
    by_id: BTreeMap<String, GoodId>,
    cash_id: GoodId,
}

impl Default for GoodCatalog {
    fn default() -> Self {
        crate::data_bundle::load_economy_v1_bundle()
            .expect("从外部 gamedata/catalog 加载商品数据表失败")
            .goods
    }
}

impl GoodCatalog {
    pub fn from_defs(defs: Vec<GoodDef>) -> Result<Self, String> {
        if defs.is_empty() {
            return Err("商品数据表不能为空。".into());
        }
        let mut by_id = BTreeMap::new();
        let mut cash_id = None;
        for (i, d) in defs.iter().enumerate() {
            if d.id.trim().is_empty() {
                return Err("商品 ID 不能为空。".into());
            }
            let gid = GoodId(i as u16);
            if by_id.insert(d.id.clone(), gid).is_some() {
                return Err(format!("重复的商品 ID：{}", d.id));
            }
            if d.is_currency && cash_id.is_none() {
                cash_id = Some(gid);
            }
        }
        let cash_id = cash_id.ok_or_else(|| "商品数据表至少需要定义一种货币资源。".to_string())?;
        Ok(Self {
            defs,
            by_id,
            cash_id,
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
    pub fn defs(&self) -> &[GoodDef] {
        &self.defs
    }

    #[inline]
    pub fn cash_id(&self) -> GoodId {
        self.cash_id
    }

    #[inline]
    pub fn id_of(&self, str_id: &str) -> Option<GoodId> {
        self.by_id.get(str_id).copied()
    }

    #[inline]
    pub fn get(&self, id: GoodId) -> &GoodDef {
        &self.defs[usize::from(id.0)]
    }

    /// Returns all non-currency tradable `GoodId`s in catalog order (used by `SectorMarket`
    /// and `settlement` to automatically create and clear order books for every good).
    pub fn tradable_good_ids(&self) -> Vec<GoodId> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| (!d.is_currency && !d.non_market).then_some(GoodId(i as u16)))
            .collect()
    }

    /// Returns all household staple goods `(GoodId, demand_per_100)` driven purely by `GoodDef` data.
    pub fn staple_goods(&self) -> Vec<(GoodId, i64)> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                (!d.is_currency && d.staple_demand_per_100 > 0)
                    .then_some((GoodId(i as u16), d.staple_demand_per_100))
            })
            .collect()
    }

    /// Primary household staple good (`first` staple in catalog, if any).
    pub fn primary_staple(&self) -> Option<(GoodId, i64)> {
        self.staple_goods().into_iter().next()
    }

    /// Returns all household service/amenity goods `(GoodId, demand_per_100, excess_satisfaction_bonus)`.
    pub fn service_goods(&self) -> Vec<(GoodId, i64, i16)> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                (!d.is_currency && d.service_demand_per_100 > 0).then_some((
                    GoodId(i as u16),
                    d.service_demand_per_100,
                    d.excess_satisfaction_bonus,
                ))
            })
            .collect()
    }

    /// Returns all goods whose monthly local production expands `SectorCapital` pool ceiling `(GoodId, cents_per_unit)`.
    pub fn pool_cap_goods(&self) -> Vec<(GoodId, i64)> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                (!d.is_currency && d.capital_pool_cap_cents_per_unit > 0)
                    .then_some((GoodId(i as u16), d.capital_pool_cap_cents_per_unit))
            })
            .collect()
    }

    /// Returns all strategic depot goods `(GoodId, target_units)` driven purely by `GoodDef` data.
    pub fn strategic_depot_goods(&self) -> Vec<(GoodId, i64)> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                (!d.is_currency && d.strategic_depot_target > 0)
                    .then_some((GoodId(i as u16), d.strategic_depot_target))
            })
            .collect()
    }

    /// Returns all standardized coupon-bearing claim goods `(GoodId, issuer_key, monthly_coupon_cents_per_unit)`.
    pub fn coupon_claim_goods(&self) -> Vec<(GoodId, String, i64)> {
        self.defs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                if d.is_currency || d.coupon_permille_per_month <= 0 {
                    return None;
                }
                let issuer = d.issuer_key.as_ref()?.clone();
                let coupon_cents =
                    ((d.genesis_price_cents * d.coupon_permille_per_month) / 1000).max(1);
                Some((GoodId(i as u16), issuer, coupon_cents))
            })
            .collect()
    }

    /// Dynamically registers a new standardized good (e.g. sovereign bond or `Trust` equity share).
    /// Hot-patches good definitions in place by string `id`, preserving existing `GoodId`s
    /// (including runtime-issued trust shares & debt claims) and appending newly added goods.
    pub fn patch_in_place(&mut self, incoming: GoodCatalog) -> Result<(), String> {
        let cur_cash_str = &self.defs[usize::from(self.cash_id.0)].id;
        let inc_cash_str = &incoming.defs[usize::from(incoming.cash_id.0)].id;
        if cur_cash_str != inc_cash_str {
            return Err(format!(
                "法定货币 ID 从 {cur_cash_str} 变更为 {inc_cash_str}，请使用「重载重开局」。"
            ));
        }
        for inc_def in incoming.defs {
            if let Some(gid) = self.id_of(&inc_def.id) {
                self.defs[usize::from(gid.0)] = inc_def;
            } else {
                self.register_good(inc_def)?;
            }
        }
        Ok(())
    }

    pub fn register_good(&mut self, def: GoodDef) -> Result<GoodId, String> {
        if def.id.trim().is_empty() {
            return Err("商品 ID 不能为空。".into());
        }
        if self.by_id.contains_key(&def.id) {
            return Err(format!("重复的商品 ID：{}", def.id));
        }
        if self.defs.len() >= usize::from(u16::MAX) {
            return Err("商品数量超出 u16 上限。".into());
        }
        let gid = GoodId(self.defs.len() as u16);
        self.by_id.insert(def.id.clone(), gid);
        self.defs.push(def);
        Ok(gid)
    }
}

#[cfg(test)]
pub fn default_good_defs() -> Vec<GoodDef> {
    vec![
        GoodDef {
            id: GOOD_CASH.into(),
            name: "信用点".into(),
            is_currency: true,
            non_market: false,
            unit_volume: 0,
            retention_permille: 1000,
            transport_channel: TransportChannel::Telecom,
            genesis_price_cents: CASH_SCALE,
            staple_demand_per_100: 0,
            service_demand_per_100: 0,
            excess_satisfaction_bonus: 0,
            capital_pool_cap_cents_per_unit: 0,
            strategic_depot_target: 0,
            coupon_permille_per_month: 0,
            issuer_key: None,
            market_rule: MarketClearingRule::spot_security(),
        },
        GoodDef {
            id: GOOD_APPLE.into(),
            name: "苹果".into(),
            is_currency: false,
            non_market: false,
            unit_volume: 1,
            retention_permille: 960, // 4%/month fresh produce spoilage when hoarded
            transport_channel: TransportChannel::Physical,
            genesis_price_cents: GENESIS_APPLE_PRICE_CENTS,
            staple_demand_per_100: 4,
            service_demand_per_100: 0,
            excess_satisfaction_bonus: 0,
            capital_pool_cap_cents_per_unit: 0,
            strategic_depot_target: 80,
            coupon_permille_per_month: 0,
            issuer_key: None,
            market_rule: MarketClearingRule::default(),
        },
        GoodDef {
            id: GOOD_SERVICE.into(),
            name: "商业服务".into(),
            is_currency: false,
            non_market: false,
            unit_volume: 0,
            retention_permille: 0, // monthly local flow service (clears to 0 at month-end)
            transport_channel: TransportChannel::LocalOnly,
            genesis_price_cents: 400,
            staple_demand_per_100: 0,
            service_demand_per_100: 2,
            excess_satisfaction_bonus: 8,
            capital_pool_cap_cents_per_unit: 40 * CASH_SCALE,
            strategic_depot_target: 0,
            coupon_permille_per_month: 0,
            issuer_key: None,
            market_rule: MarketClearingRule::default(),
        },
    ]
}

/// Computes demand for `population` given a `GoodDef`'s `per_100` consumption rate.
#[inline]
pub fn compute_per_100_demand(population: u32, demand_per_100: i64) -> i64 {
    if population == 0 || demand_per_100 <= 0 {
        return 0;
    }
    ((i64::from(population) * demand_per_100 + 99) / 100).max(1)
}

#[inline]
pub fn snap_price_cents(raw_cents: i64) -> i64 {
    let clamped = raw_cents.clamp(MIN_ORDER_PRICE_CENTS, MAX_ORDER_PRICE_CENTS);
    ((clamped + PRICE_TICK_CENTS / 2) / PRICE_TICK_CENTS) * PRICE_TICK_CENTS
}

#[inline]
pub fn cents_to_credits_round(cents: i64) -> i64 {
    if cents >= 0 {
        (cents + CASH_SCALE / 2) / CASH_SCALE
    } else {
        (cents - CASH_SCALE / 2) / CASH_SCALE
    }
}

/// Universal resource & cash warehouse used identically by `Polity`, `Sector` state depots,
/// `SectorCapital`, every individual `Workshop`, and every `PopGroup`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Unencumbered balances indexed by `GoodId` (`base:cash` stored in cents; physical goods in units).
    pub available: Vec<i64>,
    /// Balances currently frozen in active limit orders on an `OrderBook` (still occupy physical capacity!).
    pub locked: Vec<i64>,
    /// Maximum physical storage volume (`i64::MAX` only if unbounded; physical goods occupy `qty * unit_volume`).
    pub capacity: i64,
}

impl Inventory {
    pub fn new(good_count: usize, capacity: i64) -> Self {
        Self {
            available: vec![0; good_count],
            locked: vec![0; good_count],
            capacity: capacity.max(0),
        }
    }

    pub fn ensure_len(&mut self, good_count: usize) {
        if self.available.len() < good_count {
            self.available.resize(good_count, 0);
        }
        if self.locked.len() < good_count {
            self.locked.resize(good_count, 0);
        }
    }

    #[inline]
    pub fn avail(&self, id: GoodId) -> i64 {
        self.available.get(usize::from(id.0)).copied().unwrap_or(0)
    }

    #[inline]
    pub fn locked_amt(&self, id: GoodId) -> i64 {
        self.locked.get(usize::from(id.0)).copied().unwrap_or(0)
    }

    #[inline]
    pub fn total(&self, id: GoodId) -> i64 {
        self.avail(id) + self.locked_amt(id)
    }

    pub fn used_volume(&self, catalog: &GoodCatalog) -> i64 {
        let mut used = 0i64;
        for (i, def) in catalog.defs().iter().enumerate() {
            if def.unit_volume > 0 {
                let tot = self.available.get(i).copied().unwrap_or(0)
                    + self.locked.get(i).copied().unwrap_or(0);
                used = used.saturating_add(tot.max(0) * i64::from(def.unit_volume));
            }
        }
        used
    }

    pub fn free_volume(&self, catalog: &GoodCatalog) -> i64 {
        (self.capacity - self.used_volume(catalog)).max(0)
    }

    /// Max additional units of `good` that can physically fit into this warehouse.
    pub fn max_receivable_units(&self, catalog: &GoodCatalog, good: GoodId) -> i64 {
        let vol = i64::from(catalog.get(good).unit_volume);
        if vol <= 0 {
            i64::MAX / 4
        } else {
            self.free_volume(catalog) / vol
        }
    }

    /// Produces new goods into `available` (capped by physical warehouse capacity).
    pub fn produce_capped(&mut self, catalog: &GoodCatalog, good: GoodId, raw_amount: i64) -> i64 {
        if raw_amount <= 0 {
            return 0;
        }
        self.ensure_len(catalog.len());
        let fit = self.max_receivable_units(catalog, good).min(raw_amount);
        if fit > 0 {
            self.available[usize::from(good.0)] += fit;
        }
        fit
    }

    /// Consumes up to `amount` from `available` (e.g. Pop eating food or workshop burning inputs).
    pub fn consume_up_to(&mut self, good: GoodId, amount: i64) -> i64 {
        if amount <= 0 {
            return 0;
        }
        let idx = usize::from(good.0);
        if idx >= self.available.len() {
            return 0;
        }
        let actual = self.available[idx].min(amount).max(0);
        self.available[idx] -= actual;
        actual
    }

    /// Applies end-of-month spoilage / zero-carryover decay on `available` physical/flow goods.
    pub fn apply_monthly_retention(&mut self, catalog: &GoodCatalog) {
        for (i, def) in catalog.defs().iter().enumerate() {
            if def.is_currency || def.retention_permille >= 1000 {
                continue;
            }
            if let Some(av) = self.available.get_mut(i) {
                if *av > 0 {
                    *av = (*av * i64::from(def.retention_permille)) / 1000;
                }
            }
        }
    }

    pub fn lock(&mut self, good: GoodId, amount: i64) -> i64 {
        if amount <= 0 {
            return 0;
        }
        let idx = usize::from(good.0);
        if idx >= self.available.len() {
            return 0;
        }
        let actual = self.available[idx].min(amount).max(0);
        self.available[idx] -= actual;
        self.locked[idx] += actual;
        actual
    }

    pub fn unlock(&mut self, good: GoodId, amount: i64) -> i64 {
        if amount <= 0 {
            return 0;
        }
        let idx = usize::from(good.0);
        if idx >= self.locked.len() {
            return 0;
        }
        let actual = self.locked[idx].min(amount).max(0);
        self.locked[idx] -= actual;
        self.available[idx] += actual;
        actual
    }

    pub fn unlock_all(&mut self, good: GoodId) {
        let idx = usize::from(good.0);
        if idx < self.locked.len() {
            let amt = self.locked[idx];
            self.locked[idx] = 0;
            self.available[idx] += amt;
        }
    }
}

/// Atomically transfers up to `amount` of `good` from `inventories[from_idx].available`
/// into `inventories[to_idx].available` (respecting receiver capacity).
pub fn transfer_available(
    catalog: &GoodCatalog,
    inventories: &mut [Inventory],
    from_idx: usize,
    to_idx: usize,
    good: GoodId,
    amount: i64,
) -> i64 {
    if amount <= 0 || from_idx == to_idx {
        return amount.max(0);
    }
    let avail = inventories[from_idx].avail(good);
    let fit = inventories[to_idx].max_receivable_units(catalog, good);
    let moved = amount.min(avail).min(fit).max(0);
    if moved > 0 {
        let g = usize::from(good.0);
        inventories[from_idx].available[g] -= moved;
        inventories[to_idx].available[g] += moved;
    }
    moved
}

/// Atomically transfers `amount` from `inventories[from_idx].locked` into `inventories[to_idx].available`.
pub fn transfer_from_locked(
    inventories: &mut [Inventory],
    from_idx: usize,
    to_idx: usize,
    good: GoodId,
    amount: i64,
) {
    if amount <= 0 {
        return;
    }
    let g = usize::from(good.0);
    debug_assert!(
        inventories[from_idx].locked[g] >= amount,
        "locked balance underflow on good {g}"
    );
    inventories[from_idx].locked[g] -= amount;
    if from_idx == to_idx {
        inventories[to_idx].available[g] += amount;
    } else {
        inventories[to_idx].available[g] += amount;
    }
}
