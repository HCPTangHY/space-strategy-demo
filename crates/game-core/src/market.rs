//! Unified `GoodCatalog` (including `base:cash` as an ordinary resource),
//! capacity-bounded `Inventory` containers, and the `S × S` simultaneous
//! limit-order-book + intra-sector 70% MA contract-pool clearing engine.
//!
//! Zero "Mars market" void resources: every trade atomically transfers real
//! `Good` units and real `base:cash` cents between participant `Inventory`s.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const GOOD_CASH: &str = "base:cash";
pub const GOOD_APPLE: &str = "base:apple";

/// Fixed-point scale for `base:cash`: `100` cents = `1.00` credit (`CR`).
pub const CASH_SCALE: i64 = 100;
/// Minimum order-book price increment: `10` cents = `0.10 CR`.
pub const PRICE_TICK_CENTS: i64 = 10;
/// Genesis initial price of `1` apple (`10.00 CR = 1000 cents`), derived from
/// Sraffa recipe labor-time ratio × initial tax numeraire.
pub const GENESIS_APPLE_PRICE_CENTS: i64 = 1_000;
pub const MIN_ORDER_PRICE_CENTS: i64 = 100; // 1.00 CR floor
pub const MAX_ORDER_PRICE_CENTS: i64 = 10_000; // 100.00 CR ceiling

/// Intra-sector public long-term contract share (`35%` of eligible regular volume;
/// remaining `65%` trades on the spot order book so marginal prices clear inventory!).
pub const CONTRACT_POOL_SHARE_PERCENT: i64 = 35;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GoodId(pub u16);

/// Pure data attribute describing how a good can move across `Sector` boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportChannel {
    /// Non-tradable outside its local `Sector` (e.g. municipal services, housing).
    LocalOnly,
    /// Flow good transmitted over interconnected power grids.
    PowerGrid,
    /// Weightless digital/financial transfer (e.g. `base:cash`, compute, telecom).
    Telecom,
    /// Physical bulk cargo requiring land/maritime/orbital freight capacity.
    Physical,
}

/// Pure editable data definition for any resource, flow capacity, or sovereign currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoodDef {
    pub id: String,
    pub name: String,
    pub is_currency: bool,
    /// Physical warehouse space per unit (`0` for `base:cash` and flow goods; `>= 1` for physical cargo).
    pub unit_volume: u16,
    /// End-of-month carryover in permille (`1000` = non-perishable, `950` = 5% spoilage, `0` = flow good).
    pub retention_permille: u16,
    pub transport_channel: TransportChannel,
    pub genesis_price_cents: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoodCatalog {
    defs: Vec<GoodDef>,
    by_id: BTreeMap<String, GoodId>,
    cash_id: GoodId,
}

impl Default for GoodCatalog {
    fn default() -> Self {
        Self::from_defs(default_good_defs()).expect("默认商品数据表必须合法")
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
}

pub fn default_good_defs() -> Vec<GoodDef> {
    vec![
        GoodDef {
            id: GOOD_CASH.into(),
            name: "信用点".into(),
            is_currency: true,
            unit_volume: 0,
            retention_permille: 1000,
            transport_channel: TransportChannel::Telecom,
            genesis_price_cents: CASH_SCALE,
        },
        GoodDef {
            id: GOOD_APPLE.into(),
            name: "苹果".into(),
            is_currency: false,
            unit_volume: 1,
            retention_permille: 960, // 4%/month fresh produce spoilage when hoarded
            transport_channel: TransportChannel::Physical,
            genesis_price_cents: GENESIS_APPLE_PRICE_CENTS,
        },
    ]
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

/// Identifies an order owner on the book (`goods_inv` + `cash_inv` indices into the clearing slice).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestingOrder {
    pub order_id: u32,
    pub sector_id: u16,
    pub goods_inv: usize,
    pub cash_inv: usize,
    pub price_cents: i64,
    pub remaining_qty: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeFillRecord {
    pub price_cents: i64,
    pub qty: i64,
    pub from_sector: u16,
    pub to_sector: u16,
    pub freight_cents: i64,
}

/// Per-Sector, per-Good Central Limit Order Book (`NVDA/USDT`-style).
/// `last_price_cents` is strictly the execution price of the most recent matched trade!
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorOrderBook {
    pub good: GoodId,
    pub last_price_cents: i64,
    pub prev_price_cents: i64,
    /// 30-day / rolling volume-weighted moving average price used by the intra-sector 70% contract pool.
    pub ma_price_cents: i64,
    pub asks: Vec<RestingOrder>,
    pub bids: Vec<RestingOrder>,
    pub recent_trades: Vec<TradeFillRecord>,
    pub last_contract_volume: i64,
    pub last_spot_volume: i64,
    pub last_net_import: i64,
}

impl SectorOrderBook {
    pub fn new(good: GoodId, genesis_price_cents: i64) -> Self {
        let p = snap_price_cents(genesis_price_cents);
        Self {
            good,
            last_price_cents: p,
            prev_price_cents: p,
            ma_price_cents: p,
            asks: Vec::new(),
            bids: Vec::new(),
            recent_trades: Vec::new(),
            last_contract_volume: 0,
            last_spot_volume: 0,
            last_net_import: 0,
        }
    }

    pub fn best_ask_cents(&self) -> Option<i64> {
        self.asks
            .iter()
            .find(|o| o.remaining_qty > 0)
            .map(|o| o.price_cents)
    }

    pub fn best_bid_cents(&self) -> Option<i64> {
        self.bids
            .iter()
            .find(|o| o.remaining_qty > 0)
            .map(|o| o.price_cents)
    }

    pub fn to_snapshot(&self, max_levels: usize) -> OrderBookSnapshot {
        let mut ask_buckets: BTreeMap<i64, i64> = BTreeMap::new();
        for a in &self.asks {
            if a.remaining_qty > 0 {
                *ask_buckets.entry(a.price_cents).or_insert(0) += a.remaining_qty;
            }
        }
        let mut asks_out = Vec::new();
        let mut cum_ask = 0i64;
        for (&price_cents, &qty) in ask_buckets.iter().take(max_levels) {
            cum_ask += qty;
            asks_out.push(PriceLevelSnapshot {
                price_cents,
                qty,
                cumulative_qty: cum_ask,
            });
        }

        let mut bid_buckets: BTreeMap<i64, i64> = BTreeMap::new();
        for b in &self.bids {
            if b.remaining_qty > 0 {
                *bid_buckets.entry(b.price_cents).or_insert(0) += b.remaining_qty;
            }
        }
        let mut bids_out = Vec::new();
        let mut cum_bid = 0i64;
        for (&price_cents, &qty) in bid_buckets.iter().rev().take(max_levels) {
            cum_bid += qty;
            bids_out.push(PriceLevelSnapshot {
                price_cents,
                qty,
                cumulative_qty: cum_bid,
            });
        }

        OrderBookSnapshot {
            last_price_cents: self.last_price_cents,
            prev_price_cents: self.prev_price_cents,
            ma_price_cents: self.ma_price_cents,
            best_ask_cents: asks_out.first().map(|l| l.price_cents),
            best_bid_cents: bids_out.first().map(|l| l.price_cents),
            asks: asks_out,
            bids: bids_out,
            last_contract_volume: self.last_contract_volume,
            last_spot_volume: self.last_spot_volume,
            last_net_import: self.last_net_import,
            recent_trades: self.recent_trades.iter().rev().take(5).cloned().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PriceLevelSnapshot {
    pub price_cents: i64,
    pub qty: i64,
    pub cumulative_qty: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OrderBookSnapshot {
    pub last_price_cents: i64,
    pub prev_price_cents: i64,
    pub ma_price_cents: i64,
    pub best_ask_cents: Option<i64>,
    pub best_bid_cents: Option<i64>,
    pub asks: Vec<PriceLevelSnapshot>,
    pub bids: Vec<PriceLevelSnapshot>,
    pub last_contract_volume: i64,
    pub last_spot_volume: i64,
    pub last_net_import: i64,
    pub recent_trades: Vec<TradeFillRecord>,
}

/// Submitted sell intention for the simultaneous settlement cycle.
#[derive(Clone, Copy, Debug)]
pub struct SellSubmission {
    pub sector_id: u16,
    pub goods_inv: usize,
    pub cash_inv: usize,
    pub ask_price_cents: i64,
    pub qty: i64,
    pub allow_contract_pool: bool,
}

/// Submitted buy intention for the simultaneous settlement cycle.
#[derive(Clone, Copy, Debug)]
pub struct BuySubmission {
    pub sector_id: u16,
    pub goods_inv: usize,
    pub cash_inv: usize,
    pub bid_price_cents: i64,
    pub qty: i64,
    pub allow_contract_pool: bool,
}

/// Precomputed `S × S` logistics matrix for a planet (`freight_cents[s][d]` & `capacity[s][d]`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogisticsMatrix {
    pub sector_count: usize,
    pub freight_cents: Vec<i64>,
    pub monthly_capacity: Vec<i64>,
}

impl LogisticsMatrix {
    pub fn from_links(
        sector_count: usize,
        direct_edges: &[(usize, usize, i64, i64)],
        coastal_sectors: &[bool],
    ) -> Self {
        let inf = i64::MAX / 8;
        let mut freight = vec![inf; sector_count * sector_count];
        let mut cap = vec![0i64; sector_count * sector_count];

        for i in 0..sector_count {
            freight[i * sector_count + i] = 0;
            cap[i * sector_count + i] = i64::MAX / 8;
        }

        // Direct terrain / coastal adjacency links
        for &(a, b, edge_cap, fee_credits) in direct_edges {
            if a < sector_count && b < sector_count && edge_cap > 0 {
                let f = (fee_credits.max(1)) * CASH_SCALE;
                let idx_ab = a * sector_count + b;
                let idx_ba = b * sector_count + a;
                if f < freight[idx_ab] {
                    freight[idx_ab] = f;
                    freight[idx_ba] = f;
                }
                cap[idx_ab] = cap[idx_ab].max(edge_cap);
                cap[idx_ba] = cap[idx_ba].max(edge_cap);
            }
        }

        // Global maritime shipping lanes between any two coastal sectors (`中国买澳洲铁矿` ocean shipping!)
        for a in 0..sector_count {
            if !coastal_sectors.get(a).copied().unwrap_or(false) {
                continue;
            }
            for b in (a + 1)..sector_count {
                if !coastal_sectors.get(b).copied().unwrap_or(false) {
                    continue;
                }
                let sea_fee = 250; // 2.50 CR ocean shipping rate
                let sea_cap = 90;
                let idx_ab = a * sector_count + b;
                let idx_ba = b * sector_count + a;
                if sea_fee < freight[idx_ab] {
                    freight[idx_ab] = sea_fee;
                    freight[idx_ba] = sea_fee;
                }
                cap[idx_ab] = cap[idx_ab].max(sea_cap);
                cap[idx_ba] = cap[idx_ba].max(sea_cap);
            }
        }

        // Floyd-Warshall multi-hop overland relay so inland sectors can also buy/sell globally
        // with cumulative distance freight & bottleneck capacity!
        for k in 0..sector_count {
            for i in 0..sector_count {
                let ik = freight[i * sector_count + k];
                let cap_ik = cap[i * sector_count + k];
                if ik >= inf || cap_ik <= 0 {
                    continue;
                }
                for j in 0..sector_count {
                    let kj = freight[k * sector_count + j];
                    let cap_kj = cap[k * sector_count + j];
                    if kj >= inf || cap_kj <= 0 {
                        continue;
                    }
                    let cand_f = ik + kj;
                    let cand_c = cap_ik.min(cap_kj);
                    let ij = i * sector_count + j;
                    if cand_f < freight[ij] || (cand_f == freight[ij] && cand_c > cap[ij]) {
                        freight[ij] = cand_f;
                        cap[ij] = cand_c;
                    }
                }
            }
        }

        Self {
            sector_count,
            freight_cents: freight,
            monthly_capacity: cap,
        }
    }

    #[inline]
    pub fn freight(&self, src: usize, dst: usize) -> i64 {
        self.freight_cents[src * self.sector_count + dst]
    }

    #[inline]
    pub fn capacity(&self, src: usize, dst: usize) -> i64 {
        self.monthly_capacity[src * self.sector_count + dst]
    }
}

/// Releases every resting order of the previous settlement cycle back into its owner's
/// `available` balance, so all participants can re-quote their full holdings this cycle.
pub fn release_resting_orders(
    catalog: &GoodCatalog,
    good: GoodId,
    books: &mut [SectorOrderBook],
    inventories: &mut [Inventory],
) {
    let cash_id = catalog.cash_id();
    for book in books.iter_mut() {
        for ask in book.asks.drain(..) {
            if ask.remaining_qty > 0 {
                inventories[ask.goods_inv].unlock(good, ask.remaining_qty);
            }
        }
        for bid in book.bids.drain(..) {
            if bid.remaining_qty > 0 {
                inventories[bid.cash_inv].unlock(cash_id, bid.remaining_qty * bid.price_cents);
            }
        }
    }
}

/// Clears a single tradable `good` across all `S` sectors on a planet simultaneously:
/// 1. Unlocks previous resting orders on `books` back to owner `inventories`;
/// 2. Runs the `70%` intra-sector contract pool at `min(MA30, best_landed_ask)` so cheaper
///    external supply is NEVER blocked by high-cost local producers;
/// 3. Locks remaining asks/bids onto the `S` `SectorOrderBook`s and runs the unified `S × S`
///    (including `s == d` zero-freight diagonal) best-landed-price matching loop;
/// 4. Leaves non-crossing asks & bids resting on `books` as real order-book depth.
pub fn clear_planetary_good_market(
    catalog: &GoodCatalog,
    good: GoodId,
    books: &mut [SectorOrderBook],
    logistics: &LogisticsMatrix,
    inventories: &mut [Inventory],
    freight_receiver_cash_inv: usize,
    mut sells: Vec<SellSubmission>,
    mut buys: Vec<BuySubmission>,
) {
    let cash_id = catalog.cash_id();
    let s_count = books.len();
    if s_count == 0 {
        return;
    }

    #[cfg(debug_assertions)]
    let total_good_before: i64 = inventories.iter().map(|inv| inv.total(good)).sum();
    #[cfg(debug_assertions)]
    let total_cash_before: i64 = inventories.iter().map(|inv| inv.total(cash_id)).sum();

    // 0. Release any unfilled resting orders from the previous settlement cycle.
    for book in books.iter_mut() {
        book.prev_price_cents = book.last_price_cents;
        book.last_contract_volume = 0;
        book.last_spot_volume = 0;
        book.last_net_import = 0;
        book.recent_trades.clear();
        for ask in book.asks.drain(..) {
            if ask.remaining_qty > 0 {
                inventories[ask.goods_inv].unlock(good, ask.remaining_qty);
            }
        }
        for bid in book.bids.drain(..) {
            if bid.remaining_qty > 0 {
                inventories[bid.cash_inv].unlock(cash_id, bid.remaining_qty * bid.price_cents);
            }
        }
    }

    let can_cross_sectors =
        catalog.get(good).transport_channel != TransportChannel::LocalOnly && s_count > 1;
    let mut cap_rem = logistics.monthly_capacity.clone();
    if !can_cross_sectors {
        for s in 0..s_count {
            for d in 0..s_count {
                if s != d {
                    cap_rem[s * s_count + d] = 0;
                }
            }
        }
    }

    // Sanitize & clamp sell submissions to real available physical stock
    for sell in &mut sells {
        sell.ask_price_cents = snap_price_cents(sell.ask_price_cents);
        let avail = inventories[sell.goods_inv].avail(good);
        sell.qty = sell.qty.min(avail).max(0);
    }
    for buy in &mut buys {
        buy.bid_price_cents = snap_price_cents(buy.bid_price_cents);
        buy.qty = buy.qty.max(0);
    }

    // Compute lowest available landed ask into each sector `d` so intra-sector contract pools
    // never force buyers to overpay when cheaper external imports exist!
    let mut best_external_landed = vec![i64::MAX / 4; s_count];
    if can_cross_sectors {
        for sell in &sells {
            if sell.qty <= 0 {
                continue;
            }
            let s = usize::from(sell.sector_id);
            if s >= s_count {
                continue;
            }
            for d in 0..s_count {
                if s != d && cap_rem[s * s_count + d] > 0 {
                    let landed = sell.ask_price_cents + logistics.freight(s, d);
                    if landed < best_external_landed[d] {
                        best_external_landed[d] = landed;
                    }
                }
            }
        }
    }

    // Track ONLY spot-book executions for `ma_price_cents` discovery (never feed `pool_price == old MA`
    // back into `MA`, which previously caused a 70% self-locking feedback loop!).
    let mut spot_notional = vec![0i128; s_count];
    let mut spot_vol = vec![0i64; s_count];

    // 1. Intra-sector Long-Term Contract Pool (`MA` locked price, bounded by external landed imports;
    //    if local spot ask falls below `80% MA`, buyers halve contract take and shift to cheap spot!).
    for sec in 0..s_count {
        let pool_price = snap_price_cents(books[sec].ma_price_cents.min(best_external_landed[sec]));
        let best_local_ask = sells
            .iter()
            .filter(|s| usize::from(s.sector_id) == sec && s.qty > 0)
            .map(|s| s.ask_price_cents)
            .min();
        let buyer_pool_pct = match best_local_ask {
            Some(la) if la * 100 < pool_price * 80 => CONTRACT_POOL_SHARE_PERCENT / 2,
            _ => CONTRACT_POOL_SHARE_PERCENT,
        };

        let mut sec_sell_indices = Vec::new();
        let mut pool_supply = 0i64;
        for (idx, sell) in sells.iter().enumerate() {
            if usize::from(sell.sector_id) == sec
                && sell.allow_contract_pool
                && sell.qty > 0
                && sell.ask_price_cents <= pool_price
            {
                let share = (sell.qty * CONTRACT_POOL_SHARE_PERCENT) / 100;
                if share > 0 {
                    let avail = inventories[sell.goods_inv].avail(good).min(share);
                    if avail > 0 {
                        sec_sell_indices.push((idx, avail));
                        pool_supply += avail;
                    }
                }
            }
        }

        let mut sec_buy_indices = Vec::new();
        let mut pool_demand = 0i64;
        if pool_supply > 0 && buyer_pool_pct > 0 {
            for (idx, buy) in buys.iter().enumerate() {
                if usize::from(buy.sector_id) == sec
                    && buy.allow_contract_pool
                    && buy.qty > 0
                    && buy.bid_price_cents >= pool_price
                {
                    let share = (buy.qty * buyer_pool_pct) / 100;
                    if share > 0 {
                        let fit = inventories[buy.goods_inv].max_receivable_units(catalog, good);
                        let afford = inventories[buy.cash_inv].avail(cash_id) / pool_price;
                        let eff = share.min(fit).min(afford).max(0);
                        if eff > 0 {
                            sec_buy_indices.push((idx, eff));
                            pool_demand += eff;
                        }
                    }
                }
            }
        }

        let matched = pool_supply.min(pool_demand);
        if matched > 0 {
            // Allocate proportionally & settle real goods + cash directly
            let mut rem_to_take = matched;
            let mut seller_cursor = 0usize;
            let mut buyer_allocated: Vec<(usize, i64)> = Vec::with_capacity(sec_buy_indices.len());
            let mut alloc_sum = 0i64;
            for &(b_idx, b_eff) in &sec_buy_indices {
                let q = ((i128::from(b_eff) * i128::from(matched)) / i128::from(pool_demand)) as i64;
                if q > 0 {
                    buyer_allocated.push((b_idx, q));
                    alloc_sum += q;
                }
            }
            for &(b_idx, b_eff) in &sec_buy_indices {
                if alloc_sum >= matched {
                    break;
                }
                let cur_q = buyer_allocated
                    .iter_mut()
                    .find(|x| x.0 == b_idx);
                if let Some(entry) = cur_q {
                    if entry.1 < b_eff {
                        entry.1 += 1;
                        alloc_sum += 1;
                    }
                } else {
                    buyer_allocated.push((b_idx, 1));
                    alloc_sum += 1;
                }
            }

            let mut seller_rem: Vec<(usize, i64)> = sec_sell_indices;
            for (b_idx, mut b_need) in buyer_allocated {
                while b_need > 0 && seller_cursor < seller_rem.len() {
                    let (s_idx, s_avail) = seller_rem[seller_cursor];
                    if s_avail <= 0 {
                        seller_cursor += 1;
                        continue;
                    }
                    let buy_sub = buys[b_idx];
                    let sell_sub = sells[s_idx];
                    let fit = inventories[buy_sub.goods_inv].max_receivable_units(catalog, good);
                    let afford = inventories[buy_sub.cash_inv].avail(cash_id) / pool_price;
                    let step = b_need
                        .min(s_avail)
                        .min(rem_to_take)
                        .min(fit)
                        .min(afford)
                        .min(inventories[sell_sub.goods_inv].avail(good));
                    if step <= 0 {
                        break;
                    }
                    // Lock buyer cash first so real delivery and payment are strictly atomic.
                    let locked_cash =
                        inventories[buy_sub.cash_inv].lock(cash_id, step * pool_price);
                    let payable_step = locked_cash / pool_price;
                    let cash_dust = locked_cash - payable_step * pool_price;
                    if cash_dust > 0 {
                        inventories[buy_sub.cash_inv].unlock(cash_id, cash_dust);
                    }
                    if payable_step <= 0 {
                        break;
                    }
                    let moved = transfer_available(
                        catalog,
                        inventories,
                        sell_sub.goods_inv,
                        buy_sub.goods_inv,
                        good,
                        payable_step,
                    );
                    let unpaid = (payable_step - moved) * pool_price;
                    if unpaid > 0 {
                        inventories[buy_sub.cash_inv].unlock(cash_id, unpaid);
                    }
                    if moved <= 0 {
                        break;
                    }
                    transfer_from_locked(
                        inventories,
                        buy_sub.cash_inv,
                        sell_sub.cash_inv,
                        cash_id,
                        moved * pool_price,
                    );
                    let step = moved;
                    sells[s_idx].qty -= step;
                    buys[b_idx].qty -= step;
                    seller_rem[seller_cursor].1 -= step;
                    b_need -= step;
                    rem_to_take -= step;
                    books[sec].last_contract_volume += step;
                    if seller_rem[seller_cursor].1 == 0 {
                        seller_cursor += 1;
                    }
                }
            }
        }
    }

    // 2. Lock all remaining orders onto their home Sector's OrderBook
    let mut next_order_id = 1u32;
    for sell in sells {
        let s = usize::from(sell.sector_id);
        if s >= s_count || sell.qty <= 0 {
            continue;
        }
        let locked_qty = inventories[sell.goods_inv].lock(good, sell.qty);
        if locked_qty > 0 {
            books[s].asks.push(RestingOrder {
                order_id: next_order_id,
                sector_id: sell.sector_id,
                goods_inv: sell.goods_inv,
                cash_inv: sell.cash_inv,
                price_cents: sell.ask_price_cents,
                remaining_qty: locked_qty,
            });
            next_order_id += 1;
        }
    }

    for buy in buys {
        let d = usize::from(buy.sector_id);
        if d >= s_count || buy.qty <= 0 {
            continue;
        }
        let fit = inventories[buy.goods_inv].max_receivable_units(catalog, good);
        let afford = inventories[buy.cash_inv].avail(cash_id) / buy.bid_price_cents;
        let lockable_qty = buy.qty.min(fit).min(afford).max(0);
        if lockable_qty > 0 {
            let needed_cash = lockable_qty * buy.bid_price_cents;
            let locked_cash = inventories[buy.cash_inv].lock(cash_id, needed_cash);
            let actual_qty = locked_cash / buy.bid_price_cents;
            let dust = locked_cash - actual_qty * buy.bid_price_cents;
            if dust > 0 {
                inventories[buy.cash_inv].unlock(cash_id, dust);
            }
            if actual_qty > 0 {
                books[d].bids.push(RestingOrder {
                    order_id: next_order_id,
                    sector_id: buy.sector_id,
                    goods_inv: buy.goods_inv,
                    cash_inv: buy.cash_inv,
                    price_cents: buy.bid_price_cents,
                    remaining_qty: actual_qty,
                });
                next_order_id += 1;
            }
        }
    }

    for book in books.iter_mut() {
        book.asks
            .sort_by(|a, b| a.price_cents.cmp(&b.price_cents).then_with(|| a.order_id.cmp(&b.order_id)));
        book.bids
            .sort_by(|a, b| b.price_cents.cmp(&a.price_cents).then_with(|| a.order_id.cmp(&b.order_id)));
    }

    let mut ask_head = vec![0usize; s_count];
    let mut bid_head = vec![0usize; s_count];

    // 3. Unified `S × S` Simultaneous Clearing Loop (local `s == d` is just `freight == 0`!)
    loop {
        for s in 0..s_count {
            while ask_head[s] < books[s].asks.len() && books[s].asks[ask_head[s]].remaining_qty <= 0 {
                ask_head[s] += 1;
            }
            while bid_head[s] < books[s].bids.len() && books[s].bids[bid_head[s]].remaining_qty <= 0 {
                bid_head[s] += 1;
            }
        }

        // Pick best `(src_sector, dst_sector)` with highest `bid_price - (ask_price + freight) >= 0`,
        // tie-breaking on lowest `landed_ask` so cheaper external imports beat costly local supply!
        let mut best_pair: Option<(usize, usize, i64, i64, i64)> = None;
        for d in 0..s_count {
            if bid_head[d] >= books[d].bids.len() {
                continue;
            }
            let bid = books[d].bids[bid_head[d]];
            let buyer_fit = inventories[bid.goods_inv].max_receivable_units(catalog, good);
            if buyer_fit <= 0 {
                // Buyer warehouse reached full physical capacity: cancel & refund remaining locked cash!
                let refund = bid.remaining_qty * bid.price_cents;
                inventories[bid.cash_inv].unlock(cash_id, refund);
                books[d].bids[bid_head[d]].remaining_qty = 0;
                continue;
            }

            for s in 0..s_count {
                if ask_head[s] >= books[s].asks.len() {
                    continue;
                }
                let rem_cap = cap_rem[s * s_count + d];
                if rem_cap <= 0 {
                    continue;
                }
                let ask = books[s].asks[ask_head[s]];
                let freight = logistics.freight(s, d);
                if freight >= i64::MAX / 16 {
                    continue;
                }
                let landed_ask = ask.price_cents + freight;
                let spread = bid.price_cents - landed_ask;
                if spread < 0 {
                    continue;
                }

                let replace = match best_pair {
                    None => true,
                    Some((bs, bd, b_spread, b_landed, b_freight)) => {
                        spread > b_spread
                            || (spread == b_spread && landed_ask < b_landed)
                            || (spread == b_spread
                                && landed_ask == b_landed
                                && freight < b_freight)
                            || (spread == b_spread
                                && landed_ask == b_landed
                                && freight == b_freight
                                && (s, d) < (bs, bd))
                    }
                };
                if replace {
                    best_pair = Some((s, d, spread, landed_ask, freight));
                }
            }
        }

        let Some((s, d, _spread, landed_ask, freight)) = best_pair else {
            break;
        };

        let ask = books[s].asks[ask_head[s]];
        let bid = books[d].bids[bid_head[d]];
        let buyer_fit = inventories[bid.goods_inv].max_receivable_units(catalog, good);
        let rem_cap = cap_rem[s * s_count + d];
        let qty = ask
            .remaining_qty
            .min(bid.remaining_qty)
            .min(rem_cap)
            .min(buyer_fit);

        if qty <= 0 {
            if buyer_fit <= 0 {
                let refund = bid.remaining_qty * bid.price_cents;
                inventories[bid.cash_inv].unlock(cash_id, refund);
                books[d].bids[bid_head[d]].remaining_qty = 0;
            }
            break;
        }

        // Execute atomic delivery:
        // 1) Move locked `good` from seller -> buyer's available warehouse
        transfer_from_locked(inventories, ask.goods_inv, bid.goods_inv, good, qty);
        // 2) Move `qty * ask.price_cents` from buyer's locked cash -> seller's available cash
        let seller_proceeds = qty * ask.price_cents;
        transfer_from_locked(inventories, bid.cash_inv, ask.cash_inv, cash_id, seller_proceeds);
        // 3) Move `qty * freight` from buyer's locked cash -> freight receiver
        if freight > 0 {
            transfer_from_locked(
                inventories,
                bid.cash_inv,
                freight_receiver_cash_inv,
                cash_id,
                qty * freight,
            );
        }
        // 4) Refund price-improvement savings `(bid.price_cents - landed_ask) * qty` to buyer!
        let savings = (bid.price_cents - landed_ask) * qty;
        if savings > 0 {
            inventories[bid.cash_inv].unlock(cash_id, savings);
        }

        books[s].asks[ask_head[s]].remaining_qty -= qty;
        books[d].bids[bid_head[d]].remaining_qty -= qty;
        if s != d {
            cap_rem[s * s_count + d] -= qty;
            books[s].last_net_import -= qty;
            books[d].last_net_import += qty;
        }

        // `last_price` is strictly the execution price of the last matched trade!
        // Resting asks are always older than same-cycle bids (makers), so execution happens at the
        // ask (+ freight for imports). Seller books `ask`, buyer books the landed price.
        let exec_price_s = ask.price_cents;
        let exec_price_d = landed_ask;
        books[s].last_price_cents = exec_price_s;
        books[d].last_price_cents = exec_price_d;
        books[s].last_spot_volume += qty;
        if d != s {
            books[d].last_spot_volume += qty;
        }

        spot_notional[s] += i128::from(qty) * i128::from(exec_price_s);
        spot_vol[s] += qty;
        if d != s {
            spot_notional[d] += i128::from(qty) * i128::from(exec_price_d);
            spot_vol[d] += qty;
        }

        let fill = TradeFillRecord {
            price_cents: exec_price_d,
            qty,
            from_sector: s as u16,
            to_sector: d as u16,
            freight_cents: freight,
        };
        books[d].recent_trades.push(fill.clone());
        if s != d {
            books[s].recent_trades.push(fill);
        }
    }

    // Retain only active resting orders on each book and update rolling `ma_price_cents`
    // strictly from marginal SPOT executions (plus resting overhang pressure when supply swamps demand!).
    for sec in 0..s_count {
        books[sec].asks.retain(|o| o.remaining_qty > 0);
        books[sec].bids.retain(|o| o.remaining_qty > 0);

        if spot_vol[sec] > 0 {
            let mut target_px = (spot_notional[sec] / i128::from(spot_vol[sec])) as i64;
            // If resting asks remain lower than `target_px` with zero bids left, blend in best resting ask.
            if books[sec].best_bid_cents().is_none() {
                if let Some(best_a) = books[sec].best_ask_cents() {
                    if best_a < target_px {
                        target_px = (target_px + best_a) / 2;
                    }
                }
            }
            let blended = (books[sec].ma_price_cents * 2 + target_px) / 3;
            books[sec].ma_price_cents = snap_price_cents(blended);
        } else if let (Some(best_a), Some(best_b)) =
            (books[sec].best_ask_cents(), books[sec].best_bid_cents())
        {
            // No cross occurred this period: nudge MA gently toward resting mid-market
            let mid = (best_a + best_b) / 2;
            let blended = (books[sec].ma_price_cents * 4 + mid) / 5;
            books[sec].ma_price_cents = snap_price_cents(blended);
        } else if let Some(best_a) = books[sec].best_ask_cents() {
            if best_a < books[sec].ma_price_cents {
                books[sec].ma_price_cents =
                    snap_price_cents((books[sec].ma_price_cents * 4 + best_a) / 5);
            }
        } else if let Some(best_b) = books[sec].best_bid_cents() {
            if best_b > books[sec].ma_price_cents {
                books[sec].ma_price_cents =
                    snap_price_cents((books[sec].ma_price_cents * 4 + best_b) / 5);
            }
        }
    }

    #[cfg(debug_assertions)]
    {
        let total_good_after: i64 = inventories.iter().map(|inv| inv.total(good)).sum();
        let total_cash_after: i64 = inventories.iter().map(|inv| inv.total(cash_id)).sum();
        debug_assert_eq!(
            total_good_before, total_good_after,
            "Market clearing violated physical good conservation!"
        );
        debug_assert_eq!(
            total_cash_before, total_cash_after,
            "Market clearing violated cash conservation!"
        );
    }
}

// ── Local Intra-Sector Labor Order Book (Marginal Hiring & Contract Renewal) ──

/// Minimum monthly wage per 1 worker in cents (`15` = `15.00 CR` per 100 workers).
pub const MIN_WAGE_CENTS: i64 = 15;
/// Maximum monthly wage per 1 worker in cents (`2000` = `2000.00 CR` per 100 workers).
pub const MAX_WAGE_CENTS: i64 = 2_000;

/// Per-sector, per-occupation local labor order-book state (`LocalOnly` — never crosses sectors).
/// Only marginal job openings (new hires + `1/24` rolling contract renewals) and job-seekers trade here;
/// in-force contracts stay locked on their building slots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleWageBook {
    /// Occupation index (`PopTypeId.0`).
    pub role_index: u16,
    /// Latest executed monthly wage per 1 worker in cents (`60` = `60.00 CR / 100人`).
    pub last_wage_cents: i64,
    pub prev_wage_cents: i64,
    /// Smoothed reference wage per 1 worker in cents (`0` for non-salaried dividend roles).
    pub ma_wage_cents: i64,
    pub last_hired: u32,
    pub last_laid_off: u32,
    pub best_ask_wage_cents: Option<i64>,
    pub best_bid_wage_cents: Option<i64>,
}

impl RoleWageBook {
    pub fn new(role_index: u16, genesis_wage_cents: i64) -> Self {
        let w = if genesis_wage_cents <= 0 {
            0
        } else {
            genesis_wage_cents.clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS)
        };
        Self {
            role_index,
            last_wage_cents: w,
            prev_wage_cents: w,
            ma_wage_cents: w,
            last_hired: 0,
            last_laid_off: 0,
            best_ask_wage_cents: None,
            best_bid_wage_cents: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LaborAsk {
    pub seeker_group_idx: usize,
    pub ask_wage_cents: i64,
    pub available_heads: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct LaborBid {
    pub slot_ref: usize,
    pub bid_wage_cents: i64,
    pub wanted_heads: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct LaborFill {
    pub slot_ref: usize,
    pub seeker_group_idx: usize,
    pub hired: u32,
    pub wage_cents: i64,
}

/// Clears one occupation's local labor book for a single sector:
/// 1) 70% `ma_wage_cents` voluntary contract pool (handles smooth renewals & steady hiring),
/// 2) 30% + unmatched spot limit order book (midpoint call-auction print so employer bidding wars
///    genuinely lift wages and distressed job-seeker underbidding genuinely softens wages).
pub fn clear_local_labor_book(
    book: &mut RoleWageBook,
    asks: &[LaborAsk],
    bids: &[LaborBid],
) -> Vec<LaborFill> {
    book.prev_wage_cents = book.last_wage_cents;
    book.last_hired = 0;
    book.best_ask_wage_cents = None;
    book.best_bid_wage_cents = None;

    let mut fills = Vec::new();
    if asks.is_empty() && bids.is_empty() {
        return fills;
    }

    let mut ask_rem: Vec<u32> = asks.iter().map(|a| a.available_heads).collect();
    let mut bid_rem: Vec<u32> = bids.iter().map(|b| b.wanted_heads).collect();

    // Non-salaried dividend seats (e.g. Capitalist partners in CBD / Campus): match at 0 salary.
    if book.ma_wage_cents <= 0 {
        let mut s_cur = 0usize;
        for (b_i, b_need) in bid_rem.iter_mut().enumerate() {
            while *b_need > 0 && s_cur < ask_rem.len() {
                if ask_rem[s_cur] == 0 {
                    s_cur += 1;
                    continue;
                }
                let take = (*b_need).min(ask_rem[s_cur]);
                *b_need -= take;
                ask_rem[s_cur] -= take;
                book.last_hired += take;
                fills.push(LaborFill {
                    slot_ref: bids[b_i].slot_ref,
                    seeker_group_idx: asks[s_cur].seeker_group_idx,
                    hired: take,
                    wage_cents: 0,
                });
            }
        }
        return fills;
    }

    let ask_px: Vec<i64> = asks
        .iter()
        .map(|a| a.ask_wage_cents.clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS))
        .collect();
    let bid_px: Vec<i64> = bids
        .iter()
        .map(|b| b.bid_wage_cents.clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS))
        .collect();

    let pool_wage = book.ma_wage_cents.clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS);
    let mut spot_notional = 0i128;
    let mut spot_heads = 0u64;

    // Stage 1: MA Contract Pool (voluntary: ask <= pool_wage && bid >= pool_wage)
    let mut ask_pool: Vec<(usize, u32)> = Vec::new();
    let mut supply_pool = 0u32;
    for (i, &rem) in ask_rem.iter().enumerate() {
        if rem > 0 && ask_px[i] <= pool_wage {
            let share = ((u64::from(rem) * CONTRACT_POOL_SHARE_PERCENT as u64 + 50) / 100)
                .min(u64::from(rem)) as u32;
            if share > 0 {
                ask_pool.push((i, share));
                supply_pool += share;
            }
        }
    }

    let mut bid_pool: Vec<(usize, u32)> = Vec::new();
    let mut demand_pool = 0u32;
    if supply_pool > 0 {
        for (j, &rem) in bid_rem.iter().enumerate() {
            if rem > 0 && bid_px[j] >= pool_wage {
                let share = ((u64::from(rem) * CONTRACT_POOL_SHARE_PERCENT as u64 + 50) / 100)
                    .min(u64::from(rem)) as u32;
            if share > 0 {
                    bid_pool.push((j, share));
                    demand_pool += share;
                }
            }
        }
    }

    let matched_pool = supply_pool.min(demand_pool);
    if matched_pool > 0 {
        let mut alloc: Vec<(usize, u32)> = Vec::with_capacity(bid_pool.len());
        let mut alloc_sum = 0u32;
        for &(j, b_share) in &bid_pool {
            let q = ((u64::from(b_share) * u64::from(matched_pool)) / u64::from(demand_pool)) as u32;
            if q > 0 {
                alloc.push((j, q));
                alloc_sum += q;
            }
        }
        for &(j, b_share) in &bid_pool {
            if alloc_sum >= matched_pool {
                break;
            }
            if let Some(entry) = alloc.iter_mut().find(|x| x.0 == j) {
                if entry.1 < b_share {
                    entry.1 += 1;
                    alloc_sum += 1;
                }
            } else {
                alloc.push((j, 1));
                alloc_sum += 1;
            }
        }

        let mut s_cursor = 0usize;
        for (j, mut need) in alloc {
            while need > 0 && s_cursor < ask_pool.len() {
                let (i, s_share) = ask_pool[s_cursor];
                if s_share == 0 {
                    s_cursor += 1;
                    continue;
                }
                let take = need.min(s_share).min(ask_rem[i]).min(bid_rem[j]);
                if take == 0 {
                    s_cursor += 1;
                    continue;
                }
                ask_pool[s_cursor].1 -= take;
                ask_rem[i] -= take;
                bid_rem[j] -= take;
                need -= take;
                book.last_hired += take;
                fills.push(LaborFill {
                    slot_ref: bids[j].slot_ref,
                    seeker_group_idx: asks[i].seeker_group_idx,
                    hired: take,
                    wage_cents: pool_wage,
                });
                if ask_pool[s_cursor].1 == 0 {
                    s_cursor += 1;
                }
            }
        }
    }

    // Stage 2: Spot Limit Order Book (30% + unmatched remainder)
    let mut ask_order: Vec<usize> = (0..asks.len()).filter(|&i| ask_rem[i] > 0).collect();
    ask_order.sort_by(|&a, &b| ask_px[a].cmp(&ask_px[b]).then_with(|| a.cmp(&b)));

    let mut bid_order: Vec<usize> = (0..bids.len()).filter(|&j| bid_rem[j] > 0).collect();
    bid_order.sort_by(|&a, &b| bid_px[b].cmp(&bid_px[a]).then_with(|| a.cmp(&b)));

    let mut a_pos = 0usize;
    let mut b_pos = 0usize;
    while a_pos < ask_order.len() && b_pos < bid_order.len() {
        let i = ask_order[a_pos];
        let j = bid_order[b_pos];
        if ask_rem[i] == 0 {
            a_pos += 1;
            continue;
        }
        if bid_rem[j] == 0 {
            b_pos += 1;
            continue;
        }
        if bid_px[j] < ask_px[i] {
            break;
        }
        let take = ask_rem[i].min(bid_rem[j]);
        // Simultaneous call-auction midpoint lets scarce hiring bids pull wages up and distress asks pull down.
        let exec_wage = ((bid_px[j] + ask_px[i]) / 2).clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS);
        ask_rem[i] -= take;
        bid_rem[j] -= take;
        book.last_hired += take;
        book.last_wage_cents = exec_wage;
        spot_notional += i128::from(take) * i128::from(exec_wage);
        spot_heads += u64::from(take);
        fills.push(LaborFill {
            slot_ref: bids[j].slot_ref,
            seeker_group_idx: asks[i].seeker_group_idx,
            hired: take,
            wage_cents: exec_wage,
        });
    }

    let rest_ask = ask_order
        .into_iter()
        .find(|&i| ask_rem[i] > 0)
        .map(|i| ask_px[i]);
    let rest_bid = bid_order
        .into_iter()
        .find(|&j| bid_rem[j] > 0)
        .map(|j| bid_px[j]);
    book.best_ask_wage_cents = rest_ask;
    book.best_bid_wage_cents = rest_bid;

    if spot_heads > 0 {
        let spot_vwap = (spot_notional / i128::from(spot_heads)) as i64;
        book.ma_wage_cents =
            ((book.ma_wage_cents * 2 + spot_vwap + 1) / 3).clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS);
    } else {
        if matched_pool > 0 {
            book.last_wage_cents = pool_wage;
        }
        if let (Some(ba), Some(bb)) = (rest_ask, rest_bid) {
            let mid = (ba + bb) / 2;
            book.ma_wage_cents =
                ((book.ma_wage_cents * 4 + mid + 2) / 5).clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS);
        } else if let Some(ba) = rest_ask {
            if ba < book.ma_wage_cents {
                book.ma_wage_cents =
                    ((book.ma_wage_cents * 4 + ba + 2) / 5).clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS);
            }
        } else if let Some(bb) = rest_bid {
            if bb > book.ma_wage_cents {
                book.ma_wage_cents =
                    ((book.ma_wage_cents * 4 + bb + 2) / 5).clamp(MIN_WAGE_CENTS, MAX_WAGE_CENTS);
            }
        }
    }

    // Merge fills for the same `(slot_ref, seeker_group_idx, wage_cents)` to keep contracts compact.
    let mut merged: Vec<LaborFill> = Vec::with_capacity(fills.len());
    for f in fills {
        if let Some(prev) = merged.iter_mut().find(|m| {
            m.slot_ref == f.slot_ref
                && m.seeker_group_idx == f.seeker_group_idx
                && m.wage_cents == f.wage_cents
        }) {
            prev.hired += f.hired;
        } else {
            merged.push(f);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv_with(catalog: &GoodCatalog, cash_cents: i64, apples: i64) -> Inventory {
        let mut inv = Inventory::new(catalog.len(), 1_000);
        inv.available[usize::from(catalog.cash_id().0)] = cash_cents;
        inv.produce_capped(catalog, catalog.id_of(GOOD_APPLE).unwrap(), apples);
        inv
    }

    /// A cheaper import (ask 5.00 + freight 2.50 = 7.50 landed) must beat local supply at 9.00,
    /// and the local sector's 70% contract pool must not force the buyer onto the expensive seller.
    #[test]
    fn cheapest_landed_price_wins_over_local_supply() {
        let catalog = GoodCatalog::default();
        let apple = catalog.id_of(GOOD_APPLE).unwrap();
        let logistics = LogisticsMatrix::from_links(2, &[], &[true, true]);
        assert_eq!(logistics.freight(0, 1), 250);

        // 0 = freight receiver, 1 = local seller (sector 0), 2 = remote seller (sector 1), 3 = buyer (sector 0)
        let mut invs = vec![
            inv_with(&catalog, 0, 0),
            inv_with(&catalog, 0, 10),
            inv_with(&catalog, 0, 10),
            inv_with(&catalog, 100_000, 0),
        ];
        let mut books = vec![
            SectorOrderBook::new(apple, 1_000),
            SectorOrderBook::new(apple, 1_000),
        ];
        let sells = vec![
            SellSubmission { sector_id: 0, goods_inv: 1, cash_inv: 1, ask_price_cents: 900, qty: 10, allow_contract_pool: true },
            SellSubmission { sector_id: 1, goods_inv: 2, cash_inv: 2, ask_price_cents: 500, qty: 10, allow_contract_pool: true },
        ];
        let buys = vec![BuySubmission {
            sector_id: 0, goods_inv: 3, cash_inv: 3, bid_price_cents: 1_200, qty: 10, allow_contract_pool: true,
        }];
        let cash_before: i64 = invs.iter().map(|i| i.total(catalog.cash_id())).sum();
        clear_planetary_good_market(&catalog, apple, &mut books, &logistics, &mut invs, 0, sells, buys);
        release_resting_orders(&catalog, apple, &mut books, &mut invs);

        let cash_after: i64 = invs.iter().map(|i| i.total(catalog.cash_id())).sum();
        assert_eq!(cash_before, cash_after);
        assert_eq!(invs[3].avail(apple), 10, "buyer filled");
        assert_eq!(invs[2].avail(apple), 0, "remote seller sold everything");
        assert_eq!(invs[1].avail(apple), 10, "expensive local seller sold nothing");
        assert_eq!(invs[0].avail(catalog.cash_id()), 10 * 250, "freight paid to carrier");
        assert_eq!(invs[2].avail(catalog.cash_id()), 10 * 500);
        assert_eq!(books[0].last_price_cents, 750);
    }
}
