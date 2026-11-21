//! Spatial Multi-Sector Commodity & Bundled-Transport Call-Auction Clearing Engine.

use super::inventory::{
    snap_price_cents, transfer_available, transfer_from_locked, GoodCatalog, GoodId, Inventory,
    MarketClearingRule, PricingMode, TransportChannel,
};
use super::order_book::{
    BuySubmission, LogisticsMatrix, NodeMatrix, NodeOrderBook, RestingOrder, SectorOrderBook,
    SellSubmission, TradeFillRecord,
};
use super::{MAX_ORDER_PRICE_CENTS, MIN_ORDER_PRICE_CENTS, PRICE_TICK_CENTS};
use std::collections::BTreeMap;

/// Scale-independent `N × N` spatial market clearing across any `N` nodes (sectors or planets),
/// parameterized by `rule: MarketClearingRule` (`contract_pool_percent`, `pricing_mode`, `ma_inertia_weight`).
#[allow(clippy::too_many_arguments)]
pub fn clear_spatial_good_market(
    catalog: &GoodCatalog,
    good: GoodId,
    rule: MarketClearingRule,
    books: &mut [NodeOrderBook],
    network: &NodeMatrix,
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
    let pool_share_pct = i64::from(rule.contract_pool_percent.min(90));
    let ma_weight = i64::from(rule.ma_inertia_weight.max(1));

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

    let channel = catalog.get(good).transport_channel;
    let can_cross_sectors = channel != TransportChannel::LocalOnly && s_count > 1;
    let weightless_channel = matches!(channel, TransportChannel::Telecom | TransportChannel::PowerGrid);
    let mut cap_rem = if weightless_channel {
        vec![i64::MAX / 8; s_count * s_count]
    } else {
        network.monthly_capacity.clone()
    };
    if !can_cross_sectors {
        for s in 0..s_count {
            for d in 0..s_count {
                if s != d {
                    cap_rem[s * s_count + d] = 0;
                }
            }
        }
    }
    let route_freight = |s: usize, d: usize| -> i64 {
        if s == d || weightless_channel {
            0
        } else {
            network.cost(s, d)
        }
    };

    // Sanitize & clamp sell submissions to real available physical stock
    for sell in &mut sells {
        if let PricingMode::FixedPrice { price_cents } = rule.pricing_mode {
            sell.ask_price_cents = snap_price_cents(price_cents);
        } else {
            sell.ask_price_cents = snap_price_cents(sell.ask_price_cents);
        }
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
                    let landed = sell.ask_price_cents + route_freight(s, d);
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
    //    only when cheap `< 80% MA` local spot supply covers `>= 25%` of local buy demand do buyers halve
    //    contract take, so a single tiny bankruptcy liquidation order never disrupts sector contracts!).
    for sec in 0..s_count {
        let pool_price = snap_price_cents(books[sec].ma_price_cents.min(best_external_landed[sec]));
        let cheap_local_qty: i64 = sells
            .iter()
            .filter(|s| {
                usize::from(s.sector_id) == sec
                    && s.qty > 0
                    && s.ask_price_cents * 100 < pool_price * 80
            })
            .map(|s| s.qty)
            .sum();
        let total_local_buy_qty: i64 = buys
            .iter()
            .filter(|b| usize::from(b.sector_id) == sec && b.qty > 0)
            .map(|b| b.qty)
            .sum();
        let buyer_pool_pct = if pool_share_pct <= 0 {
            0
        } else if cheap_local_qty > 0 && cheap_local_qty * 4 >= total_local_buy_qty.max(1) {
            pool_share_pct / 2
        } else {
            pool_share_pct
        };

        let mut sec_sell_indices = Vec::new();
        let mut pool_supply = 0i64;
        if pool_share_pct > 0 {
            for (idx, sell) in sells.iter().enumerate() {
                if usize::from(sell.sector_id) == sec
                    && sell.allow_contract_pool
                    && sell.qty > 0
                    && sell.ask_price_cents <= pool_price
                {
                    let share = (sell.qty * pool_share_pct) / 100;
                    if share > 0 {
                        let avail = inventories[sell.goods_inv].avail(good).min(share);
                        if avail > 0 {
                            sec_sell_indices.push((idx, avail));
                            pool_supply += avail;
                        }
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

    #[derive(Clone, Copy)]
    struct PendingSpotFill {
        s: usize,
        d: usize,
        ask_cash_inv: usize,
        bid_cash_inv: usize,
        ask_price_cents: i64,
        bid_price_cents: i64,
        freight: i64,
        qty: i64,
    }

    let mut ask_head = vec![0usize; s_count];
    let mut bid_head = vec![0usize; s_count];
    let mut pending_fills: Vec<PendingSpotFill> = Vec::new();
    let mut max_filled_ask = vec![0i64; s_count];
    let mut min_filled_netback = vec![i64::MAX; s_count];
    let mut has_spot_fill = vec![false; s_count];

    // 3A. Step 1 of Spatial Call Auction: Cross-sector spread quantity matching (`S × S`).
    // Locks matched quantities & route capacities and moves physical goods immediately so buyer
    // warehouse capacity (`buyer_fit`) stays exact on every step.
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
                let freight = route_freight(s, d);
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

        let Some((s, d, _spread, _landed_ask, freight)) = best_pair else {
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

        // Move physical `good` immediately so `buyer_fit` updates accurately for subsequent matches.
        transfer_from_locked(inventories, ask.goods_inv, bid.goods_inv, good, qty);

        books[s].asks[ask_head[s]].remaining_qty -= qty;
        books[d].bids[bid_head[d]].remaining_qty -= qty;
        if s != d {
            cap_rem[s * s_count + d] -= qty;
            books[s].last_net_import -= qty;
            books[d].last_net_import += qty;
        }

        let netback = bid.price_cents - freight;
        has_spot_fill[s] = true;
        if ask.price_cents > max_filled_ask[s] {
            max_filled_ask[s] = ask.price_cents;
        }
        if netback < min_filled_netback[s] {
            min_filled_netback[s] = netback;
        }

        pending_fills.push(PendingSpotFill {
            s,
            d,
            ask_cash_inv: ask.cash_inv,
            bid_cash_inv: bid.cash_inv,
            ask_price_cents: ask.price_cents,
            bid_price_cents: bid.price_cents,
            freight,
            qty,
        });
    }

    // 3B. Step 2 of Spatial Call Auction: Compute each source node's uniform marginal FOB clearing
    // price `fob_price[s]` within `[max_filled_ask[s], min_filled_netback[s]]` (or apply configured `PricingMode`)!
    let mut fob_price = vec![0i64; s_count];
    for s in 0..s_count {
        if !has_spot_fill[s] {
            continue;
        }
        let lo = max_filled_ask[s];
        let hi = min_filled_netback[s].max(lo);
        let excess_supply = books[s]
            .asks
            .iter()
            .any(|o| o.remaining_qty > 0 && o.price_cents <= lo);
        let mut excess_demand = false;
        for d in 0..s_count {
            if cap_rem[s * s_count + d] <= 0 {
                continue;
            }
            let f = route_freight(s, d);
            if f >= i64::MAX / 16 {
                continue;
            }
            if books[d].bids.iter().any(|b| {
                b.remaining_qty > 0
                    && inventories[b.goods_inv].max_receivable_units(catalog, good) > 0
                    && b.price_cents - f >= hi
            }) {
                excess_demand = true;
                break;
            }
        }
        let raw_fob = if excess_demand && !excess_supply {
            hi
        } else {
            lo
        };
        fob_price[s] = snap_price_cents(raw_fob).clamp(lo, hi);
    }

    for fill in pending_fills {
        let s = fill.s;
        let d = fill.d;
        let exec_price_s = match rule.pricing_mode {
            PricingMode::UniformMarginalFob => fob_price[s],
            PricingMode::MakerQuote => fill.ask_price_cents,
            PricingMode::FixedPrice { price_cents } => {
                snap_price_cents(price_cents).min(fill.bid_price_cents - fill.freight)
            }
        };
        let exec_price_d = exec_price_s + fill.freight;
        debug_assert!(
            exec_price_d <= fill.bid_price_cents,
            "uniform FOB + freight exceeded buyer locked bid!"
        );

        let seller_proceeds = fill.qty * exec_price_s;
        transfer_from_locked(
            inventories,
            fill.bid_cash_inv,
            fill.ask_cash_inv,
            cash_id,
            seller_proceeds,
        );
        if fill.freight > 0 {
            transfer_from_locked(
                inventories,
                fill.bid_cash_inv,
                freight_receiver_cash_inv,
                cash_id,
                fill.qty * fill.freight,
            );
        }
        let savings = (fill.bid_price_cents - exec_price_d) * fill.qty;
        if savings > 0 {
            inventories[fill.bid_cash_inv].unlock(cash_id, savings);
        }

        books[s].last_price_cents = exec_price_s;
        books[d].last_price_cents = exec_price_d;
        books[s].last_spot_volume += fill.qty;
        if d != s {
            books[d].last_spot_volume += fill.qty;
        }

        spot_notional[s] += i128::from(fill.qty) * i128::from(exec_price_s);
        spot_vol[s] += fill.qty;
        if d != s {
            spot_notional[d] += i128::from(fill.qty) * i128::from(exec_price_d);
            spot_vol[d] += fill.qty;
        }

        let record = TradeFillRecord {
            price_cents: exec_price_d,
            qty: fill.qty,
            from_sector: s as u16,
            to_sector: d as u16,
            freight_cents: fill.freight,
        };
        books[d].recent_trades.push(record.clone());
        if s != d {
            books[s].recent_trades.push(record);
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
            let blended = (books[sec].ma_price_cents * ma_weight + target_px) / (ma_weight + 1);
            books[sec].ma_price_cents = snap_price_cents(blended);
        } else if let (Some(best_a), Some(best_b)) =
            (books[sec].best_ask_cents(), books[sec].best_bid_cents())
        {
            // No cross occurred this period: nudge MA gently toward resting mid-market without rounding freeze
            let cur_ma = books[sec].ma_price_cents;
            let mid = (best_a + best_b) / 2;
            let blended = snap_price_cents((cur_ma * 4 + mid) / 5);
            books[sec].ma_price_cents = if mid + PRICE_TICK_CENTS <= cur_ma {
                blended.min(cur_ma - PRICE_TICK_CENTS).max(MIN_ORDER_PRICE_CENTS)
            } else if mid >= cur_ma + PRICE_TICK_CENTS {
                blended.max(cur_ma + PRICE_TICK_CENTS).min(MAX_ORDER_PRICE_CENTS)
            } else {
                blended
            };
        } else if let Some(best_a) = books[sec].best_ask_cents() {
            let cur_ma = books[sec].ma_price_cents;
            let gen_px = catalog.get(good).genesis_price_cents.max(MIN_ORDER_PRICE_CENTS);
            if best_a < cur_ma {
                let blended = snap_price_cents((cur_ma * 4 + best_a) / 5);
                books[sec].ma_price_cents = blended.min(cur_ma - PRICE_TICK_CENTS).max(best_a);
            } else if books[sec].last_contract_volume == 0 && cur_ma > gen_px {
                let decayed = snap_price_cents((cur_ma * 96) / 100).min(cur_ma - PRICE_TICK_CENTS);
                books[sec].ma_price_cents = decayed.max(gen_px);
            }
        } else if let Some(best_b) = books[sec].best_bid_cents() {
            let cur_ma = books[sec].ma_price_cents;
            if best_b > cur_ma {
                let blended = snap_price_cents((cur_ma * 4 + best_b) / 5);
                books[sec].ma_price_cents =
                    blended.max(cur_ma + PRICE_TICK_CENTS).min(best_b).min(MAX_ORDER_PRICE_CENTS);
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

/// Convenience wrapper reading `catalog.get(good).market_rule` and delegating to `clear_spatial_good_market`.
#[allow(clippy::too_many_arguments)]
pub fn clear_planetary_good_market(
    catalog: &GoodCatalog,
    good: GoodId,
    books: &mut [SectorOrderBook],
    logistics: &LogisticsMatrix,
    inventories: &mut [Inventory],
    freight_receiver_cash_inv: usize,
    sells: Vec<SellSubmission>,
    buys: Vec<BuySubmission>,
) {
    let rule = catalog.get(good).market_rule;
    clear_spatial_good_market(
        catalog,
        good,
        rule,
        books,
        logistics,
        inventories,
        freight_receiver_cash_inv,
        sells,
        buys,
    );
}

/// Declarative transport mode descriptor resolved to concrete `GoodId` for bundled clearing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BundledModeSpec {
    pub kind: crate::data_bundle::TransportHubKind,
    pub cap_good: GoodId,
    pub endpoint_shares: i64,
    pub steps_per_extra_share: u16,
    pub max_extra_shares: i64,
}

/// Planetary geography, coast-segment mapping, customs tariffs, and embargo context for `clear_bundled_transport_markets`.
pub struct BundledTransportNetwork<'a> {
    pub sector_count: usize,
    pub sector_geo_region: &'a [u16],
    pub geo: &'a crate::economy::GeoTransportTopology,
    pub modes: &'a [BundledModeSpec],
    /// Maps each `Inventory` index (`ask.goods_inv`) to its port tile's `coast_id` (`u16::MAX` if non-coastal).
    /// When empty (`&[]`), falls back to region-level `geo.sea_distance(ra, rb)`.
    pub inv_coast_id: &'a [u16],
    pub embargoed: &'a [bool],
    pub tariff_per_vol_cents: &'a [i64],
    pub tariff_receiver_inv: &'a [usize],
}

#[inline]
fn snap_price_cents_floor(raw_cents: i64) -> i64 {
    let clamped = raw_cents.clamp(MIN_ORDER_PRICE_CENTS, MAX_ORDER_PRICE_CENTS);
    (clamped / PRICE_TICK_CENTS) * PRICE_TICK_CENTS
}

#[inline]
fn order_matches_coast(o: &RestingOrder, coast_filter: Option<u16>, inv_coast_id: &[u16]) -> bool {
    match coast_filter {
        None => true,
        Some(cid) => inv_coast_id.get(o.goods_inv).copied().unwrap_or(u16::MAX) == cid,
    }
}

/// Quotes `shares_per_unit` from `asks[start..]` (optionally restricted to `coast_filter`) for `1` cargo unit:
/// returns `Some((unit_cost_cents_for_1_cargo, max_cargo_units_available, first_matching_ask_avail))` or `None`.
fn quote_capacity_leg(
    asks: &[RestingOrder],
    start: usize,
    coast_filter: Option<u16>,
    inv_coast_id: &[u16],
    shares_per_unit: i64,
) -> Option<(i64, i64, i64)> {
    if shares_per_unit <= 0 {
        return Some((0, i64::MAX / 8, i64::MAX / 8));
    }
    let mut total_avail = 0i64;
    let mut first_avail = 0i64;
    let mut seen_first = false;
    let mut rem_for_one = shares_per_unit;
    let mut max_ask_for_one = 0i64;
    for o in asks.iter().skip(start) {
        if o.remaining_qty <= 0 || !order_matches_coast(o, coast_filter, inv_coast_id) {
            continue;
        }
        if !seen_first {
            first_avail = o.remaining_qty;
            seen_first = true;
        }
        total_avail += o.remaining_qty;
        if rem_for_one > 0 {
            max_ask_for_one = max_ask_for_one.max(o.price_cents);
            rem_for_one -= o.remaining_qty.min(rem_for_one);
        }
    }
    if total_avail < shares_per_unit {
        None
    } else {
        Some((
            shares_per_unit * max_ask_for_one,
            total_avail / shares_per_unit,
            first_avail,
        ))
    }
}

/// Multi-good spatial call auction with **real order-book transport capacity consumption** and
/// **two-step bottleneck clearing price discovery**:
/// - Hubs post their produced capacity flow goods (`base:cap_land`, `base:cap_sea`, `base:cap_air`, `base:cap_grid`)
///   as ordinary `asks` on their home sector's order book;
/// - Cross-sector trades of `Physical` and `PowerGrid` goods atomically match origin good `asks` together
///   with route capacity `asks` (prioritizing highest net value per capacity share);
/// - Non-sold-out markets clear at seller ask floor `lo`, while sold-out bottleneck markets (`remaining_qty == 0`
///   with unmet buyer demand) clear up to the marginal buyer's net willingness-to-pay `hi`, crediting hub
///   `Workshop`s directly and updating each sector's capacity `ma_price_cents`.
pub fn clear_bundled_transport_markets(
    catalog: &GoodCatalog,
    net: &BundledTransportNetwork<'_>,
    good_books: &mut BTreeMap<GoodId, Vec<SectorOrderBook>>,
    mut orders_by_good: BTreeMap<GoodId, (Vec<SellSubmission>, Vec<BuySubmission>)>,
    inventories: &mut [Inventory],
    polity_freight_rev_by_inv: &mut BTreeMap<usize, i64>,
) {
    use crate::data_bundle::TransportHubKind;

    let s_count = net.sector_count;
    if s_count == 0 {
        return;
    }
    let cash_id = catalog.cash_id();

    let mut cap_gids: Vec<GoodId> = net.modes.iter().map(|m| m.cap_good).collect();
    cap_gids.sort_unstable();
    cap_gids.dedup();

    #[cfg(debug_assertions)]
    let total_cash_before: i64 = inventories.iter().map(|inv| inv.total(cash_id)).sum();
    #[cfg(debug_assertions)]
    let non_cap_totals_before: BTreeMap<GoodId, i64> = orders_by_good
        .keys()
        .copied()
        .filter(|g| !cap_gids.contains(g))
        .map(|g| (g, inventories.iter().map(|inv| inv.total(g)).sum()))
        .collect();

    // 0. Reset books & release any stale resting orders for all participating goods
    let participating_gids: Vec<GoodId> = orders_by_good.keys().copied().collect();
    for &gid in &participating_gids {
        if let Some(books) = good_books.get_mut(&gid) {
            for book in books.iter_mut() {
                book.prev_price_cents = book.last_price_cents;
                book.last_contract_volume = 0;
                book.last_spot_volume = 0;
                book.last_net_import = 0;
                book.recent_trades.clear();
                for ask in book.asks.drain(..) {
                    if ask.remaining_qty > 0 {
                        inventories[ask.goods_inv].unlock(gid, ask.remaining_qty);
                    }
                }
                for bid in book.bids.drain(..) {
                    if bid.remaining_qty > 0 {
                        inventories[bid.cash_inv]
                            .unlock(cash_id, bid.remaining_qty * bid.price_cents);
                    }
                }
            }
        }
    }

    // Sanitize all sell/buy submissions
    for (&gid, (sells, buys)) in orders_by_good.iter_mut() {
        let rule = catalog.get(gid).market_rule;
        for sell in sells.iter_mut() {
            if let PricingMode::FixedPrice { price_cents } = rule.pricing_mode {
                sell.ask_price_cents = snap_price_cents(price_cents);
            } else {
                sell.ask_price_cents = snap_price_cents(sell.ask_price_cents);
            }
            let avail = inventories[sell.goods_inv].avail(gid);
            sell.qty = sell.qty.min(avail).max(0);
        }
        for buy in buys.iter_mut() {
            buy.bid_price_cents = snap_price_cents(buy.bid_price_cents);
            buy.qty = buy.qty.max(0);
        }
    }

    // 1. Lock capacity flow goods onto their home `SectorOrderBook`s first (clearing any optional local buys)
    let mut spot_notional: BTreeMap<GoodId, Vec<i128>> = participating_gids
        .iter()
        .map(|&g| (g, vec![0i128; s_count]))
        .collect();
    let mut spot_vol: BTreeMap<GoodId, Vec<i64>> = participating_gids
        .iter()
        .map(|&g| (g, vec![0i64; s_count]))
        .collect();

    for &cap_gid in &cap_gids {
        let Some((cap_sells, cap_buys)) = orders_by_good.remove(&cap_gid) else {
            continue;
        };
        let Some(books) = good_books.get_mut(&cap_gid) else {
            continue;
        };
        if !cap_buys.is_empty() {
            // Clear optional intra-sector passenger/local demand first, keeping unfilled `asks` locked!
            let local_mat = NodeMatrix::local_only(s_count);
            clear_planetary_good_market(
                catalog,
                cap_gid,
                books,
                &local_mat,
                inventories,
                net.tariff_receiver_inv.first().copied().unwrap_or(0),
                cap_sells,
                cap_buys,
            );
            // Release any unfilled local `bids` while keeping remaining `asks` locked for cross-sector freight!
            for book in books.iter_mut() {
                for bid in book.bids.drain(..) {
                    if bid.remaining_qty > 0 {
                        inventories[bid.cash_inv]
                            .unlock(cash_id, bid.remaining_qty * bid.price_cents);
                    }
                }
            }
        } else {
            let mut next_oid = 1u32;
            for sell in cap_sells {
                let s = usize::from(sell.sector_id);
                if s >= s_count || sell.qty <= 0 {
                    continue;
                }
                let locked = inventories[sell.goods_inv].lock(cap_gid, sell.qty);
                if locked > 0 {
                    books[s].asks.push(RestingOrder {
                        order_id: next_oid,
                        sector_id: sell.sector_id,
                        goods_inv: sell.goods_inv,
                        cash_inv: sell.cash_inv,
                        price_cents: sell.ask_price_cents,
                        remaining_qty: locked,
                    });
                    next_oid += 1;
                }
            }
            for book in books.iter_mut() {
                book.asks.sort_by(|a, b| {
                    a.price_cents
                        .cmp(&b.price_cents)
                        .then_with(|| a.order_id.cmp(&b.order_id))
                });
            }
        }
    }

    let mut cap_head: BTreeMap<GoodId, Vec<usize>> = cap_gids
        .iter()
        .map(|&g| (g, vec![0usize; s_count]))
        .collect();

    #[derive(Clone)]
    struct RouteOption {
        /// `(cap_good, sector, coast_filter, shares_per_cargo_unit)`
        legs: Vec<(GoodId, usize, Option<u16>, i64)>,
        leg_ask_cost_cents: i64,
        total_shares_per_unit: i64,
        max_cargo_units: i64,
        first_step_cap_units: i64,
        tariff_per_unit_cents: i64,
    }

    let find_best_route = |channel: TransportChannel,
                           vol_units: i64,
                           s: usize,
                           d: usize,
                           gbooks: &BTreeMap<GoodId, Vec<SectorOrderBook>>,
                           cheads: &BTreeMap<GoodId, Vec<usize>>|
     -> Option<RouteOption> {
        if net.embargoed.get(s * s_count + d).copied().unwrap_or(false) {
            return None;
        }
        let tariff = if s == d {
            0
        } else {
            net.tariff_per_vol_cents
                .get(s * s_count + d)
                .copied()
                .unwrap_or(0)
                * vol_units
        };
        let ra = net.sector_geo_region.get(s).copied().unwrap_or(u16::MAX);
        let rb = net.sector_geo_region.get(d).copied().unwrap_or(u16::MAX);
        if s == d || (ra != u16::MAX && ra == rb) {
            return Some(RouteOption {
                legs: Vec::new(),
                leg_ask_cost_cents: 0,
                total_shares_per_unit: 0,
                max_cargo_units: i64::MAX / 8,
                first_step_cap_units: i64::MAX / 8,
                tariff_per_unit_cents: tariff,
            });
        }
        if channel == TransportChannel::LocalOnly {
            return None;
        }
        if channel == TransportChannel::Telecom {
            return Some(RouteOption {
                legs: Vec::new(),
                leg_ask_cost_cents: 0,
                total_shares_per_unit: 0,
                max_cargo_units: i64::MAX / 8,
                first_step_cap_units: i64::MAX / 8,
                tariff_per_unit_cents: 0,
            });
        }

        let mut best: Option<RouteOption> = None;
        for mode in net.modes {
            let mode_matches = match channel {
                TransportChannel::PowerGrid => mode.kind == TransportHubKind::UtilityGridRelay,
                TransportChannel::Physical => matches!(
                    mode.kind,
                    TransportHubKind::LandRelay
                        | TransportHubKind::SeaPointToPoint
                        | TransportHubKind::SuborbitalPointToPoint
                ),
                _ => false,
            };
            if !mode_matches {
                continue;
            }
            let Some(cbooks) = gbooks.get(&mode.cap_good) else {
                continue;
            };
            let Some(heads) = cheads.get(&mode.cap_good) else {
                continue;
            };

            let cand: Option<RouteOption> = match mode.kind {
                TransportHubKind::SeaPointToPoint => {
                    let use_coast_segments =
                        !net.inv_coast_id.is_empty() && net.geo.coast_count > 0;
                    if use_coast_segments {
                        // Collect active port coast IDs in `s` and `d` (`<= 2..3` per sector -> instant O(1) table lookup!)
                        let mut coasts_s: Vec<u16> = Vec::new();
                        for o in cbooks[s].asks.iter().skip(heads[s]) {
                            if o.remaining_qty > 0 {
                                let cid = net
                                    .inv_coast_id
                                    .get(o.goods_inv)
                                    .copied()
                                    .unwrap_or(u16::MAX);
                                if cid < net.geo.coast_count as u16 && !coasts_s.contains(&cid) {
                                    coasts_s.push(cid);
                                }
                            }
                        }
                        let mut coasts_d: Vec<u16> = Vec::new();
                        for o in cbooks[d].asks.iter().skip(heads[d]) {
                            if o.remaining_qty > 0 {
                                let cid = net
                                    .inv_coast_id
                                    .get(o.goods_inv)
                                    .copied()
                                    .unwrap_or(u16::MAX);
                                if cid < net.geo.coast_count as u16 && !coasts_d.contains(&cid) {
                                    coasts_d.push(cid);
                                }
                            }
                        }
                        let mut best_sea: Option<RouteOption> = None;
                        for &cs in &coasts_s {
                            for &cd in &coasts_d {
                                let steps = net.geo.coast_sea_distance(cs, cd);
                                if steps == crate::economy::GeoTransportTopology::UNREACHABLE_STEPS
                                {
                                    continue;
                                }
                                let extra = if mode.steps_per_extra_share > 0 {
                                    i64::from(steps / mode.steps_per_extra_share)
                                        .min(mode.max_extra_shares)
                                } else {
                                    0
                                };
                                let sh = (mode.endpoint_shares + extra) * vol_units;
                                if let (Some((c_s, max_s, f_s)), Some((c_d, max_d, f_d))) = (
                                    quote_capacity_leg(
                                        &cbooks[s].asks,
                                        heads[s],
                                        Some(cs),
                                        net.inv_coast_id,
                                        sh,
                                    ),
                                    quote_capacity_leg(
                                        &cbooks[d].asks,
                                        heads[d],
                                        Some(cd),
                                        net.inv_coast_id,
                                        sh,
                                    ),
                                ) {
                                    let step_u =
                                        (f_s / sh).max(1).min((f_d / sh).max(1)).min(max_s.min(max_d));
                                    let opt = RouteOption {
                                        legs: vec![
                                            (mode.cap_good, s, Some(cs), sh),
                                            (mode.cap_good, d, Some(cd), sh),
                                        ],
                                        leg_ask_cost_cents: c_s + c_d,
                                        total_shares_per_unit: sh * 2,
                                        max_cargo_units: max_s.min(max_d),
                                        first_step_cap_units: step_u,
                                        tariff_per_unit_cents: tariff,
                                    };
                                    let rep = match &best_sea {
                                        None => true,
                                        Some(b) => {
                                            opt.leg_ask_cost_cents < b.leg_ask_cost_cents
                                                || (opt.leg_ask_cost_cents == b.leg_ask_cost_cents
                                                    && opt.max_cargo_units > b.max_cargo_units)
                                        }
                                    };
                                    if rep {
                                        best_sea = Some(opt);
                                    }
                                }
                            }
                        }
                        best_sea
                    } else {
                        let steps = net.geo.sea_distance(ra, rb);
                        if steps == crate::economy::GeoTransportTopology::UNREACHABLE_STEPS {
                            None
                        } else {
                            let extra = if mode.steps_per_extra_share > 0 {
                                i64::from(steps / mode.steps_per_extra_share)
                                    .min(mode.max_extra_shares)
                            } else {
                                0
                            };
                            let sh = (mode.endpoint_shares + extra) * vol_units;
                            if let (Some((c_s, max_s, f_s)), Some((c_d, max_d, f_d))) = (
                                quote_capacity_leg(
                                    &cbooks[s].asks,
                                    heads[s],
                                    None,
                                    net.inv_coast_id,
                                    sh,
                                ),
                                quote_capacity_leg(
                                    &cbooks[d].asks,
                                    heads[d],
                                    None,
                                    net.inv_coast_id,
                                    sh,
                                ),
                            ) {
                                let step_u =
                                    (f_s / sh).max(1).min((f_d / sh).max(1)).min(max_s.min(max_d));
                                Some(RouteOption {
                                    legs: vec![
                                        (mode.cap_good, s, None, sh),
                                        (mode.cap_good, d, None, sh),
                                    ],
                                    leg_ask_cost_cents: c_s + c_d,
                                    total_shares_per_unit: sh * 2,
                                    max_cargo_units: max_s.min(max_d),
                                    first_step_cap_units: step_u,
                                    tariff_per_unit_cents: tariff,
                                })
                            } else {
                                None
                            }
                        }
                    }
                }
                TransportHubKind::SuborbitalPointToPoint => {
                    let steps = net.geo.air_distance(ra, rb);
                    if steps == crate::economy::GeoTransportTopology::UNREACHABLE_STEPS {
                        None
                    } else {
                        let extra = if mode.steps_per_extra_share > 0
                            && steps > mode.steps_per_extra_share
                        {
                            1.min(mode.max_extra_shares)
                        } else {
                            0
                        };
                        let sh = (mode.endpoint_shares + extra) * vol_units;
                        if let (Some((c_s, max_s, f_s)), Some((c_d, max_d, f_d))) = (
                            quote_capacity_leg(
                                &cbooks[s].asks,
                                heads[s],
                                None,
                                net.inv_coast_id,
                                sh,
                            ),
                            quote_capacity_leg(
                                &cbooks[d].asks,
                                heads[d],
                                None,
                                net.inv_coast_id,
                                sh,
                            ),
                        ) {
                            let step_u =
                                (f_s / sh).max(1).min((f_d / sh).max(1)).min(max_s.min(max_d));
                            Some(RouteOption {
                                legs: vec![
                                    (mode.cap_good, s, None, sh),
                                    (mode.cap_good, d, None, sh),
                                ],
                                leg_ask_cost_cents: c_s + c_d,
                                total_shares_per_unit: sh * 2,
                                max_cargo_units: max_s.min(max_d),
                                first_step_cap_units: step_u,
                                tariff_per_unit_cents: tariff,
                            })
                        } else {
                            None
                        }
                    }
                }
                TransportHubKind::LandRelay | TransportHubKind::UtilityGridRelay => {
                    // Dijkstra over `S` sectors where crossing natural border `(u, v)` consumes
                    // `endpoint_shares * mult * vol_units` in `u` and in `v`.
                    let inf = i64::MAX / 8;
                    let mut dist = vec![inf; s_count];
                    let mut prev = vec![usize::MAX; s_count];
                    let mut vis = vec![false; s_count];
                    dist[s] = 0;

                    for _ in 0..s_count {
                        let mut u = usize::MAX;
                        let mut best_d = inf;
                        for i in 0..s_count {
                            if !vis[i] && dist[i] < best_d {
                                best_d = dist[i];
                                u = i;
                            }
                        }
                        if u == usize::MAX || u == d {
                            break;
                        }
                        vis[u] = true;
                        let ru = net.sector_geo_region.get(u).copied().unwrap_or(u16::MAX);
                        for v in 0..s_count {
                            if vis[v] || u == v {
                                continue;
                            }
                            if u != s
                                && net.embargoed.get(u * s_count + v).copied().unwrap_or(false)
                            {
                                continue;
                            }
                            let rv = net.sector_geo_region.get(v).copied().unwrap_or(u16::MAX);
                            let edge_cost = if ru != u16::MAX && ru == rv {
                                0
                            } else {
                                let mult = i64::from(net.geo.land_mult(ru, rv));
                                if mult <= 0 {
                                    continue;
                                }
                                let sh = mode.endpoint_shares * mult * vol_units;
                                let (Some((cu, _, _)), Some((cv, _, _))) = (
                                    quote_capacity_leg(
                                        &cbooks[u].asks,
                                        heads[u],
                                        None,
                                        net.inv_coast_id,
                                        sh,
                                    ),
                                    quote_capacity_leg(
                                        &cbooks[v].asks,
                                        heads[v],
                                        None,
                                        net.inv_coast_id,
                                        sh,
                                    ),
                                ) else {
                                    continue;
                                };
                                cu + cv
                            };
                            let nd = dist[u] + edge_cost;
                            if nd < dist[v] {
                                dist[v] = nd;
                                prev[v] = u;
                            }
                        }
                    }

                    if dist[d] >= inf {
                        None
                    } else {
                        let mut path = Vec::new();
                        let mut cur = d;
                        while cur != usize::MAX {
                            path.push(cur);
                            if cur == s {
                                break;
                            }
                            cur = prev[cur];
                        }
                        path.reverse();
                        let mut shares_by_sec = vec![0i64; s_count];
                        for win in path.windows(2) {
                            let (u, v) = (win[0], win[1]);
                            let ru = net.sector_geo_region[u];
                            let rv = net.sector_geo_region[v];
                            if ru != rv {
                                let mult = i64::from(net.geo.land_mult(ru, rv));
                                let sh = mode.endpoint_shares * mult * vol_units;
                                shares_by_sec[u] += sh;
                                shares_by_sec[v] += sh;
                            }
                        }
                        let mut legs = Vec::new();
                        let mut exact_cost = 0i64;
                        let mut total_sh = 0i64;
                        let mut min_units = i64::MAX / 8;
                        let mut first_step = i64::MAX / 8;
                        let mut ok = true;
                        for (sec, &sh) in shares_by_sec.iter().enumerate() {
                            if sh > 0 {
                                if let Some((c, u_max, f_av)) = quote_capacity_leg(
                                    &cbooks[sec].asks,
                                    heads[sec],
                                    None,
                                    net.inv_coast_id,
                                    sh,
                                ) {
                                    legs.push((mode.cap_good, sec, None, sh));
                                    exact_cost += c;
                                    total_sh += sh;
                                    min_units = min_units.min(u_max);
                                    first_step = first_step.min((f_av / sh).max(1));
                                } else {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok {
                            Some(RouteOption {
                                legs,
                                leg_ask_cost_cents: exact_cost,
                                total_shares_per_unit: total_sh,
                                max_cargo_units: min_units,
                                first_step_cap_units: first_step.min(min_units),
                                tariff_per_unit_cents: tariff,
                            })
                        } else {
                            None
                        }
                    }
                }
            };

            if let Some(opt) = cand {
                let replace = match &best {
                    None => true,
                    Some(cur) => {
                        opt.leg_ask_cost_cents < cur.leg_ask_cost_cents
                            || (opt.leg_ask_cost_cents == cur.leg_ask_cost_cents
                                && opt.max_cargo_units > cur.max_cargo_units)
                    }
                };
                if replace {
                    best = Some(opt);
                }
            }
        }
        best
    };

    // 2. Intra-sector Contract Pool + Order-Book Locking for all `PowerGrid` & `Physical` commodities
    let commodity_gids: Vec<GoodId> = orders_by_good.keys().copied().collect();
    for &gid in &commodity_gids {
        let Some((mut sells, mut buys)) = orders_by_good.remove(&gid) else {
            continue;
        };
        let gdef = catalog.get(gid);
        let rule = gdef.market_rule;
        let pool_share_pct = i64::from(rule.contract_pool_percent.min(90));
        let vol_units = if gdef.transport_channel == TransportChannel::Physical {
            i64::from(gdef.unit_volume.max(1))
        } else {
            1
        };

        let mut best_external_landed = vec![i64::MAX / 4; s_count];
        for sell in &sells {
            if sell.qty <= 0 {
                continue;
            }
            let s = usize::from(sell.sector_id);
            if s >= s_count {
                continue;
            }
            for d in 0..s_count {
                if s != d {
                    if let Some(rt) = find_best_route(
                        gdef.transport_channel,
                        vol_units,
                        s,
                        d,
                        good_books,
                        &cap_head,
                    ) {
                        let landed =
                            sell.ask_price_cents + rt.leg_ask_cost_cents + rt.tariff_per_unit_cents;
                        if landed < best_external_landed[d] {
                            best_external_landed[d] = landed;
                        }
                    }
                }
            }
        }

        let Some(books) = good_books.get_mut(&gid) else {
            continue;
        };

        for sec in 0..s_count {
            let pool_price =
                snap_price_cents(books[sec].ma_price_cents.min(best_external_landed[sec]));
            let cheap_local_qty: i64 = sells
                .iter()
                .filter(|s| {
                    usize::from(s.sector_id) == sec
                        && s.qty > 0
                        && s.ask_price_cents * 100 < pool_price * 80
                })
                .map(|s| s.qty)
                .sum();
            let total_local_buy_qty: i64 = buys
                .iter()
                .filter(|b| usize::from(b.sector_id) == sec && b.qty > 0)
                .map(|b| b.qty)
                .sum();
            let buyer_pool_pct = if pool_share_pct <= 0 {
                0
            } else if cheap_local_qty > 0 && cheap_local_qty * 4 >= total_local_buy_qty.max(1) {
                pool_share_pct / 2
            } else {
                pool_share_pct
            };

            let mut sec_sell_indices = Vec::new();
            let mut pool_supply = 0i64;
            if pool_share_pct > 0 {
                for (idx, sell) in sells.iter().enumerate() {
                    if usize::from(sell.sector_id) == sec
                        && sell.allow_contract_pool
                        && sell.qty > 0
                        && sell.ask_price_cents <= pool_price
                    {
                        let share = (sell.qty * pool_share_pct) / 100;
                        if share > 0 {
                            let avail = inventories[sell.goods_inv].avail(gid).min(share);
                            if avail > 0 {
                                sec_sell_indices.push((idx, avail));
                                pool_supply += avail;
                            }
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
                            let fit = inventories[buy.goods_inv].max_receivable_units(catalog, gid);
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
                let mut rem_to_take = matched;
                let mut seller_cursor = 0usize;
                let mut buyer_allocated: Vec<(usize, i64)> =
                    Vec::with_capacity(sec_buy_indices.len());
                let mut alloc_sum = 0i64;
                for &(b_idx, b_eff) in &sec_buy_indices {
                    let q =
                        ((i128::from(b_eff) * i128::from(matched)) / i128::from(pool_demand)) as i64;
                    if q > 0 {
                        buyer_allocated.push((b_idx, q));
                        alloc_sum += q;
                    }
                }
                for &(b_idx, b_eff) in &sec_buy_indices {
                    if alloc_sum >= matched {
                        break;
                    }
                    if let Some(entry) = buyer_allocated.iter_mut().find(|x| x.0 == b_idx) {
                        if entry.1 < b_eff {
                            entry.1 += 1;
                            alloc_sum += 1;
                        }
                    } else {
                        buyer_allocated.push((b_idx, 1));
                        alloc_sum += 1;
                    }
                }

                let mut seller_rem = sec_sell_indices;
                for (b_idx, mut b_need) in buyer_allocated {
                    while b_need > 0 && seller_cursor < seller_rem.len() {
                        let (s_idx, s_avail) = seller_rem[seller_cursor];
                        if s_avail <= 0 {
                            seller_cursor += 1;
                            continue;
                        }
                        let buy_sub = buys[b_idx];
                        let sell_sub = sells[s_idx];
                        let fit =
                            inventories[buy_sub.goods_inv].max_receivable_units(catalog, gid);
                        let afford = inventories[buy_sub.cash_inv].avail(cash_id) / pool_price;
                        let step = b_need
                            .min(s_avail)
                            .min(rem_to_take)
                            .min(fit)
                            .min(afford)
                            .min(inventories[sell_sub.goods_inv].avail(gid));
                        if step <= 0 {
                            break;
                        }
                        let locked_cash =
                            inventories[buy_sub.cash_inv].lock(cash_id, step * pool_price);
                        let payable_step = locked_cash / pool_price;
                        let dust = locked_cash - payable_step * pool_price;
                        if dust > 0 {
                            inventories[buy_sub.cash_inv].unlock(cash_id, dust);
                        }
                        if payable_step <= 0 {
                            break;
                        }
                        let moved = transfer_available(
                            catalog,
                            inventories,
                            sell_sub.goods_inv,
                            buy_sub.goods_inv,
                            gid,
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
                        sells[s_idx].qty -= moved;
                        buys[b_idx].qty -= moved;
                        seller_rem[seller_cursor].1 -= moved;
                        b_need -= moved;
                        rem_to_take -= moved;
                        books[sec].last_contract_volume += moved;
                        if seller_rem[seller_cursor].1 == 0 {
                            seller_cursor += 1;
                        }
                    }
                }
            }
        }

        let mut next_oid = 1u32;
        for sell in sells {
            let s = usize::from(sell.sector_id);
            if s >= s_count || sell.qty <= 0 {
                continue;
            }
            let locked = inventories[sell.goods_inv].lock(gid, sell.qty);
            if locked > 0 {
                books[s].asks.push(RestingOrder {
                    order_id: next_oid,
                    sector_id: sell.sector_id,
                    goods_inv: sell.goods_inv,
                    cash_inv: sell.cash_inv,
                    price_cents: sell.ask_price_cents,
                    remaining_qty: locked,
                });
                next_oid += 1;
            }
        }
        for buy in buys {
            let d = usize::from(buy.sector_id);
            if d >= s_count || buy.qty <= 0 {
                continue;
            }
            let fit = inventories[buy.goods_inv].max_receivable_units(catalog, gid);
            let afford = inventories[buy.cash_inv].avail(cash_id) / buy.bid_price_cents;
            let lockable = buy.qty.min(fit).min(afford).max(0);
            if lockable > 0 {
                let locked_cash =
                    inventories[buy.cash_inv].lock(cash_id, lockable * buy.bid_price_cents);
                let actual_qty = locked_cash / buy.bid_price_cents;
                let dust = locked_cash - actual_qty * buy.bid_price_cents;
                if dust > 0 {
                    inventories[buy.cash_inv].unlock(cash_id, dust);
                }
                if actual_qty > 0 {
                    books[d].bids.push(RestingOrder {
                        order_id: next_oid,
                        sector_id: buy.sector_id,
                        goods_inv: buy.goods_inv,
                        cash_inv: buy.cash_inv,
                        price_cents: buy.bid_price_cents,
                        remaining_qty: actual_qty,
                    });
                    next_oid += 1;
                }
            }
        }
        for book in books.iter_mut() {
            book.asks.sort_by(|a, b| {
                a.price_cents
                    .cmp(&b.price_cents)
                    .then_with(|| a.order_id.cmp(&b.order_id))
            });
            book.bids.sort_by(|a, b| {
                b.price_cents
                    .cmp(&a.price_cents)
                    .then_with(|| a.order_id.cmp(&b.order_id))
            });
        }
    }

    // 3A. Step 1 of Simultaneous Multi-Good Bundled Call Auction
    type SubMarketKey = (GoodId, usize, Option<u16>);

    #[derive(Clone, Copy)]
    struct CapSliceFill {
        cap_good: GoodId,
        sector: usize,
        coast_filter: Option<u16>,
        ask_cash_inv: usize,
        ask_price_cents: i64,
        total_shares: i64,
    }

    struct PendingBundledFill {
        good: GoodId,
        s: usize,
        d: usize,
        ask_cash_inv: usize,
        bid_cash_inv: usize,
        ask_price_cents: i64,
        bid_price_cents: i64,
        tariff_per_unit_cents: i64,
        qty: i64,
        legs: Vec<(GoodId, usize, Option<u16>, i64)>,
        cap_slices: Vec<CapSliceFill>,
    }

    let mut good_ask_head: BTreeMap<GoodId, Vec<usize>> = commodity_gids
        .iter()
        .map(|&g| (g, vec![0usize; s_count]))
        .collect();
    let mut good_bid_head: BTreeMap<GoodId, Vec<usize>> = commodity_gids
        .iter()
        .map(|&g| (g, vec![0usize; s_count]))
        .collect();

    let mut max_filled_ask: BTreeMap<SubMarketKey, i64> = BTreeMap::new();
    let mut has_spot_fill: BTreeMap<SubMarketKey, bool> = BTreeMap::new();
    let mut pending_fills: Vec<PendingBundledFill> = Vec::new();

    loop {
        for &cg in &cap_gids {
            if let (Some(books), Some(heads)) = (good_books.get(&cg), cap_head.get_mut(&cg)) {
                for s in 0..s_count {
                    while heads[s] < books[s].asks.len()
                        && books[s].asks[heads[s]].remaining_qty <= 0
                    {
                        heads[s] += 1;
                    }
                }
            }
        }
        for &gid in &commodity_gids {
            if let Some(books) = good_books.get(&gid) {
                let aheads = good_ask_head.get_mut(&gid).unwrap();
                let bheads = good_bid_head.get_mut(&gid).unwrap();
                for s in 0..s_count {
                    while aheads[s] < books[s].asks.len()
                        && books[s].asks[aheads[s]].remaining_qty <= 0
                    {
                        aheads[s] += 1;
                    }
                    while bheads[s] < books[s].bids.len()
                        && books[s].bids[bheads[s]].remaining_qty <= 0
                    {
                        bheads[s] += 1;
                    }
                }
            }
        }

        let mut best_match: Option<(GoodId, usize, usize, i64, i64, RouteOption)> = None;

        for &gid in &commodity_gids {
            let gdef = catalog.get(gid);
            let vol_units = if gdef.transport_channel == TransportChannel::Physical {
                i64::from(gdef.unit_volume.max(1))
            } else {
                1
            };
            let aheads = &good_ask_head[&gid];
            let bheads = &good_bid_head[&gid];

            for d in 0..s_count {
                let bh = bheads[d];
                if bh >= good_books[&gid][d].bids.len() {
                    continue;
                }
                let bid = good_books[&gid][d].bids[bh];
                let buyer_fit = inventories[bid.goods_inv].max_receivable_units(catalog, gid);
                if buyer_fit <= 0 {
                    let refund = bid.remaining_qty * bid.price_cents;
                    inventories[bid.cash_inv].unlock(cash_id, refund);
                    good_books.get_mut(&gid).unwrap()[d].bids[bh].remaining_qty = 0;
                    continue;
                }

                for s in 0..s_count {
                    let ah = aheads[s];
                    if ah >= good_books[&gid][s].asks.len() {
                        continue;
                    }
                    let ask = good_books[&gid][s].asks[ah];
                    if ask.price_cents > bid.price_cents {
                        continue;
                    }
                    let Some(rt) = find_best_route(
                        gdef.transport_channel,
                        vol_units,
                        s,
                        d,
                        good_books,
                        &cap_head,
                    ) else {
                        continue;
                    };
                    if rt.max_cargo_units <= 0 {
                        continue;
                    }
                    let landed_ask =
                        ask.price_cents + rt.leg_ask_cost_cents + rt.tariff_per_unit_cents;
                    let spread = bid.price_cents - landed_ask;
                    if spread < 0 {
                        continue;
                    }
                    // Priority score: surplus per capacity share (`total_shares == 0` local/same-region gets bonus
                    // so local trades settle without wasting cross-sector capacity, while high-margin goods beat bulk ore!)
                    let score = if rt.total_shares_per_unit == 0 {
                        spread * 24 + 1
                    } else {
                        (spread * 12) / rt.total_shares_per_unit
                    };

                    let replace = match &best_match {
                        None => true,
                        Some((bg, bs, bd, b_score, b_landed, _)) => {
                            score > *b_score
                                || (score == *b_score && landed_ask < *b_landed)
                                || (score == *b_score
                                    && landed_ask == *b_landed
                                    && (gid, s, d) < (*bg, *bs, *bd))
                        }
                    };
                    if replace {
                        best_match = Some((gid, s, d, score, landed_ask, rt));
                    }
                }
            }
        }

        let Some((gid, s, d, _score, _landed, rt)) = best_match else {
            break;
        };

        let ah = good_ask_head[&gid][s];
        let bh = good_bid_head[&gid][d];
        let ask = good_books[&gid][s].asks[ah];
        let bid = good_books[&gid][d].bids[bh];
        let buyer_fit = inventories[bid.goods_inv].max_receivable_units(catalog, gid);

        // Bound `qty` so we don't step into a higher-priced capacity ask level that would violate `bid.price_cents`
        let qty = ask
            .remaining_qty
            .min(bid.remaining_qty)
            .min(buyer_fit)
            .min(rt.max_cargo_units)
            .min(rt.first_step_cap_units);
        if qty <= 0 {
            break;
        }

        // Move physical/flow good from seller locked -> buyer available
        transfer_from_locked(inventories, ask.goods_inv, bid.goods_inv, gid, qty);
        {
            let gbooks = good_books.get_mut(&gid).unwrap();
            gbooks[s].asks[ah].remaining_qty -= qty;
            gbooks[d].bids[bh].remaining_qty -= qty;
            if s != d {
                gbooks[s].last_net_import -= qty;
                gbooks[d].last_net_import += qty;
            }
        }
        has_spot_fill.insert((gid, s, None), true);
        let cur_lo = max_filled_ask.entry((gid, s, None)).or_insert(0);
        *cur_lo = (*cur_lo).max(ask.price_cents);

        // Consume exact capacity shares across route legs (respecting `coast_filter` for ports!)
        let mut cap_slices = Vec::new();
        for &(cg, sec, cfilter, sh_per_unit) in &rt.legs {
            let mut need_sh = qty * sh_per_unit;
            let cbooks = good_books.get_mut(&cg).unwrap();
            let idx_start = cap_head[&cg][sec];
            for o in cbooks[sec].asks.iter_mut().skip(idx_start) {
                if need_sh <= 0 {
                    break;
                }
                if o.remaining_qty <= 0 || !order_matches_coast(o, cfilter, net.inv_coast_id) {
                    continue;
                }
                let take = o.remaining_qty.min(need_sh);
                o.remaining_qty -= take;
                need_sh -= take;
                inventories[o.goods_inv].unlock(cg, take);
                inventories[o.goods_inv].consume_up_to(cg, take);

                has_spot_fill.insert((cg, sec, cfilter), true);
                let clo = max_filled_ask.entry((cg, sec, cfilter)).or_insert(0);
                *clo = (*clo).max(o.price_cents);

                cap_slices.push(CapSliceFill {
                    cap_good: cg,
                    sector: sec,
                    coast_filter: cfilter,
                    ask_cash_inv: o.cash_inv,
                    ask_price_cents: o.price_cents,
                    total_shares: take,
                });
            }
            debug_assert_eq!(need_sh, 0, "quoted capacity shares must be fully available");
        }

        pending_fills.push(PendingBundledFill {
            good: gid,
            s,
            d,
            ask_cash_inv: ask.cash_inv,
            bid_cash_inv: bid.cash_inv,
            ask_price_cents: ask.price_cents,
            bid_price_cents: bid.price_cents,
            tariff_per_unit_cents: rt.tariff_per_unit_cents,
            qty,
            legs: rt.legs,
            cap_slices,
        });
    }

    // 3B. Step 2 of Call Auction: Identify sold-out bottlenecks (`excess_demand && !excess_supply`)
    // per `SubMarketKey` (so a congested West Coast ocean port isn't diluted by an idle East Coast lake port!).
    let mut sold_out: BTreeMap<SubMarketKey, bool> = BTreeMap::new();
    for (&(gid, sec, cfilter), &lo) in &max_filled_ask {
        let excess_supply = good_books
            .get(&gid)
            .and_then(|v| v.get(sec))
            .map(|b| {
                b.asks.iter().any(|o| {
                    o.remaining_qty > 0
                        && o.price_cents <= lo
                        && order_matches_coast(o, cfilter, net.inv_coast_id)
                })
            })
            .unwrap_or(false);
        sold_out.insert((gid, sec, cfilter), !excess_supply);
    }

    // Check which sold-out markets face genuine unmet demand (`excess_demand`)
    let mut is_bottleneck: BTreeMap<SubMarketKey, bool> = BTreeMap::new();
    for &gid in &commodity_gids {
        let gdef = catalog.get(gid);
        let vol_units = if gdef.transport_channel == TransportChannel::Physical {
            i64::from(gdef.unit_volume.max(1))
        } else {
            1
        };
        for d in 0..s_count {
            let has_unmet_bid = good_books[&gid][d]
                .bids
                .iter()
                .filter(|b| {
                    b.remaining_qty > 0
                        && inventories[b.goods_inv].max_receivable_units(catalog, gid) > 0
                })
                .map(|b| b.price_cents)
                .max();
            let Some(max_unmet_bid) = has_unmet_bid else {
                continue;
            };
            for s in 0..s_count {
                if net.embargoed.get(s * s_count + d).copied().unwrap_or(false) {
                    continue;
                }
                let tariff = if s == d {
                    0
                } else {
                    net.tariff_per_vol_cents
                        .get(s * s_count + d)
                        .copied()
                        .unwrap_or(0)
                        * vol_units
                };
                if sold_out.get(&(gid, s, None)).copied().unwrap_or(false) {
                    let lo_g = max_filled_ask[&(gid, s, None)];
                    if let Some(rt) = find_best_route(
                        gdef.transport_channel,
                        vol_units,
                        s,
                        d,
                        good_books,
                        &cap_head,
                    ) {
                        if max_unmet_bid - tariff - rt.leg_ask_cost_cents >= lo_g {
                            is_bottleneck.insert((gid, s, None), true);
                        }
                    }
                }
                let origin_px = good_books[&gid][s]
                    .asks
                    .iter()
                    .find(|o| o.remaining_qty > 0)
                    .map(|o| o.price_cents)
                    .or_else(|| max_filled_ask.get(&(gid, s, None)).copied());
                let Some(orig_lo) = origin_px else {
                    continue;
                };
                if max_unmet_bid <= orig_lo + tariff {
                    continue;
                }
                let ra = net.sector_geo_region.get(s).copied().unwrap_or(u16::MAX);
                let rb = net.sector_geo_region.get(d).copied().unwrap_or(u16::MAX);
                if s == d || ra == rb {
                    continue;
                }
                for mode in net.modes {
                    let mode_ok = match gdef.transport_channel {
                        TransportChannel::PowerGrid => {
                            mode.kind == TransportHubKind::UtilityGridRelay
                        }
                        TransportChannel::Physical => matches!(
                            mode.kind,
                            TransportHubKind::LandRelay
                                | TransportHubKind::SeaPointToPoint
                                | TransportHubKind::SuborbitalPointToPoint
                        ),
                        _ => false,
                    };
                    if !mode_ok {
                        continue;
                    }
                    match mode.kind {
                        TransportHubKind::SeaPointToPoint => {
                            let use_coast_segments =
                                !net.inv_coast_id.is_empty() && net.geo.coast_count > 0;
                            if use_coast_segments {
                                for ca in 0..(net.geo.coast_count as u16) {
                                    for cb in 0..(net.geo.coast_count as u16) {
                                        if net.geo.coast_sea_distance(ca, cb)
                                            < crate::economy::GeoTransportTopology::UNREACHABLE_STEPS
                                        {
                                            let s_had_ca = good_books[&mode.cap_good][s]
                                                .asks
                                                .iter()
                                                .any(|o| {
                                                    order_matches_coast(
                                                        o,
                                                        Some(ca),
                                                        net.inv_coast_id,
                                                    )
                                                });
                                            let d_had_cb = good_books[&mode.cap_good][d]
                                                .asks
                                                .iter()
                                                .any(|o| {
                                                    order_matches_coast(
                                                        o,
                                                        Some(cb),
                                                        net.inv_coast_id,
                                                    )
                                                });
                                            if s_had_ca && d_had_cb {
                                                if sold_out
                                                    .get(&(mode.cap_good, s, Some(ca)))
                                                    .copied()
                                                    .unwrap_or(false)
                                                {
                                                    is_bottleneck
                                                        .insert((mode.cap_good, s, Some(ca)), true);
                                                }
                                                if sold_out
                                                    .get(&(mode.cap_good, d, Some(cb)))
                                                    .copied()
                                                    .unwrap_or(false)
                                                {
                                                    is_bottleneck
                                                        .insert((mode.cap_good, d, Some(cb)), true);
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if net.geo.sea_distance(ra, rb)
                                < crate::economy::GeoTransportTopology::UNREACHABLE_STEPS
                            {
                                for &ep in &[s, d] {
                                    if sold_out
                                        .get(&(mode.cap_good, ep, None))
                                        .copied()
                                        .unwrap_or(false)
                                    {
                                        is_bottleneck.insert((mode.cap_good, ep, None), true);
                                    }
                                }
                            }
                        }
                        TransportHubKind::SuborbitalPointToPoint => {
                            for &ep in &[s, d] {
                                if sold_out
                                    .get(&(mode.cap_good, ep, None))
                                    .copied()
                                    .unwrap_or(false)
                                {
                                    is_bottleneck.insert((mode.cap_good, ep, None), true);
                                }
                            }
                        }
                        TransportHubKind::LandRelay | TransportHubKind::UtilityGridRelay => {
                            let mut reach_s = vec![false; s_count];
                            let mut q = std::collections::VecDeque::new();
                            reach_s[s] = true;
                            q.push_back(s);
                            while let Some(u) = q.pop_front() {
                                let ru = net.sector_geo_region[u];
                                for v in 0..s_count {
                                    let rv = net.sector_geo_region[v];
                                    if !reach_s[v] && (ru == rv || net.geo.land_mult(ru, rv) > 0) {
                                        reach_s[v] = true;
                                        q.push_back(v);
                                    }
                                }
                            }
                            if reach_s[d] {
                                for u in 0..s_count {
                                    if reach_s[u]
                                        && sold_out
                                            .get(&(mode.cap_good, u, None))
                                            .copied()
                                            .unwrap_or(false)
                                    {
                                        is_bottleneck.insert((mode.cap_good, u, None), true);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut bottleneck_mult_permille: BTreeMap<SubMarketKey, i64> = BTreeMap::new();
    for (&k, &bn) in &is_bottleneck {
        if bn {
            bottleneck_mult_permille.insert(k, i64::MAX / 4);
        }
    }
    for f in &pending_fills {
        let lo_g = max_filled_ask[&(f.good, f.s, None)];
        let mut lo_legs = 0i64;
        for &(cg, sec, cf, sh) in &f.legs {
            lo_legs += sh * max_filled_ask[&(cg, sec, cf)];
        }
        let headroom = f.bid_price_cents - f.tariff_per_unit_cents - lo_g - lo_legs;
        let mut bn_weight = 0i64;
        if is_bottleneck
            .get(&(f.good, f.s, None))
            .copied()
            .unwrap_or(false)
        {
            bn_weight += lo_g;
        }
        for &(cg, sec, cf, sh) in &f.legs {
            if is_bottleneck
                .get(&(cg, sec, cf))
                .copied()
                .unwrap_or(false)
            {
                bn_weight += sh * max_filled_ask[&(cg, sec, cf)];
            }
        }
        if bn_weight > 0 {
            let mult = if headroom <= 0 {
                1000
            } else {
                1000 + (headroom * 1000) / bn_weight
            };
            if is_bottleneck
                .get(&(f.good, f.s, None))
                .copied()
                .unwrap_or(false)
            {
                let e = bottleneck_mult_permille
                    .get_mut(&(f.good, f.s, None))
                    .unwrap();
                *e = (*e).min(mult);
            }
            for &(cg, sec, cf, _) in &f.legs {
                if is_bottleneck
                    .get(&(cg, sec, cf))
                    .copied()
                    .unwrap_or(false)
                {
                    let e = bottleneck_mult_permille.get_mut(&(cg, sec, cf)).unwrap();
                    *e = (*e).min(mult);
                }
            }
        }
    }

    let mut clear_px: BTreeMap<SubMarketKey, i64> = BTreeMap::new();
    for (&k, &lo) in &max_filled_ask {
        let rule = catalog.get(k.0).market_rule;
        let px = match rule.pricing_mode {
            PricingMode::UniformMarginalFob => {
                if let Some(&mult) = bottleneck_mult_permille.get(&k) {
                    if mult > 1000 && mult < i64::MAX / 8 {
                        snap_price_cents_floor((lo * mult) / 1000).max(lo)
                    } else {
                        lo
                    }
                } else {
                    lo
                }
            }
            PricingMode::MakerQuote => lo,
            PricingMode::FixedPrice { price_cents } => snap_price_cents(price_cents),
        };
        clear_px.insert(k, px);
    }

    // Settle cash atomically for every fill (guaranteed `<= f.bid_price_cents`!)
    for f in pending_fills {
        let rule_g = catalog.get(f.good).market_rule;
        let mut exec_good_px = match rule_g.pricing_mode {
            PricingMode::UniformMarginalFob => clear_px[&(f.good, f.s, None)],
            PricingMode::MakerQuote => f.ask_price_cents,
            PricingMode::FixedPrice { price_cents } => snap_price_cents(price_cents),
        };

        // Compute total freight cents across `f.cap_slices`
        let mut slice_payments: Vec<(usize, GoodId, usize, i64, i64)> =
            Vec::with_capacity(f.cap_slices.len());
        let mut total_cap_cents = 0i64;
        for sl in &f.cap_slices {
            let sl_px = match catalog.get(sl.cap_good).market_rule.pricing_mode {
                PricingMode::UniformMarginalFob => {
                    clear_px[&(sl.cap_good, sl.sector, sl.coast_filter)]
                }
                PricingMode::MakerQuote => sl.ask_price_cents,
                PricingMode::FixedPrice { price_cents } => snap_price_cents(price_cents),
            };
            let pay = sl.total_shares * sl_px;
            total_cap_cents += pay;
            slice_payments.push((sl.ask_cash_inv, sl.cap_good, sl.sector, sl.total_shares, pay));
        }

        let total_tariff_cents = f.qty * f.tariff_per_unit_cents;
        let max_budget_cents = f.qty * f.bid_price_cents;

        // Guard against cross-good multi-step ask jumps exceeding an earlier buyer's locked bid
        if f.qty * exec_good_px + total_cap_cents + total_tariff_cents > max_budget_cents {
            exec_good_px = f.ask_price_cents;
            total_cap_cents = 0;
            slice_payments.clear();
            for sl in &f.cap_slices {
                let pay = sl.total_shares * sl.ask_price_cents;
                total_cap_cents += pay;
                slice_payments.push((
                    sl.ask_cash_inv,
                    sl.cap_good,
                    sl.sector,
                    sl.total_shares,
                    pay,
                ));
            }
        }

        let good_proceeds = f.qty * exec_good_px;
        let total_spent = good_proceeds + total_cap_cents + total_tariff_cents;
        debug_assert!(
            total_spent <= max_budget_cents,
            "bundled execution exceeded buyer locked cash!"
        );

        transfer_from_locked(
            inventories,
            f.bid_cash_inv,
            f.ask_cash_inv,
            cash_id,
            good_proceeds,
        );
        for &(hub_cash_inv, cg, sec, shares, pay) in &slice_payments {
            if pay > 0 {
                transfer_from_locked(inventories, f.bid_cash_inv, hub_cash_inv, cash_id, pay);
                if let Some(rev) = polity_freight_rev_by_inv.get_mut(&hub_cash_inv) {
                    *rev += pay;
                }
            }
            if shares > 0 {
                let unit_cap_px = pay / shares;
                if let Some(cbooks) = good_books.get_mut(&cg) {
                    cbooks[sec].last_price_cents = unit_cap_px;
                    cbooks[sec].last_spot_volume += shares;
                    cbooks[sec].recent_trades.push(TradeFillRecord {
                        price_cents: unit_cap_px,
                        qty: shares,
                        from_sector: sec as u16,
                        to_sector: sec as u16,
                        freight_cents: 0,
                    });
                }
                spot_notional.get_mut(&cg).unwrap()[sec] += i128::from(pay);
                spot_vol.get_mut(&cg).unwrap()[sec] += shares;
            }
        }
        if total_tariff_cents > 0 {
            let t_inv = net.tariff_receiver_inv[f.d];
            transfer_from_locked(
                inventories,
                f.bid_cash_inv,
                t_inv,
                cash_id,
                total_tariff_cents,
            );
            if let Some(rev) = polity_freight_rev_by_inv.get_mut(&t_inv) {
                *rev += total_tariff_cents;
            }
        }

        let savings = max_budget_cents - total_spent;
        if savings > 0 {
            inventories[f.bid_cash_inv].unlock(cash_id, savings);
        }

        let unit_freight = (total_cap_cents + total_tariff_cents) / f.qty;
        let exec_landed_d = exec_good_px + unit_freight;
        {
            let gbooks = good_books.get_mut(&f.good).unwrap();
            gbooks[f.s].last_price_cents = exec_good_px;
            gbooks[f.d].last_price_cents = exec_landed_d;
            gbooks[f.s].last_spot_volume += f.qty;
            if f.d != f.s {
                gbooks[f.d].last_spot_volume += f.qty;
            }
            let rec = TradeFillRecord {
                price_cents: exec_landed_d,
                qty: f.qty,
                from_sector: f.s as u16,
                to_sector: f.d as u16,
                freight_cents: unit_freight,
            };
            gbooks[f.d].recent_trades.push(rec.clone());
            if f.s != f.d {
                gbooks[f.s].recent_trades.push(rec);
            }
        }
        spot_notional.get_mut(&f.good).unwrap()[f.s] +=
            i128::from(f.qty) * i128::from(exec_good_px);
        spot_vol.get_mut(&f.good).unwrap()[f.s] += f.qty;
        if f.d != f.s {
            spot_notional.get_mut(&f.good).unwrap()[f.d] +=
                i128::from(f.qty) * i128::from(exec_landed_d);
            spot_vol.get_mut(&f.good).unwrap()[f.d] += f.qty;
        }
    }

    // Retain active resting orders & update rolling `ma_price_cents` across all participating books
    for &gid in &participating_gids {
        let ma_weight = i64::from(catalog.get(gid).market_rule.ma_inertia_weight.max(1));
        let Some(books) = good_books.get_mut(&gid) else {
            continue;
        };
        let s_notional = &spot_notional[&gid];
        let s_vol = &spot_vol[&gid];

        for sec in 0..s_count {
            books[sec].asks.retain(|o| o.remaining_qty > 0);
            books[sec].bids.retain(|o| o.remaining_qty > 0);

            if s_vol[sec] > 0 {
                let mut target_px = (s_notional[sec] / i128::from(s_vol[sec])) as i64;
                if books[sec].best_bid_cents().is_none() {
                    if let Some(best_a) = books[sec].best_ask_cents() {
                        if best_a < target_px {
                            target_px = (target_px + best_a) / 2;
                        }
                    }
                }
                let blended = (books[sec].ma_price_cents * ma_weight + target_px) / (ma_weight + 1);
                books[sec].ma_price_cents = snap_price_cents(blended);
            } else if let (Some(best_a), Some(best_b)) =
                (books[sec].best_ask_cents(), books[sec].best_bid_cents())
            {
                let cur_ma = books[sec].ma_price_cents;
                let mid = (best_a + best_b) / 2;
                let blended = snap_price_cents((cur_ma * 4 + mid) / 5);
                books[sec].ma_price_cents = if mid + PRICE_TICK_CENTS <= cur_ma {
                    blended.min(cur_ma - PRICE_TICK_CENTS).max(MIN_ORDER_PRICE_CENTS)
                } else if mid >= cur_ma + PRICE_TICK_CENTS {
                    blended.max(cur_ma + PRICE_TICK_CENTS).min(MAX_ORDER_PRICE_CENTS)
                } else {
                    blended
                };
            } else if let Some(best_a) = books[sec].best_ask_cents() {
                let cur_ma = books[sec].ma_price_cents;
                let gen_px = catalog.get(gid).genesis_price_cents.max(MIN_ORDER_PRICE_CENTS);
                if best_a < cur_ma {
                    let blended = snap_price_cents((cur_ma * 4 + best_a) / 5);
                    books[sec].ma_price_cents = blended.min(cur_ma - PRICE_TICK_CENTS).max(best_a);
                } else if books[sec].last_contract_volume == 0 && cur_ma > gen_px {
                    let decayed =
                        snap_price_cents((cur_ma * 96) / 100).min(cur_ma - PRICE_TICK_CENTS);
                    books[sec].ma_price_cents = decayed.max(gen_px);
                }
            } else if let Some(best_b) = books[sec].best_bid_cents() {
                let cur_ma = books[sec].ma_price_cents;
                if best_b > cur_ma {
                    let blended = snap_price_cents((cur_ma * 4 + best_b) / 5);
                    books[sec].ma_price_cents = blended
                        .max(cur_ma + PRICE_TICK_CENTS)
                        .min(best_b)
                        .min(MAX_ORDER_PRICE_CENTS);
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    {
        let total_cash_after: i64 = inventories.iter().map(|inv| inv.total(cash_id)).sum();
        debug_assert_eq!(
            total_cash_before, total_cash_after,
            "Bundled transport market clearing violated cash conservation!"
        );
        for (&gid, &before) in &non_cap_totals_before {
            let after: i64 = inventories.iter().map(|inv| inv.total(gid)).sum();
            debug_assert_eq!(
                before, after,
                "Bundled transport market clearing violated physical good conservation for {:?}",
                gid
            );
        }
    }
}
