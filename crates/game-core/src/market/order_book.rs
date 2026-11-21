//! Order-Book Depth State, Spatial Network Matrix (`NodeMatrix`), and Resting Order Release.

use super::inventory::{snap_price_cents, GoodCatalog, GoodId, Inventory};
use super::CASH_SCALE;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// Shared hub clique connectivity rule (e.g. coastal ocean shipping between coastal sectors,
/// or orbital jumpgate / deep-space lanes between starport planets).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubCliqueRule {
    pub eligible_nodes: Vec<bool>,
    pub unit_cost_cents: i64,
    pub monthly_capacity: i64,
}

/// Scale-independent network specification used to synthesize an `N × N` `NodeMatrix`
/// across any set of `N` nodes (sectors on a planet, planets in a star system, or `1 × 1` local).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeNetworkSpec {
    pub node_count: usize,
    /// Symmetric direct links `(node_a, node_b, monthly_capacity, unit_cost_cents)`.
    pub direct_edges_cents: Vec<(usize, usize, i64, i64)>,
    /// Shared hub cliques (every pair of eligible nodes gains this direct route).
    pub hub_cliques: Vec<HubCliqueRule>,
    /// If true, runs Floyd-Warshall min-cost / bottleneck-capacity multi-hop relay synthesis.
    pub allow_multihop_relay: bool,
}

/// Scale-independent `N × N` pairwise interaction & transport matrix (`cost_cents[i][j]` & `capacity[i][j]`).
/// Works identically for `Sector <-> Sector` planetary networks and `Planet <-> Planet` interstellar networks!
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMatrix {
    #[serde(alias = "sector_count")]
    pub node_count: usize,
    #[serde(alias = "freight_cents")]
    pub cost_cents: Vec<i64>,
    pub monthly_capacity: Vec<i64>,
}

/// Type alias preserving ergonomics for planetary sector logistics callers.
pub type LogisticsMatrix = NodeMatrix;
/// Type alias emphasizing scale-independent order books on any spatial node.
pub type NodeOrderBook = SectorOrderBook;

impl NodeMatrix {
    pub const INF_COST: i64 = i64::MAX / 8;

    /// Builds a `LocalOnly` diagonal `N × N` matrix (`0` cross-node capacity).
    pub fn local_only(node_count: usize) -> Self {
        let mut cost = vec![Self::INF_COST; node_count * node_count];
        let mut cap = vec![0i64; node_count * node_count];
        for i in 0..node_count {
            cost[i * node_count + i] = 0;
            cap[i * node_count + i] = Self::INF_COST;
        }
        Self {
            node_count,
            cost_cents: cost,
            monthly_capacity: cap,
        }
    }

    /// Builds a weightless, zero-cost, unbounded `N × N` matrix (e.g. `Telecom` securities/currency).
    pub fn weightless_global(node_count: usize) -> Self {
        Self {
            node_count,
            cost_cents: vec![0; node_count * node_count],
            monthly_capacity: vec![Self::INF_COST; node_count * node_count],
        }
    }

    /// Synthesizes an `N × N` matrix from a customizable `NodeNetworkSpec` (zero hardcoded terrain/sea constants).
    pub fn from_spec(spec: &NodeNetworkSpec) -> Self {
        let n = spec.node_count;
        let inf = Self::INF_COST;
        let mut cost = vec![inf; n * n];
        let mut cap = vec![0i64; n * n];

        for i in 0..n {
            cost[i * n + i] = 0;
            cap[i * n + i] = inf;
        }

        for &(a, b, edge_cap, fee_cents) in &spec.direct_edges_cents {
            if a < n && b < n && a != b && edge_cap > 0 {
                let f = fee_cents.max(0);
                let idx_ab = a * n + b;
                let idx_ba = b * n + a;
                if f < cost[idx_ab] {
                    cost[idx_ab] = f;
                    cost[idx_ba] = f;
                }
                cap[idx_ab] = cap[idx_ab].max(edge_cap);
                cap[idx_ba] = cap[idx_ba].max(edge_cap);
            }
        }

        for clique in &spec.hub_cliques {
            if clique.monthly_capacity <= 0 {
                continue;
            }
            let c_fee = clique.unit_cost_cents.max(0);
            for a in 0..n {
                if !clique.eligible_nodes.get(a).copied().unwrap_or(false) {
                    continue;
                }
                for b in (a + 1)..n {
                    if !clique.eligible_nodes.get(b).copied().unwrap_or(false) {
                        continue;
                    }
                    let idx_ab = a * n + b;
                    let idx_ba = b * n + a;
                    if c_fee < cost[idx_ab] {
                        cost[idx_ab] = c_fee;
                        cost[idx_ba] = c_fee;
                    }
                    cap[idx_ab] = cap[idx_ab].max(clique.monthly_capacity);
                    cap[idx_ba] = cap[idx_ba].max(clique.monthly_capacity);
                }
            }
        }

        if spec.allow_multihop_relay {
            for k in 0..n {
                for i in 0..n {
                    let ik = cost[i * n + k];
                    let cap_ik = cap[i * n + k];
                    if ik >= inf || cap_ik <= 0 {
                        continue;
                    }
                    for j in 0..n {
                        let kj = cost[k * n + j];
                        let cap_kj = cap[k * n + j];
                        if kj >= inf || cap_kj <= 0 {
                            continue;
                        }
                        let cand_f = ik + kj;
                        let cand_c = cap_ik.min(cap_kj);
                        let ij = i * n + j;
                        if cand_f < cost[ij] || (cand_f == cost[ij] && cand_c > cap[ij]) {
                            cost[ij] = cand_f;
                            cap[ij] = cand_c;
                        }
                    }
                }
            }
        }

        Self {
            node_count: n,
            cost_cents: cost,
            monthly_capacity: cap,
        }
    }

    /// Convenience constructor for default planetary overland + coastal shipping parameters.
    pub fn from_links(
        node_count: usize,
        direct_edges_credits: &[(usize, usize, i64, i64)],
        coastal_nodes: &[bool],
    ) -> Self {
        let direct_edges_cents = direct_edges_credits
            .iter()
            .map(|&(a, b, edge_cap, fee_credits)| (a, b, edge_cap, fee_credits.max(1) * CASH_SCALE))
            .collect();
        Self::from_spec(&NodeNetworkSpec {
            node_count,
            direct_edges_cents,
            hub_cliques: vec![HubCliqueRule {
                eligible_nodes: coastal_nodes.to_vec(),
                unit_cost_cents: 250,
                monthly_capacity: 90,
            }],
            allow_multihop_relay: true,
        })
    }

    #[inline]
    pub fn cost(&self, src: usize, dst: usize) -> i64 {
        self.cost_cents[src * self.node_count + dst]
    }

    #[inline]
    pub fn freight(&self, src: usize, dst: usize) -> i64 {
        self.cost(src, dst)
    }

    #[inline]
    pub fn capacity(&self, src: usize, dst: usize) -> i64 {
        self.monthly_capacity[src * self.node_count + dst]
    }

    /// Generic `N × N` pairwise operator: evaluates the identical `kernel(src, dst, state[src], state[dst], cost, cap)`
    /// across all `(src, dst)` pairs (`src != dst`), clamping directed flows to `[0, capacity(src, dst)]`
    /// and optional available outflow `max_outflow_per_node[src]`, returning `(flow_matrix, net_node_deltas)`
    /// with exact zero-sum conservation (`net_node_deltas.iter().sum::<i64>() == 0`)!
    pub fn compute_pairwise_flows<F>(
        &self,
        node_states: &[i64],
        max_outflow_per_node: Option<&[i64]>,
        mut kernel: F,
    ) -> (Vec<i64>, Vec<i64>)
    where
        F: FnMut(usize, usize, i64, i64, i64, i64) -> i64,
    {
        let n = self.node_count;
        let mut flows = vec![0i64; n * n];
        let mut deltas = vec![0i64; n];
        if node_states.len() < n {
            return (flows, deltas);
        }
        for i in 0..n {
            let mut rem_out = max_outflow_per_node
                .and_then(|v| v.get(i).copied())
                .unwrap_or(Self::INF_COST)
                .max(0);
            if rem_out == 0 {
                continue;
            }
            for j in 0..n {
                if i == j {
                    continue;
                }
                let cap = self.capacity(i, j);
                let cost = self.cost(i, j);
                if cap <= 0 || cost >= Self::INF_COST / 2 {
                    continue;
                }
                let raw = kernel(i, j, node_states[i], node_states[j], cost, cap);
                let actual = raw.max(0).min(cap).min(rem_out);
                if actual > 0 {
                    flows[i * n + j] = actual;
                    deltas[i] -= actual;
                    deltas[j] += actual;
                    rem_out -= actual;
                    if rem_out == 0 {
                        break;
                    }
                }
            }
        }
        debug_assert_eq!(deltas.iter().sum::<i64>(), 0, "NodeMatrix pairwise flows must be zero-sum conserved");
        (flows, deltas)
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
