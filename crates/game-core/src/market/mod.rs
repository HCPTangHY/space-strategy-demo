//! Commodity, Security & Labor Market Clearing Subsystems (`市场与仓库原语模块组`).

pub const GOOD_CASH: &str = "base:cash";
pub const GOOD_APPLE: &str = "base:apple";
pub const GOOD_SERVICE: &str = "base:service";
pub const GOOD_ADMIN: &str = "base:admin";

/// Fixed-point scale for `base:cash`: `100` cents = `1.00` credit (`CR`).
pub const CASH_SCALE: i64 = 100;
/// Minimum order-book price increment: `10` cents = `0.10 CR`.
pub const PRICE_TICK_CENTS: i64 = 10;
/// Genesis initial price of `1` apple (`10.00 CR = 1000 cents`), derived from
/// Sraffa recipe labor-time ratio × initial tax numeraire.
pub const GENESIS_APPLE_PRICE_CENTS: i64 = 1_000;
pub const MIN_ORDER_PRICE_CENTS: i64 = 20; // 0.20 CR floor (allows 0.50 CR oversupplied rail/grid fees)
pub const MAX_ORDER_PRICE_CENTS: i64 = 10_000; // 100.00 CR ceiling

/// Intra-sector public long-term contract share (`35%` of eligible regular volume;
/// remaining `65%` trades on the spot order book so marginal prices clear inventory!).
pub const CONTRACT_POOL_SHARE_PERCENT: i64 = 35;

pub mod commodity_clearing;
pub mod inventory;
pub mod labor_clearing;
pub mod order_book;

pub use commodity_clearing::{
    clear_bundled_transport_markets, clear_planetary_good_market, clear_spatial_good_market,
    BundledModeSpec, BundledTransportNetwork,
};
pub use inventory::{
    cents_to_credits_round, compute_per_100_demand, snap_price_cents, transfer_available,
    transfer_from_locked, GoodCatalog, GoodDef, GoodId, Inventory, MarketClearingRule,
    PricingMode, TransportChannel,
};
#[cfg(test)]
pub use inventory::default_good_defs;
pub use labor_clearing::{
    clear_local_labor_book, LaborAsk, LaborBid, LaborFill, RoleWageBook, MAX_WAGE_CENTS,
    MIN_WAGE_CENTS,
};
pub use order_book::{
    release_resting_orders, BuySubmission, HubCliqueRule, LogisticsMatrix, NodeMatrix,
    NodeNetworkSpec, NodeOrderBook, OrderBookSnapshot, PriceLevelSnapshot, RestingOrder,
    SectorOrderBook, SellSubmission, TradeFillRecord,
};

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
        let catalog = GoodCatalog::from_defs(default_good_defs()).unwrap();
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
        assert_eq!(invs[2].avail(cash_id_from(&catalog)), 10 * 500);
        assert_eq!(books[0].last_price_cents, 750);
    }

    fn cash_id_from(c: &GoodCatalog) -> GoodId {
        c.cash_id()
    }
}
