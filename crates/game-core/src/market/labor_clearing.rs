//! Intra-Sector Local Labor Order-Book Clearing (`RoleWageBook`).

use super::CONTRACT_POOL_SHARE_PERCENT;
use serde::{Deserialize, Serialize};

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
    if bids.is_empty() {
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
