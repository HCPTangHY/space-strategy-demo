//! Commodity, Service, Security & Strategic-Depot Quoting Heuristics (`AI 报价与挂单决策`).

use crate::building::{BuildingCatalog, BuildingDef, BuildingTag, TileSlotKey};
use crate::economy::{BuildingOwner, DepotPolicy, PopGroup, PopTypeCatalog, PopTypeDef, PopTypeId};
use crate::market::{
    compute_per_100_demand, snap_price_cents, BuySubmission, GoodCatalog, GoodId, Inventory,
    SectorOrderBook, SellSubmission, CASH_SCALE, PRICE_TICK_CENTS,
};
use crate::planet::{PlanetTiles, SectorSimView};

#[inline]
pub(crate) fn pct_price(base_cents: i64, percent: i64) -> i64 {
    snap_price_cents(base_cents * percent / 100)
}

/// Computes a workshop's ask price from local `ma_cents`, `unit_cost_cents`, and warehouse fill ratio:
/// - Low inventory (`<= 35%` full): quotes at `max(unit_cost, 100% MA)`
/// - Moderate inventory (`35%..=75%` full): gently trims `100% -> 95% MA`, floored at `unit_cost`
/// - Overstocked / near-full warehouse (`> 75%` full): discounts down to `80% MA` and relaxes the cost floor
///   down to `50% unit_cost` so full workshops liquidate excess stock for working capital instead of freezing.
pub(crate) fn compute_workshop_ask_cents(
    ma_cents: i64,
    unit_cost_cents: i64,
    qty: i64,
    capacity: i64,
) -> i64 {
    let cap = capacity.max(1);
    let fill_permille = ((qty.max(0) * 1000) / cap).clamp(0, 1000);
    if fill_permille <= 350 {
        snap_price_cents(unit_cost_cents.max(ma_cents))
    } else if fill_permille <= 750 {
        let ma_permille = 1000 - ((fill_permille - 350) * 50) / 400;
        let target = (ma_cents * ma_permille) / 1000;
        snap_price_cents(unit_cost_cents.max(target))
    } else {
        let crowd = fill_permille - 750; // 0..=250
        let ma_permille = 950 - (crowd * 150) / 250;
        let cost_floor_permille = 1000 - (crowd * 500) / 250;
        let target = (ma_cents * ma_permille) / 1000;
        let floor = (unit_cost_cents * cost_floor_permille) / 1000;
        snap_price_cents(target.max(floor))
    }
}

/// Data-driven priority key for redeeming locked `StaffContract` obligations when a multi-slot payer
/// (`Polity` treasury, `Enterprise`, `Trust`, or `SectorCapital`) has finite cash.
/// Reads `PopTypeDef.redemption_tier` directly from `PopTypeCatalog` (`0` = top priority, e.g. military).
pub(crate) fn contract_redemption_priority(
    catalog: &PopTypeCatalog,
    def: &BuildingDef,
    role_id: PopTypeId,
    is_capital_tile: bool,
) -> (u8, u8) {
    let base_tier = catalog.get(role_id).redemption_tier;
    let role_tier = if base_tier == 0 {
        0
    } else if def.is_state_core_or_public() && base_tier <= 1 {
        1
    } else if def.is_state_core_or_public()
        || def.tags.has(BuildingTag::CorporateCampus)
        || def.tags.has(BuildingTag::Commercial)
    {
        base_tier.max(2)
    } else {
        3
    };
    let loc_tier = if is_capital_tile { 0 } else { 1 };
    (role_tier, loc_tier)
}

/// Commodity order-quoting pass by autonomous entities (`Workshop`, `PopGroup`, `Sector` Depot)
/// for any tradable `good` in `GoodCatalog`, driven purely by `BuildingDef` recipes and `GoodDef` attributes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_monthly_commodity_orders(
    goods: &GoodCatalog,
    buildings: &BuildingCatalog,
    pop_catalog: &PopTypeCatalog,
    pt: &PlanetTiles,
    sec_views: &[SectorSimView],
    depot_policy: &[DepotPolicy],
    books: &[SectorOrderBook],
    invs: &[Inventory],
    polity_inv: usize,
    depot_invs: &[usize],
    pop_invs: &[Vec<usize>],
    ws_refs: &[(usize, TileSlotKey, usize)],
    good: GoodId,
) -> (Vec<SellSubmission>, Vec<BuySubmission>) {
    let sec_count = sec_views.len();
    let cash = goods.cash_id();
    let gdef = goods.get(good);
    let good_str = gdef.id.as_str();
    let is_flow = gdef.retention_permille == 0;
    let staple_per_100 = gdef.staple_demand_per_100;
    let service_per_100 = gdef.service_demand_per_100;
    let is_security =
        !gdef.is_currency && (gdef.coupon_permille_per_month > 0 || gdef.issuer_key.is_some());
    let depot_target = gdef.strategic_depot_target;
    let ma: Vec<i64> = books.iter().map(|b| b.ma_price_cents).collect();
    let mut sells: Vec<SellSubmission> = Vec::new();
    let mut buys: Vec<BuySubmission> = Vec::new();

    // 1. Productive workshops (recipe input procurement bids + normal output asks + bankruptcy liquidation asks)
    for &(tile, slot, w_inv) in ws_refs {
        let s = usize::from(pt.tiles[tile].sector_id);
        if s >= sec_count {
            continue;
        }
        let Some(tid) = pt.tiles[tile].slots.get(slot) else {
            continue;
        };
        let def = buildings.get(tid);
        let scale = i64::from(def.scale_for_slot(slot));
        let input_req: i64 = def
            .monthly_inputs
            .iter()
            .filter(|(gid_s, _)| gid_s == good_str)
            .map(|&(_, amt)| amt * scale)
            .sum();

        let ws_meta = pt.workshops[tile][slot.as_index()].as_ref();
        let owner = pt.slot_owner(tile, slot);
        let is_local_private = matches!(owner, BuildingOwner::LocalPrivate { .. });
        let in_liquidation = is_local_private
            && ws_meta
                .map(|ws| {
                    ws.consecutive_arrears_months >= 2 && ws.total_unpaid_claims_cents() > 0
                })
                .unwrap_or(false);

        let qty = invs[w_inv].avail(good);
        if input_req > 0 && !in_liquidation {
            // Flow goods (`retention_permille == 0`) only bid `1.0x` current-month need; physical goods keep `2.0x` buffer!
            let target_stock = if is_flow { input_req } else { input_req * 2 };
            let want = target_stock - qty;
            if want > 0 && pt.slot_honors_wages(tile, slot) {
                let bid = if qty < input_req {
                    pct_price(ma[s], 112)
                } else {
                    ma[s]
                };
                buys.push(BuySubmission {
                    sector_id: s as u16,
                    goods_inv: w_inv,
                    cash_inv: w_inv,
                    bid_price_cents: bid,
                    qty: want,
                    allow_contract_pool: true,
                });
            }
            continue;
        }

        if qty <= 0 {
            continue;
        }

        if in_liquidation {
            sells.push(SellSubmission {
                sector_id: s as u16,
                goods_inv: w_inv,
                cash_inv: w_inv,
                ask_price_cents: pct_price(ma[s], 50),
                qty,
                allow_contract_pool: false,
            });
        } else {
            let unit_cost = ws_meta.map(|ws| ws.last_unit_cost_cents).unwrap_or(0);
            let ask_price_cents = if is_flow || gdef.unit_volume == 0 {
                // When a flow/capacity workshop had substantial unsold capacity last month,
                // step down by at least 1 price tick (`0.10 CR`) toward `unit_cost` so oversupplied
                // rail/port/grid hubs smoothly compete down to `0.50 CR` instead of rounding back to `1.00 CR`!
                let underutilized = ws_meta
                    .map(|ws| {
                        ws.last_output > 0
                            && ws.last_revenue_cents < (ws.last_output * ma[s] * 65) / 100
                    })
                    .unwrap_or(false);
                if underutilized {
                    let nudged = (ma[s] - PRICE_TICK_CENTS).min(pct_price(ma[s], 92));
                    snap_price_cents(nudged.max(unit_cost))
                } else {
                    pct_price(ma[s], 98).max(unit_cost)
                }
            } else {
                let base_ask =
                    compute_workshop_ask_cents(ma[s], unit_cost, qty, invs[w_inv].capacity);
                let stagnant_stock = ws_meta
                    .map(|ws| {
                        ws.last_revenue_cents >= 0
                            && qty > ws.last_output.max(1)
                            && ws.last_revenue_cents < (ws.last_output.max(1) * ma[s] * 50) / 100
                    })
                    .unwrap_or(false);
                if stagnant_stock && base_ask > unit_cost && ma[s] > unit_cost {
                    let nudged = (ma[s] - PRICE_TICK_CENTS)
                        .min(pct_price(ma[s], 92))
                        .min(base_ask);
                    snap_price_cents(nudged.max(unit_cost))
                } else {
                    base_ask
                }
            };
            sells.push(SellSubmission {
                sector_id: s as u16,
                goods_inv: w_inv,
                cash_inv: w_inv,
                ask_price_cents,
                qty,
                allow_contract_pool: ma[s] >= unit_cost,
            });
        }
    }

    // Precompute basic monthly living bill per `(sector, pop_group)` when evaluating service or security surplus allocation
    let staples_list = goods.staple_goods();
    let services_list = goods.service_goods();
    let pop_surplus_cents = |s: usize, g: &PopGroup| -> (i64, &PopTypeDef) {
        let pdef = pop_catalog.get(g.pop_type);
        let mut basic_monthly = i64::from(g.count) * 4;
        for &(sgid, per_100) in &staples_list {
            basic_monthly +=
                compute_per_100_demand(g.count, per_100) * goods.get(sgid).genesis_price_cents;
        }
        for &(svgid, per_100, _) in &services_list {
            basic_monthly +=
                compute_per_100_demand(g.count, per_100) * goods.get(svgid).genesis_price_cents;
        }
        let reserve = basic_monthly.max(CASH_SCALE) * i64::from(pdef.reserve_months.max(1));
        let inv_idx = pop_invs[s]
            .iter()
            .copied()
            .find(|&idx| {
                sec_views[s]
                    .pops
                    .groups
                    .iter()
                    .position(|x| x.id == g.id)
                    .and_then(|gi| pop_invs[s].get(gi).copied())
                    == Some(idx)
            })
            .unwrap_or(polity_inv);
        let avail_cash = invs[inv_idx].avail(cash);
        ((avail_cash - reserve).max(0), pdef)
    };

    // 2A. Household `PopGroup` staple consumption bids (`1.0x` for flow staples, `1.5x` for storable physical staples)
    if staple_per_100 > 0 {
        for s in 0..sec_count {
            for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                if g.count == 0 {
                    continue;
                }
                let need = compute_per_100_demand(g.count, staple_per_100);
                if need <= 0 {
                    continue;
                }
                let inv = pop_invs[s][gi];
                let pantry = invs[inv].avail(good);
                if !is_flow && pantry > need * 2 {
                    sells.push(SellSubmission {
                        sector_id: s as u16,
                        goods_inv: inv,
                        cash_inv: inv,
                        ask_price_cents: ma[s],
                        qty: pantry - need * 2,
                        allow_contract_pool: true,
                    });
                }
                let target_pantry = if is_flow { need } else { need * 3 / 2 };
                let want = target_pantry - pantry;
                if want > 0 {
                    let avail_cash = invs[inv].avail(cash);
                    if avail_cash < PRICE_TICK_CENTS {
                        continue;
                    }
                    let desired_bid = if pantry < need {
                        pct_price(ma[s], 125)
                    } else {
                        ma[s]
                    };
                    let (bid, eff_want) = if avail_cash >= want * desired_bid {
                        (desired_bid, want)
                    } else if avail_cash >= need * ma[s] {
                        let unit_cap =
                            ((avail_cash / need.max(1)) / PRICE_TICK_CENTS) * PRICE_TICK_CENTS;
                        let px = desired_bid.min(unit_cap.max(ma[s]));
                        let q = (avail_cash / px.max(1)).min(want).max(1);
                        (px, q)
                    } else if avail_cash >= ma[s] {
                        let q = (avail_cash / ma[s].max(1)).min(want).max(1);
                        (ma[s], q)
                    } else {
                        let affordable_px = (avail_cash / PRICE_TICK_CENTS) * PRICE_TICK_CENTS;
                        if affordable_px < PRICE_TICK_CENTS {
                            continue;
                        }
                        (affordable_px, 1)
                    };
                    buys.push(BuySubmission {
                        sector_id: s as u16,
                        goods_inv: inv,
                        cash_inv: inv,
                        bid_price_cents: bid,
                        qty: eff_want,
                        allow_contract_pool: true,
                    });
                }
            }
        }
    }

    // 2B. Household `PopGroup` local service demand (`1.0x` baseline + surplus cash enjoyment purchases!)
    if service_per_100 > 0 {
        for s in 0..sec_count {
            for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                if g.count == 0 {
                    continue;
                }
                let base_need = compute_per_100_demand(g.count, service_per_100);
                if base_need <= 0 {
                    continue;
                }
                let inv = pop_invs[s][gi];
                let cur_held = invs[inv].avail(good);
                let avail_cash = invs[inv].avail(cash);
                let (surplus, pdef) = pop_surplus_cents(s, g);
                let mut extra_units = 0i64;
                if surplus > 0 && pdef.surplus_service_permille > 0 {
                    let extra_budget = (surplus * i64::from(pdef.surplus_service_permille)) / 1000;
                    let bid_px = pct_price(ma[s], 112).max(1);
                    extra_units = (extra_budget / bid_px).min(base_need * 2);
                }
                let base_want = if avail_cash >= base_need * ma[s] {
                    (base_need - cur_held).max(0)
                } else {
                    (avail_cash / ma[s].max(1)).min((base_need - cur_held).max(0))
                };
                let total_want = base_want + extra_units;
                if total_want > 0 {
                    let bid_px = if extra_units > 0 {
                        pct_price(ma[s], 112)
                    } else {
                        ma[s]
                    };
                    buys.push(BuySubmission {
                        sector_id: s as u16,
                        goods_inv: inv,
                        cash_inv: inv,
                        bid_price_cents: bid_px,
                        qty: total_want,
                        allow_contract_pool: true,
                    });
                }
            }
        }
    }

    // 2C. Household `PopGroup` surplus financial investment bids on open security books (bonds & `Trust` shares)
    if is_security {
        for s in 0..sec_count {
            for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                if g.count == 0 {
                    continue;
                }
                let (surplus, pdef) = pop_surplus_cents(s, g);
                if surplus <= 0 || pdef.surplus_invest_permille == 0 {
                    continue;
                }
                let invest_budget = (surplus * i64::from(pdef.surplus_invest_permille)) / 1000;
                let bid_px = pct_price(ma[s], 102).max(1);
                let qty = invest_budget / bid_px;
                if qty > 0 {
                    let inv = pop_invs[s][gi];
                    buys.push(BuySubmission {
                        sector_id: s as u16,
                        goods_inv: inv,
                        cash_inv: inv,
                        bid_price_cents: bid_px,
                        qty,
                        allow_contract_pool: false,
                    });
                }
            }
        }
    }

    // 3. Sector State Depot operating policy + State Core public baseline flow outputs (e.g. `base:cap_land`)
    if is_flow && gdef.unit_volume == 0 && depot_target == 0 {
        for s in 0..sec_count {
            let depot = depot_invs[s];
            let qty = invs[depot].avail(good);
            if qty > 0 {
                sells.push(SellSubmission {
                    sector_id: s as u16,
                    goods_inv: depot,
                    cash_inv: polity_inv,
                    ask_price_cents: ma[s].max(gdef.genesis_price_cents),
                    qty,
                    allow_contract_pool: false,
                });
            }
        }
    }
    if depot_target > 0 {
        for s in 0..sec_count {
            let depot = depot_invs[s];
            match depot_policy[s] {
                DepotPolicy::Idle => {}
                DepotPolicy::Stabilize => {
                    let qty = invs[depot].avail(good);
                    if qty > 0 {
                        sells.push(SellSubmission {
                            sector_id: s as u16,
                            goods_inv: depot,
                            cash_inv: polity_inv,
                            ask_price_cents: pct_price(ma[s], 95),
                            qty,
                            allow_contract_pool: false,
                        });
                    }
                }
                DepotPolicy::Stockpile => {
                    let want = depot_target - invs[depot].total(good);
                    if want > 0 {
                        buys.push(BuySubmission {
                            sector_id: s as u16,
                            goods_inv: depot,
                            cash_inv: polity_inv,
                            bid_price_cents: ma[s],
                            qty: want,
                            allow_contract_pool: false,
                        });
                    }
                }
            }
        }
    }

    (sells, buys)
}
