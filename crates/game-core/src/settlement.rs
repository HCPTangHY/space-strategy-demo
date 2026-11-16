//! Monthly settlement cycle. Every unit of cash and every apple moves between real `Inventory`s:
//! 1) labor contract distress layoffs, `1/24` rolling contract expiry & local labor order-book hiring
//! 2) production → 3) central bank minting → 4) simultaneous commodity order-book clearing
//! 5) locked contract wages → 6) profit sweep → 7) dividends → 8) head tax → 9) consumption → 10) spoilage.

use super::*;
use crate::economy::{
    DepotPolicy, StaffContract, BASE_SECTOR_DEPOT_CAPACITY, CAPITAL_DEPOT_CAPACITY_BONUS,
    CORE_DEPOT_CAPACITY_BONUS, DEPOT_STOCKPILE_TARGET,
};
use crate::market::{
    clear_local_labor_book, clear_planetary_good_market, release_resting_orders, snap_price_cents,
    BuySubmission, GoodId, Inventory, LaborAsk, LaborBid, OrderBookSnapshot, SectorOrderBook,
    SellSubmission, CASH_SCALE, MAX_ORDER_PRICE_CENTS, MIN_ORDER_PRICE_CENTS, MIN_WAGE_CENTS,
};

/// Index of the ruling polity's central vault inside the settlement inventory slice.
/// It also receives freight fees (state-run logistics network).
const POLITY_INV: usize = 0;
/// Workshops keep this many months of wage bill as working capital before sweeping profit to the owner.
const WORKSHOP_RESERVE_MONTHS: i64 = 2;
const WORKSHOP_MIN_RESERVE_CENTS: i64 = 20 * CASH_SCALE;
/// Share of monthly capital-pool inflow paid out to shareholder Pops (rest stays as investment pool).
const DIVIDEND_PAYOUT_PERCENT: i64 = 65;
/// Head tax per person per month, in cents.
const HEAD_TAX_CENTS_PER_PERSON: i64 = 4;

#[derive(Clone, Default)]
struct SectorSlots {
    depot: usize,
    capital: usize,
    pops: Vec<usize>,
}

struct WorkshopRef {
    tile: usize,
    slot: TileSlotKey,
    inv: usize,
}

#[derive(Default, Clone, Copy)]
struct WorkshopMonth {
    /// Uncapped staffing-scaled output (`raw`), used as the true denominator for unit cost!
    nominal_output: i64,
    /// Actual units stored after physical warehouse capacity cap (`produce_capped`).
    output: i64,
    wage_bill: i64,
    wage_paid: i64,
    revenue: i64,
}

#[inline]
fn cash_idx(cash: GoodId) -> usize {
    usize::from(cash.0)
}

/// Moves up to `amount` cents between two inventories (cash has zero volume, so no capacity check).
fn move_cash(invs: &mut [Inventory], cash: GoodId, from: usize, to: usize, amount: i64) -> i64 {
    let c = cash_idx(cash);
    let moved = amount.min(invs[from].available[c]).max(0);
    if moved > 0 && from != to {
        invs[from].available[c] -= moved;
        invs[to].available[c] += moved;
    }
    moved
}

/// Pays locked `StaffContract`s of one building slot directly to each `c.pop_id`'s `Inventory`.
/// Zero cross-employer or cross-culture wage averaging! Returns `(bill, paid, income_tax)`.
#[allow(clippy::too_many_arguments)]
fn pay_contract_wages(
    cash: GoodId,
    invs: &mut [Inventory],
    payer: usize,
    slots: &SectorSlots,
    sector_pops: &SectorPops,
    contracts: &[StaffContract],
    tax_permille: i64,
    pop_income: &mut [i64],
) -> (i64, i64, i64) {
    let active: Vec<(usize, i64)> = contracts
        .iter()
        .filter(|c| c.count > 0 && c.monthly_bill_cents > 0)
        .filter_map(|c| sector_pops.index_by_id(c.pop_id).map(|gi| (gi, c.monthly_bill_cents)))
        .collect();
    let bill: i64 = active.iter().map(|&(_, b)| b).sum();
    if bill <= 0 {
        return (0, 0, 0);
    }
    let budget = bill.min(invs[payer].avail(cash)).max(0);
    if budget <= 0 {
        return (bill, 0, 0);
    }

    let (mut paid, mut tax, mut distributed) = (0i64, 0i64, 0i64);
    for (idx, &(gi, c_bill)) in active.iter().enumerate() {
        let gross = if budget == bill {
            c_bill
        } else if idx + 1 == active.len() {
            budget - distributed
        } else {
            ((i128::from(c_bill) * i128::from(budget)) / i128::from(bill)) as i64
        };
        distributed += gross;
        if gross <= 0 {
            continue;
        }
        let t = gross * tax_permille / 1000;
        move_cash(invs, cash, payer, slots.pops[gi], gross - t);
        move_cash(invs, cash, payer, POLITY_INV, t);
        pop_income[gi] += gross - t;
        paid += gross;
        tax += t;
    }
    (bill, paid, tax)
}

#[inline]
fn pct_price(base_cents: i64, percent: i64) -> i64 {
    snap_price_cents(base_cents * percent / 100)
}

/// Computes a workshop's ask price from local `ma_cents`, `unit_cost_cents`, and warehouse fill ratio:
/// - Low inventory (`<= 35%` full): quotes at `max(unit_cost, 100% MA)` (no self-inflicted 92% deflation spiral!)
/// - Moderate inventory (`35%..=75%` full): gently trims `100% -> 95% MA`, floored at `unit_cost`
/// - Overstocked / near-full warehouse (`> 75%` full): discounts down to `80% MA` and relaxes the cost floor
///   down to `50% unit_cost` so full orchards liquidate excess stock for working capital instead of freezing!
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

impl Game {
    pub(crate) fn run_monthly_political_economy(&mut self) {
        let planet_list: Vec<(String, PolityId)> = self
            .world
            .query::<&Planet>()
            .iter()
            .map(|(_, p)| (p.id.clone(), p.ruling_polity))
            .collect();
        for (pid, ruling) in planet_list {
            self.settle_planet_month(&pid, ruling);
            let sec_allow_land: BTreeMap<SectorId, bool> = self
                .world
                .query::<&Sector>()
                .iter()
                .filter(|(_, s)| s.planet_id == pid)
                .map(|(_, s)| (s.id, s.allow_private_land_use))
                .collect();
            self.run_autonomous_private_investment(&pid, &sec_allow_land);
        }
    }

    /// Step 1 of monthly settlement:
    /// - Unprofitable / cash-depleted / arrears workplaces downsize into `(same_role, Unowned)` (`XX · 待业`);
    /// - `1/24` of salaried contracts mature: solvent profitable employers renew in place at `MA`,
    ///   while downsizing employers release expiring staff into `(same_role, Unowned)`;
    /// - Local intra-sector Labor Order Books match open positions against `(same_role, Unowned)` first,
    ///   then qualified `promote_from` pools (`流民` / `个体民`), locking `StaffContract`s on each slot!
    fn settle_labor_contracts_for_month(&mut self, pid: &str, ruling: PolityId) {
        let goods = &self.goods;
        let catalog = &self.pop_catalog;
        let cash = goods.cash_id();
        let Some(apple) = goods.id_of(GOOD_APPLE) else {
            return;
        };
        let default_pool = catalog.default_pool_id();
        let vag_pool = catalog.vagrant_pool_id();
        let type_count = catalog.len();
        let month_seq = self.counters.monthly_ticks as u32;

        let mut treasury_budget = self
            .world
            .query::<&Polity>()
            .iter()
            .find_map(|(_, pol)| (pol.id == ruling).then(|| pol.cash_cents(goods)))
            .unwrap_or(0);

        let mut cap_budgets: BTreeMap<u16, i64> = BTreeMap::new();
        for (_, (sec, cap)) in self.world.query::<(&Sector, &SectorCapital)>().iter() {
            if sec.planet_id == pid {
                cap_budgets.insert(sec.id.0, cap.pool_cents(goods));
            }
        }

        // Order employable occupations by descending base wage so high-skill roles get first pick.
        let mut role_order: Vec<PopTypeId> = (0..type_count)
            .map(|i| PopTypeId(i as u16))
            .filter(|&r| r != default_pool && r != vag_pool)
            .collect();
        role_order.sort_by(|&a, &b| {
            catalog
                .get(b)
                .base_wage_per_100
                .cmp(&catalog.get(a).base_wage_per_100)
                .then_with(|| a.cmp(&b))
        });

        let mut sector_ids: Vec<u16> = self
            .world
            .query::<&Sector>()
            .iter()
            .filter_map(|(_, s)| (s.planet_id == pid).then_some(s.id.0))
            .collect();
        sector_ids.sort_unstable();

        for sec_u16 in sector_ids {
            let sec_id = SectorId(sec_u16);

            // Snapshot sector's `SectorPops` & `SectorMarket` so we can mutate alongside `PlanetTiles`.
            let mut pops_opt: Option<SectorPops> = None;
            let mut wage_books: Vec<RoleWageBook> = Vec::new();
            let mut apple_ma = market::GENESIS_APPLE_PRICE_CENTS;
            for (_, (sec, mkt, pops)) in self
                .world
                .query_mut::<(&Sector, &mut SectorMarket, &mut SectorPops)>()
            {
                if sec.planet_id == pid && sec.id == sec_id {
                    apple_ma = mkt.apple_book.ma_price_cents;
                    if mkt.wage_books.len() < type_count {
                        for i in mkt.wage_books.len()..type_count {
                            let base_w = catalog.defs()[i].base_wage_per_100 * CASH_SCALE / 100;
                            mkt.wage_books.push(RoleWageBook::new(i as u16, base_w));
                        }
                    }
                    for wb in &mut mkt.wage_books {
                        wb.last_laid_off = 0;
                        wb.last_hired = 0;
                    }
                    wage_books = std::mem::take(&mut mkt.wage_books);
                    pops_opt = Some(std::mem::take(pops));
                    break;
                }
            }
            let Some(mut pops) = pops_opt else {
                continue;
            };

            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id != pid {
                    continue;
                }

                // Collect active building slots in this sector sorted by hiring priority.
                let mut active_slots: Vec<(usize, TileSlotKey, &'static BuildingDef, u16)> =
                    Vec::new();
                for tile in &pt.tiles {
                    if tile.sector_id != sec_u16 {
                        continue;
                    }
                    let t_idx = usize::from(tile.index);
                    let hab = tile.tags.habitability_permille();
                    for (sk, tid) in tile.slots.iter_occupied() {
                        active_slots.push((t_idx, sk, BuildingDef::by_type_id(tid), hab));
                    }
                }
                active_slots.sort_by(|a, b| {
                    a.2.job_priority
                        .cmp(&b.2.job_priority)
                        .then_with(|| {
                            b.2.tags
                                .has(BuildingTag::PlanetaryCapital)
                                .cmp(&a.2.tags.has(BuildingTag::PlanetaryCapital))
                        })
                        .then_with(|| b.3.cmp(&a.3))
                        .then_with(|| a.1.as_index().cmp(&b.1.as_index()))
                        .then_with(|| a.0.cmp(&b.0))
                });

                // Determine active enterprises for accurate marginal revenue product calculation.
                let mut ent_active = vec![false; pt.enterprises.len()];
                for (e_i, ent) in pt.enterprises.iter().enumerate() {
                    if let Some(ctile) = pt.tiles.get(ent.campus_tile_index) {
                        if let Some(ctid) = ctile.slots.primary {
                            let cdef = BuildingDef::by_type_id(ctid);
                            let req = cdef.slot_jobs_required(TileSlotKey::Primary);
                            let emp: u32 = pt.slot_contracts[ent.campus_tile_index]
                                [TileSlotKey::Primary.as_index()]
                                .iter()
                                .map(|c| c.count)
                                .sum();
                            if req == 0 || emp * 2 >= req {
                                ent_active[e_i] = true;
                            }
                        }
                    }
                }

                // Deduct already-committed state & capital-pool bills in this sector first.
                // ── 1A. Distress Layoffs (workshops or private hubs unable to sustain payroll) ──
                for &(t_idx, sk, def, _) in &active_slots {
                    let owner = pt.slot_owner(t_idx, sk);
                    let bonus = match owner {
                        BuildingOwner::Enterprise { enterprise_id }
                            if ent_active
                                .get(usize::from(enterprise_id.0))
                                .copied()
                                .unwrap_or(false) =>
                        {
                            ENTERPRISE_OUTPUT_BONUS_PERMILLE
                        }
                        _ => 0,
                    };

                    if let Some(ws) = pt.workshops[t_idx][sk.as_index()].as_ref() {
                        let contracts = &mut pt.slot_contracts[t_idx][sk.as_index()];
                        let cur_emp: u32 = contracts.iter().map(|c| c.count).sum();
                        let locked_bill: i64 = contracts.iter().map(|c| c.monthly_bill_cents).sum();
                        if locked_bill > 0 {
                            // Bound expected output by physical remaining warehouse capacity AND prior real cash sales!
                            let free_cap = ws.inv.free_volume(goods);
                            let raw_out = slot_apple_output(def, sk, cur_emp, bonus);
                            let producible_out = raw_out.min(free_cap);
                            let ws_cash = ws.inv.avail(cash);
                            let expected_inflow = if ws.last_revenue_cents < 0 {
                                producible_out * apple_ma
                            } else if ws.had_wage_arrears {
                                ws.last_revenue_cents.max(0)
                            } else {
                                ws.last_revenue_cents
                                    .max(0)
                                    .max((producible_out * apple_ma) / 2)
                            };
                            // Lay off immediately if wage arrears occurred last month OR cash + inflow cannot cover payroll!
                            if ws.had_wage_arrears || ws_cash + expected_inflow / 2 < locked_bill {
                                let sustainable_bill = ws_cash
                                    .max((expected_inflow * 85) / 100)
                                    .max(0);
                                for c in contracts.iter_mut() {
                                    if c.count == 0 || c.monthly_bill_cents <= 0 {
                                        continue;
                                    }
                                    let keep = ((u128::from(c.count)
                                        * u128::from(sustainable_bill as u64))
                                        / u128::from(locked_bill as u64))
                                        as u32;
                                    let cut = c.count.saturating_sub(keep);
                                    if cut > 0 {
                                        let cut_bill = if cut == c.count {
                                            c.monthly_bill_cents
                                        } else {
                                            (c.monthly_bill_cents * i64::from(cut))
                                                / i64::from(c.count)
                                        };
                                        c.count -= cut;
                                        c.monthly_bill_cents -= cut_bill;
                                        if let Some(gi) = pops.index_by_id(c.pop_id) {
                                            let r_i = usize::from(pops.groups[gi].pop_type.0);
                                            if let Some(wb) = wage_books.get_mut(r_i) {
                                                wb.last_laid_off += cut;
                                            }
                                        }
                                        pops.release_to_between_jobs(goods, c.pop_id, cut, 8);
                                    }
                                }
                                contracts.retain(|c| c.count > 0);
                            }
                        }
                    } else if !def.is_state_core_or_public() {
                        // Commercial / Corporate Campus backed by `SectorCapital`
                        let cap_sec = match owner {
                            BuildingOwner::Enterprise { enterprise_id } => pt
                                .enterprises
                                .get(usize::from(enterprise_id.0))
                                .map(|e| e.hq_sector.0)
                                .unwrap_or(sec_u16),
                            _ => sec_u16,
                        };
                        let avail_pool = cap_budgets.get(&cap_sec).copied().unwrap_or(0);
                        let contracts = &mut pt.slot_contracts[t_idx][sk.as_index()];
                        let locked_bill: i64 = contracts.iter().map(|c| c.monthly_bill_cents).sum();
                        if locked_bill > avail_pool {
                            let sustainable = avail_pool.max(0);
                            for c in contracts.iter_mut() {
                                if c.count == 0 || c.monthly_bill_cents <= 0 {
                                    continue;
                                }
                                let keep = ((u128::from(c.count) * u128::from(sustainable as u64))
                                    / u128::from(locked_bill as u64))
                                    as u32;
                                let cut = c.count.saturating_sub(keep);
                                if cut > 0 {
                                    let cut_bill = if cut == c.count {
                                        c.monthly_bill_cents
                                    } else {
                                        (c.monthly_bill_cents * i64::from(cut)) / i64::from(c.count)
                                    };
                                    c.count -= cut;
                                    c.monthly_bill_cents -= cut_bill;
                                    if let Some(gi) = pops.index_by_id(c.pop_id) {
                                        let r_i = usize::from(pops.groups[gi].pop_type.0);
                                        if let Some(wb) = wage_books.get_mut(r_i) {
                                            wb.last_laid_off += cut;
                                        }
                                    }
                                    pops.release_to_between_jobs(goods, c.pop_id, cut, 6);
                                }
                            }
                            contracts.retain(|c| c.count > 0);
                        }
                        let rem_bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .map(|c| c.monthly_bill_cents)
                            .sum();
                        if let Some(b) = cap_budgets.get_mut(&cap_sec) {
                            *b -= rem_bill;
                        }
                    } else {
                        let rem_bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .map(|c| c.monthly_bill_cents)
                            .sum();
                        treasury_budget -= rem_bill;
                    }
                }

                // Track pre-expiry salaried headcount per role so non-renewals count as `last_laid_off`.
                let mut pre_expiry_role_emp = vec![0u32; type_count];
                for &(t_idx, sk, _, _) in &active_slots {
                    for c in &pt.slot_contracts[t_idx][sk.as_index()] {
                        if let Some(gi) = pops.index_by_id(c.pop_id) {
                            let r_i = usize::from(pops.groups[gi].pop_type.0);
                            if r_i < type_count {
                                pre_expiry_role_emp[r_i] += c.count;
                            }
                        }
                    }
                }

                // ── 1B. 24-Month Rolling Contract Maturity (`(count + 23 - phase) / 24`) ────
                // Solvent employers who still need the slot renew in place at `ma_wage_cents` (no trip
                // through `Unowned`, zero wallet dilution!); overstocked/insolvent employers release to `待业`.
                for &(t_idx, sk, def, _) in &active_slots {
                    let slot_salt =
                        (t_idx as u32).wrapping_mul(7).wrapping_add(sk.as_index() as u32 * 5);
                    let phase = month_seq.wrapping_add(slot_salt) % 24;
                    let owner = pt.slot_owner(t_idx, sk);
                    let bonus = match owner {
                        BuildingOwner::Enterprise { enterprise_id }
                            if ent_active
                                .get(usize::from(enterprise_id.0))
                                .copied()
                                .unwrap_or(false) =>
                        {
                            ENTERPRISE_OUTPUT_BONUS_PERMILLE
                        }
                        _ => 0,
                    };
                    let mut slot_bill_now: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                        .iter()
                        .map(|c| c.monthly_bill_cents)
                        .sum();

                    for c in &mut pt.slot_contracts[t_idx][sk.as_index()] {
                        if c.count == 0 {
                            continue;
                        }
                        let Some(gi) = pops.index_by_id(c.pop_id) else {
                            continue;
                        };
                        let role_id = pops.groups[gi].pop_type;
                        let r_i = usize::from(role_id.0);
                        if catalog.get(role_id).base_wage_per_100 <= 0 {
                            continue;
                        }
                        let expiring = (c.count + 23 - phase) / 24;
                        if expiring == 0 {
                            continue;
                        }
                        let exp_bill = if expiring == c.count {
                            c.monthly_bill_cents
                        } else {
                            (c.monthly_bill_cents * i64::from(expiring)) / i64::from(c.count)
                        };
                        let ma_w = wage_books
                            .get(r_i)
                            .map(|wb| wb.ma_wage_cents)
                            .unwrap_or(0);
                        let new_exp_bill = i64::from(expiring) * ma_w;

                        let can_renew_in_place = if let Some(ws) =
                            pt.workshops[t_idx][sk.as_index()].as_ref()
                        {
                            let inv_fill_permille =
                                (ws.inv.avail(apple) * 1000) / ws.inv.capacity.max(1);
                            let req = def.slot_jobs_required(sk).max(1);
                            let mrp_cents =
                                (slot_apple_output(def, sk, req, bonus) * apple_ma) / i64::from(req);
                            let bill_after = slot_bill_now - exp_bill + new_exp_bill;
                            !ws.had_wage_arrears
                                && inv_fill_permille < 880
                                && mrp_cents >= (ma_w * 85) / 100
                                && (ws.inv.avail(cash) >= bill_after || ws.last_revenue_cents < 0)
                        } else if def.is_state_core_or_public() {
                            treasury_budget + exp_bill >= new_exp_bill
                        } else {
                            let cap_sec = match owner {
                                BuildingOwner::Enterprise { enterprise_id } => pt
                                    .enterprises
                                    .get(usize::from(enterprise_id.0))
                                    .map(|e| e.hq_sector.0)
                                    .unwrap_or(sec_u16),
                                _ => sec_u16,
                            };
                            let avail = cap_budgets.get(&cap_sec).copied().unwrap_or(0);
                            avail + exp_bill >= new_exp_bill
                        };

                        if can_renew_in_place {
                            c.monthly_bill_cents = c.monthly_bill_cents - exp_bill + new_exp_bill;
                            slot_bill_now = slot_bill_now - exp_bill + new_exp_bill;
                            if pt.workshops[t_idx][sk.as_index()].is_none() {
                                if def.is_state_core_or_public() {
                                    treasury_budget = treasury_budget + exp_bill - new_exp_bill;
                                } else {
                                    let cap_sec = match owner {
                                        BuildingOwner::Enterprise { enterprise_id } => pt
                                            .enterprises
                                            .get(usize::from(enterprise_id.0))
                                            .map(|e| e.hq_sector.0)
                                            .unwrap_or(sec_u16),
                                        _ => sec_u16,
                                    };
                                    if let Some(b) = cap_budgets.get_mut(&cap_sec) {
                                        *b = *b + exp_bill - new_exp_bill;
                                    }
                                }
                            }
                        } else {
                            c.count -= expiring;
                            c.monthly_bill_cents -= exp_bill;
                            slot_bill_now -= exp_bill;
                            if pt.workshops[t_idx][sk.as_index()].is_none() {
                                if def.is_state_core_or_public() {
                                    treasury_budget += exp_bill;
                                } else {
                                    let cap_sec = match owner {
                                        BuildingOwner::Enterprise { enterprise_id } => pt
                                            .enterprises
                                            .get(usize::from(enterprise_id.0))
                                            .map(|e| e.hq_sector.0)
                                            .unwrap_or(sec_u16),
                                        _ => sec_u16,
                                    };
                                    if let Some(b) = cap_budgets.get_mut(&cap_sec) {
                                        *b += exp_bill;
                                    }
                                }
                            }
                            pops.release_to_between_jobs(goods, c.pop_id, expiring, 2);
                        }
                    }
                    pt.slot_contracts[t_idx][sk.as_index()].retain(|c| c.count > 0);
                }

                // ── 1C. Local Labor Order Book Clearing per Occupation ─────────────────────
                for &role_id in &role_order {
                    let r_i = usize::from(role_id.0);
                    let r_def = catalog.get(role_id);
                    let ma_w = wage_books[r_i].ma_wage_cents;

                    // 1) Collect employer `LaborBid`s for open + renewing positions
                    let mut bids: Vec<LaborBid> = Vec::new();
                    for (slot_ref, &(t_idx, sk, def, _)) in active_slots.iter().enumerate() {
                        let scale = def.scale_for_slot(sk);
                        let mut req = 0u32;
                        for &(role_str, base_req) in def.jobs_by_role {
                            if def.resolve_role_id(catalog, role_str) == role_id {
                                req += base_req * scale;
                            }
                        }
                        if req == 0 {
                            continue;
                        }
                        let cur_role_emp: u32 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .filter(|c| {
                                pops.index_by_id(c.pop_id)
                                    .map(|gi| pops.groups[gi].pop_type == role_id)
                                    .unwrap_or(false)
                            })
                            .map(|c| c.count)
                            .sum();
                        if cur_role_emp >= req {
                            continue;
                        }
                        let raw_vac = req - cur_role_emp;
                        let owner = pt.slot_owner(t_idx, sk);

                        if let Some(ws) = pt.workshops[t_idx][sk.as_index()].as_ref() {
                            let bonus = match owner {
                                BuildingOwner::Enterprise { enterprise_id }
                                    if ent_active
                                        .get(usize::from(enterprise_id.0))
                                        .copied()
                                        .unwrap_or(false) =>
                                {
                                    ENTERPRISE_OUTPUT_BONUS_PERMILLE
                                }
                                _ => 0,
                            };
                            let full_out = slot_apple_output(def, sk, req, bonus);
                            let mrp_cents = (full_out * apple_ma) / i64::from(req.max(1));
                            let cur_bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                                .iter()
                                .map(|c| c.monthly_bill_cents)
                                .sum();
                            let free_cash = (ws.inv.avail(cash) - cur_bill).max(0);
                            let inv_fill_permille =
                                (ws.inv.avail(apple) * 1000) / ws.inv.capacity.max(1);

                            // Jammed warehouse, prior wage arrears, or unprofitable workshop stops hiring!
                            if ws.had_wage_arrears
                                || inv_fill_permille >= 900
                                || (ma_w > 0 && mrp_cents < (ma_w * 80) / 100 && cur_role_emp > 0)
                            {
                                continue;
                            }
                            let bid_wage = if ma_w <= 0 {
                                0
                            } else if mrp_cents < ma_w {
                                mrp_cents.max(MIN_WAGE_CENTS)
                            } else if cur_role_emp * 10 < req * 8 && mrp_cents >= (ma_w * 125) / 100
                            {
                                // Understaffed profitable workshop bids above MA to attract scarce labor!
                                ((ma_w * 108 + 50) / 100)
                                    .min((mrp_cents * 85) / 100)
                                    .max(ma_w)
                            } else {
                                // Profitable workshop with sufficient staff pays 100% MA (excess profit -> owner).
                                ma_w
                            };
                            let afford = if bid_wage > 0 {
                                (free_cash / bid_wage).max(0) as u32
                            } else {
                                raw_vac
                            };
                            let wanted = raw_vac.min(afford);
                            if wanted > 0 {
                                bids.push(LaborBid {
                                    slot_ref,
                                    bid_wage_cents: bid_wage,
                                    wanted_heads: wanted,
                                });
                            }
                        } else if def.is_state_core_or_public() {
                            let bid_wage = if ma_w <= 0 {
                                0
                            } else if cur_role_emp * 10 < req * 9 {
                                (ma_w * 108 + 50) / 100
                            } else {
                                ma_w
                            };
                            let afford = if bid_wage > 0 {
                                (treasury_budget.max(0) / bid_wage) as u32
                            } else {
                                raw_vac
                            };
                            let wanted = raw_vac.min(afford);
                            if wanted > 0 {
                                bids.push(LaborBid {
                                    slot_ref,
                                    bid_wage_cents: bid_wage,
                                    wanted_heads: wanted,
                                });
                            }
                        } else {
                            let cap_sec = match owner {
                                BuildingOwner::Enterprise { enterprise_id } => pt
                                    .enterprises
                                    .get(usize::from(enterprise_id.0))
                                    .map(|e| e.hq_sector.0)
                                    .unwrap_or(sec_u16),
                                _ => sec_u16,
                            };
                            let pool_cash = cap_budgets.get(&cap_sec).copied().unwrap_or(0);
                            let bid_wage = if ma_w <= 0 {
                                0
                            } else if cur_role_emp * 10 < req * 9 {
                                (ma_w * 106 + 50) / 100
                            } else {
                                ma_w
                            };
                            let afford = if bid_wage > 0 {
                                (pool_cash.max(0) / bid_wage) as u32
                            } else {
                                raw_vac
                            };
                            let wanted = raw_vac.min(afford);
                            if wanted > 0 {
                                bids.push(LaborBid {
                                    slot_ref,
                                    bid_wage_cents: bid_wage,
                                    wanted_heads: wanted,
                                });
                            }
                        }
                    }

                    if bids.is_empty() {
                        let wb = &mut wage_books[r_i];
                        wb.prev_wage_cents = wb.last_wage_cents;
                        wb.last_hired = 0;
                        wb.best_ask_wage_cents = None;
                        wb.best_bid_wage_cents = None;
                        continue;
                    }

                    // 2) Collect job-seeker `LaborAsk`s:
                    //    (a) Same-occupation `Unowned` (`XX · 待业`) ALWAYS get first priority!
                    //    (b) Cross-occupation `promote_from` pools (`流民` for manual/military, `个体民`/lower roles
                    //        with qualification reservation floor so unskilled vagrants never underbid Scholars!).
                    let mut iter_sources: Vec<PopTypeId> = vec![role_id];
                    for sid in &r_def.promote_from {
                        if let Some(pid) = catalog.id_of(sid) {
                            if !iter_sources.contains(&pid) {
                                iter_sources.push(pid);
                            }
                        }
                    }
                    if iter_sources.len() == 1 {
                        for fb in [vag_pool, default_pool] {
                            if !iter_sources.contains(&fb) {
                                iter_sources.push(fb);
                            }
                        }
                    }

                    let food_floor_cents = (apple_ma * 4 + 99) / 100;
                    let genesis_role_wage = r_def.base_wage_per_100 * CASH_SCALE / 100;
                    let mut asks: Vec<LaborAsk> = Vec::new();
                    for &src_type in &iter_sources {
                        let is_same_role = src_type == role_id;
                        let is_vagrant_src =
                            catalog.has_dedicated_vagrant_pool() && src_type == vag_pool;
                        let mut rem_cap = if is_same_role || is_vagrant_src {
                            u32::MAX
                        } else if r_def.base_wage_per_100 >= 85 || r_def.receives_dividends {
                            30u32
                        } else {
                            70u32
                        };
                        for (gi, g) in pops.groups.iter().enumerate() {
                            if rem_cap == 0 {
                                break;
                            }
                            if g.pop_type != src_type
                                || g.employer != BuildingOwner::Unowned
                                || g.count == 0
                            {
                                continue;
                            }
                            let offer_heads = g.count.min(rem_cap);
                            rem_cap = rem_cap.saturating_sub(offer_heads);

                            let ask_wage = if ma_w <= 0 {
                                0
                            } else if is_same_role {
                                // Experienced between-jobs seekers of this exact occupation:
                                let monthly_food_bill =
                                    economy::compute_apple_demand(g.count) * apple_ma;
                                let sav = g.inv.avail(cash);
                                let met = g.last_food_met_permille;
                                if met >= 900 && sav >= monthly_food_bill * 2 {
                                    ma_w.max(food_floor_cents)
                                } else if met >= 750 && sav >= monthly_food_bill / 2 {
                                    (ma_w * 92) / 100
                                } else {
                                    (ma_w * 82) / 100
                                }
                            } else if is_vagrant_src {
                                // Destitute `流民` (only eligible on manual/soldier books via `promote_from`):
                                let sav = g.inv.avail(cash);
                                if g.last_food_met_permille >= 850 && sav > 0 {
                                    (ma_w * 90) / 100
                                } else {
                                    (ma_w * 78) / 100
                                }
                            } else {
                                // Cross-occupation trainees (`个体民` or other role retraining):
                                // never undercut current `MA` or 95% of baseline qualification wage!
                                ma_w.max((genesis_role_wage * 95) / 100)
                                    .max((food_floor_cents * 110) / 100)
                            };
                            asks.push(LaborAsk {
                                seeker_group_idx: gi,
                                ask_wage_cents: ask_wage,
                                available_heads: offer_heads,
                            });
                        }
                    }

                    // 3) Clear this occupation's local order book & lock signed contracts!
                    let fills = clear_local_labor_book(&mut wage_books[r_i], &asks, &bids);
                    for fill in fills {
                        if fill.hired == 0 {
                            continue;
                        }
                        let (t_idx, sk, def, _) = active_slots[fill.slot_ref];
                        let owner = pt.slot_owner(t_idx, sk);
                        let cid = pops.groups[fill.seeker_group_idx].culture_id;
                        let dst_idx = pops.ensure_group_index(goods, role_id, cid, owner);
                        let dst_id = pops.groups[dst_idx].id;
                        let actual = pops.transfer_between_groups(
                            goods,
                            fill.seeker_group_idx,
                            dst_idx,
                            fill.hired,
                        );
                        if actual > 0 {
                            let add_bill = i64::from(actual) * fill.wage_cents;
                            pt.add_slot_contract(t_idx, sk, dst_id, actual, add_bill);
                            if pt.workshops[t_idx][sk.as_index()].is_none() && add_bill > 0 {
                                if def.is_state_core_or_public() {
                                    treasury_budget -= add_bill;
                                } else {
                                    let cap_sec = match owner {
                                        BuildingOwner::Enterprise { enterprise_id } => pt
                                            .enterprises
                                            .get(usize::from(enterprise_id.0))
                                            .map(|e| e.hq_sector.0)
                                            .unwrap_or(sec_u16),
                                        _ => sec_u16,
                                    };
                                    if let Some(b) = cap_budgets.get_mut(&cap_sec) {
                                        *b -= add_bill;
                                    }
                                }
                            }
                        }
                    }
                }

                // Count net non-renewals after maturity + clearing into `last_laid_off`.
                let mut post_role_emp = vec![0u32; type_count];
                for &(t_idx, sk, _, _) in &active_slots {
                    for c in &pt.slot_contracts[t_idx][sk.as_index()] {
                        if let Some(gi) = pops.index_by_id(c.pop_id) {
                            let r_i = usize::from(pops.groups[gi].pop_type.0);
                            if r_i < type_count {
                                post_role_emp[r_i] += c.count;
                            }
                        }
                    }
                }
                for r_i in 0..type_count {
                    if post_role_emp[r_i] < pre_expiry_role_emp[r_i] {
                        wage_books[r_i].last_laid_off += pre_expiry_role_emp[r_i] - post_role_emp[r_i];
                    }
                }

                let ref_ids = pt.referenced_pop_ids_in_sector(sec_id);
                pops.prune_empty_groups(goods, default_pool, &ref_ids);
                break;
            }

            // Write updated `pops` & `wage_books` back to the ECS sector entity.
            for (_, (sec, mkt, spops)) in self
                .world
                .query_mut::<(&Sector, &mut SectorMarket, &mut SectorPops)>()
            {
                if sec.planet_id == pid && sec.id == sec_id {
                    mkt.wage_books = wage_books;
                    *spops = pops;
                    break;
                }
            }
        }
    }

    pub(crate) fn settle_planet_month(&mut self, pid: &str, ruling: PolityId) {
        // ── 0. Settle labor contracts (distress layoffs, 1/24 maturity, local labor book) ──
        self.settle_labor_contracts_for_month(pid, ruling);

        let goods = &self.goods;
        let catalog = &self.pop_catalog;
        let world = &mut self.world;
        let cash = goods.cash_id();
        let Some(apple) = goods.id_of(GOOD_APPLE) else {
            return;
        };

        // ── 1. Snapshot Pops for post-hiring contract & output evaluation ─────
        let mut sec_views: Vec<SectorSimView> = Vec::new();
        let mut sec_hab: BTreeMap<SectorId, u32> = BTreeMap::new();
        for (_, (sec, pops)) in world.query::<(&Sector, &SectorPops)>().iter() {
            if sec.planet_id == pid {
                sec_hab.insert(sec.id, sec.habitability_sum_permille);
                sec_views.push(SectorSimView {
                    sector_id: sec.id,
                    pops: pops.clone(),
                });
            }
        }
        sec_views.sort_by_key(|v| v.sector_id);
        let sec_count = sec_views.len();
        if sec_count == 0 {
            return;
        }

        // ── 2. Extract every participant Inventory into one flat slice ─────────
        let mut invs: Vec<Inventory> = Vec::new();
        let mut tax_permille = 150i64;
        let mut mint_permille = 0i64;
        for (_, pol) in world.query_mut::<&mut Polity>() {
            if pol.id == ruling {
                tax_permille = i64::from(pol.tax_rate_permille);
                mint_permille = pol.monetary_policy.monthly_permille();
                invs.push(std::mem::take(&mut pol.vault));
                break;
            }
        }
        if invs.is_empty() {
            return;
        }
        let vault_before = invs[POLITY_INV].avail(cash);

        let mut slots: Vec<SectorSlots> = vec![SectorSlots::default(); sec_count];
        let mut books: Vec<SectorOrderBook> = (0..sec_count)
            .map(|_| SectorOrderBook::new(apple, 0))
            .collect();
        let mut depot_policy = vec![DepotPolicy::Idle; sec_count];
        for (_, (sec, market, cap, pops)) in world.query_mut::<(
            &mut Sector,
            &mut SectorMarket,
            &mut SectorCapital,
            &mut SectorPops,
        )>() {
            let s = usize::from(sec.id.0);
            if sec.planet_id != pid || s >= sec_count {
                continue;
            }
            slots[s].depot = invs.len();
            invs.push(std::mem::take(&mut sec.state_depot));
            slots[s].capital = invs.len();
            invs.push(std::mem::take(&mut cap.vault));
            slots[s].pops = pops
                .groups
                .iter_mut()
                .map(|g| {
                    g.sync_capacity();
                    invs.push(std::mem::take(&mut g.inv));
                    invs.len() - 1
                })
                .collect();
            books[s] = std::mem::replace(&mut market.apple_book, SectorOrderBook::new(apple, 0));
            depot_policy[s] = sec.depot_policy;
        }
        for inv in invs.iter_mut() {
            inv.ensure_len(goods.len());
        }

        let mut sec_prod = vec![0i64; sec_count];
        let mut sec_sub = vec![0i64; sec_count];
        let mut sec_cons = vec![0i64; sec_count];
        let mut depot_capacity = vec![BASE_SECTOR_DEPOT_CAPACITY; sec_count];
        let mut depth: Vec<OrderBookSnapshot> = Vec::with_capacity(sec_count);
        let mut pop_income: Vec<Vec<i64>> = sec_views
            .iter()
            .map(|v| vec![0i64; v.pops.groups.len()])
            .collect();
        let mut food_met: Vec<Vec<u16>> = sec_views
            .iter()
            .map(|v| vec![1000u16; v.pops.groups.len()])
            .collect();
        let (mut tax_income, mut admin_upkeep, mut state_net, mut freight_rev) =
            (0i64, 0i64, 0i64, 0i64);
        let mut capital_inflow = vec![0i64; sec_count];
        let mut dividends_gross = vec![0i64; sec_count];
        let mut minted = 0i64;
        let mut eval_out: Option<PlanetSimEvaluation> = None;

        for (_, (planet, pt)) in world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != pid {
                continue;
            }
            let eval = evaluate_planet_economy(catalog, pt, &sec_views);

            // Depot capacity follows the administrative buildings in each sector.
            for tile in &pt.tiles {
                let s = usize::from(tile.sector_id);
                if s >= sec_count {
                    continue;
                }
                if let Some(tid) = tile.slots.primary {
                    let def = BuildingDef::by_type_id(tid);
                    if def.tags.has(BuildingTag::RegionalCore) {
                        depot_capacity[s] += CORE_DEPOT_CAPACITY_BONUS;
                    }
                    if def.tags.has(BuildingTag::PlanetaryCapital) {
                        depot_capacity[s] += CAPITAL_DEPOT_CAPACITY_BONUS;
                    }
                }
            }
            for s in 0..sec_count {
                invs[slots[s].depot].capacity = depot_capacity[s];
            }

            let mut ws_refs: Vec<WorkshopRef> = Vec::new();
            let mut ws_at: Vec<[Option<usize>; SLOTS_PER_TILE]> =
                vec![[None; SLOTS_PER_TILE]; pt.tiles.len()];
            for t in 0..pt.tiles.len() {
                for sk in TileSlotKey::ALL {
                    if let Some(ws) = pt.workshops[t][sk.as_index()].as_mut() {
                        ws_at[t][sk.as_index()] = Some(ws_refs.len());
                        ws_refs.push(WorkshopRef {
                            tile: t,
                            slot: sk,
                            inv: invs.len(),
                        });
                        let mut inv = std::mem::take(&mut ws.inv);
                        inv.ensure_len(goods.len());
                        invs.push(inv);
                    }
                }
            }
            let mut ws_month = vec![WorkshopMonth::default(); ws_refs.len()];

            // ── 3. Production into the producer's own warehouse ──────────────
            for (k, w) in ws_refs.iter().enumerate() {
                let Some(tid) = pt.tiles[w.tile].slots.get(w.slot) else {
                    continue;
                };
                let def = BuildingDef::by_type_id(tid);
                let raw = slot_apple_output(
                    def,
                    w.slot,
                    eval.slot_employed[w.tile][w.slot.as_index()],
                    eval.slot_bonus_permille[w.tile][w.slot.as_index()],
                );
                let made = invs[w.inv].produce_capped(goods, apple, raw);
                ws_month[k].nominal_output = raw;
                ws_month[k].output = made;
                let s = usize::from(pt.tiles[w.tile].sector_id);
                if s < sec_count {
                    sec_prod[s] += made;
                }
            }
            for s in 0..sec_count {
                for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                    let sub_rate = catalog.get(g.pop_type).subsistence_apple_per_100;
                    if sub_rate > 0 && g.count > 0 {
                        let raw = (i64::from(g.count) * sub_rate) / 100;
                        sec_sub[s] += invs[slots[s].pops[gi]].produce_capped(goods, apple, raw);
                    }
                }
            }

            // ── 4. Central bank mints new credits into the polity vault ───────
            let supply: i64 = invs.iter().map(|i| i.total(cash)).sum();
            minted = supply * mint_permille / 1000;
            invs[POLITY_INV].available[cash_idx(cash)] += minted;

            // ── 5. Deterministic simultaneous quoting by every participant ────
            let ma: Vec<i64> = books.iter().map(|b| b.ma_price_cents).collect();
            let mut sells: Vec<SellSubmission> = Vec::new();
            let mut buys: Vec<BuySubmission> = Vec::new();
            for w in &ws_refs {
                let s = usize::from(pt.tiles[w.tile].sector_id);
                if s >= sec_count {
                    continue;
                }
                let qty = invs[w.inv].avail(apple);
                if qty <= 0 {
                    continue;
                }
                let unit_cost = pt.workshops[w.tile][w.slot.as_index()]
                    .as_ref()
                    .map(|ws| ws.last_unit_cost_cents)
                    .unwrap_or(0);
                let ask_price_cents =
                    compute_workshop_ask_cents(ma[s], unit_cost, qty, invs[w.inv].capacity);
                sells.push(SellSubmission {
                    sector_id: s as u16,
                    goods_inv: w.inv,
                    cash_inv: w.inv,
                    ask_price_cents,
                    qty,
                    allow_contract_pool: true,
                });
            }
            for s in 0..sec_count {
                for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                    if g.count == 0 {
                        continue;
                    }
                    let need = economy::compute_apple_demand(g.count);
                    let inv = slots[s].pops[gi];
                    let pantry = invs[inv].avail(apple);
                    if pantry > need * 2 {
                        sells.push(SellSubmission {
                            sector_id: s as u16,
                            goods_inv: inv,
                            cash_inv: inv,
                            ask_price_cents: ma[s],
                            qty: pantry - need * 2,
                            allow_contract_pool: true,
                        });
                    }
                    let want = need * 3 / 2 - pantry;
                    if want > 0 {
                        // Urgency is expressed only through the limit price: an empty pantry bids higher.
                        let bid = if pantry < need {
                            pct_price(ma[s], 125)
                        } else {
                            ma[s]
                        };
                        buys.push(BuySubmission {
                            sector_id: s as u16,
                            goods_inv: inv,
                            cash_inv: inv,
                            bid_price_cents: bid,
                            qty: want,
                            allow_contract_pool: true,
                        });
                    }
                }
                let depot = slots[s].depot;
                match depot_policy[s] {
                    DepotPolicy::Idle => {}
                    DepotPolicy::Stabilize => {
                        let qty = invs[depot].avail(apple);
                        if qty > 0 {
                            sells.push(SellSubmission {
                                sector_id: s as u16,
                                goods_inv: depot,
                                cash_inv: POLITY_INV,
                                ask_price_cents: pct_price(ma[s], 95),
                                qty,
                                allow_contract_pool: false,
                            });
                        }
                    }
                    DepotPolicy::Stockpile => {
                        let want = DEPOT_STOCKPILE_TARGET - invs[depot].total(apple);
                        if want > 0 {
                            buys.push(BuySubmission {
                                sector_id: s as u16,
                                goods_inv: depot,
                                cash_inv: POLITY_INV,
                                bid_price_cents: ma[s],
                                qty: want,
                                allow_contract_pool: false,
                            });
                        }
                    }
                }
            }

            let ws_cash_before: Vec<i64> = ws_refs.iter().map(|w| invs[w.inv].avail(cash)).collect();
            clear_planetary_good_market(
                goods,
                apple,
                &mut books,
                &pt.logistics,
                &mut invs,
                POLITY_INV,
                sells,
                buys,
            );
            for (d, book) in books.iter().enumerate() {
                freight_rev += book
                    .recent_trades
                    .iter()
                    .filter(|f| usize::from(f.to_sector) == d)
                    .map(|f| f.freight_cents * f.qty)
                    .sum::<i64>();
                depth.push(book.to_snapshot(5));
            }
            release_resting_orders(goods, apple, &mut books, &mut invs);
            for (k, w) in ws_refs.iter().enumerate() {
                ws_month[k].revenue = invs[w.inv].avail(cash) - ws_cash_before[k];
            }

            // ── 6. Locked Contract Wages (paid directly to each PopId!) ───────
            for tile in &pt.tiles {
                let s = usize::from(tile.sector_id);
                if s >= sec_count {
                    continue;
                }
                let t = usize::from(tile.index);
                for (sk, tid) in tile.slots.iter_occupied() {
                    let def = BuildingDef::by_type_id(tid);
                    let ws_k = ws_at[t][sk.as_index()];
                    let payer = if let Some(k) = ws_k {
                        ws_refs[k].inv
                    } else if def.is_state_core_or_public() {
                        POLITY_INV
                    } else if def.tags.has(BuildingTag::CorporateCampus) {
                        match pt.slot_owner(t, sk) {
                            BuildingOwner::Enterprise { enterprise_id } => pt
                                .enterprises
                                .get(usize::from(enterprise_id.0))
                                .map(|e| usize::from(e.hq_sector.0))
                                .filter(|&hq| hq < sec_count)
                                .map(|hq| slots[hq].capital)
                                .unwrap_or(slots[s].capital),
                            _ => slots[s].capital,
                        }
                    } else if def.tags.has(BuildingTag::Commercial) {
                        slots[s].capital
                    } else {
                        POLITY_INV
                    };
                    let (bill, paid, tax) = pay_contract_wages(
                        cash,
                        &mut invs,
                        payer,
                        &slots[s],
                        &sec_views[s].pops,
                        &pt.slot_contracts[t][sk.as_index()],
                        tax_permille,
                        &mut pop_income[s],
                    );
                    tax_income += tax;
                    if payer == POLITY_INV {
                        admin_upkeep += paid;
                    }
                    if let Some(k) = ws_k {
                        ws_month[k].wage_bill = bill;
                        ws_month[k].wage_paid = paid;
                    }
                }
            }

            // ── 7. Profit sweep: workshop cash above working capital goes to the owner ─
            let mut ent_profit = vec![0i64; pt.enterprises.len()];
            let mut ent_revenue = vec![0i64; pt.enterprises.len()];
            for (k, w) in ws_refs.iter().enumerate() {
                let reserve = (ws_month[k].wage_bill * WORKSHOP_RESERVE_MONTHS)
                    .max(WORKSHOP_MIN_RESERVE_CENTS);
                let excess = invs[w.inv].avail(cash) - reserve;
                let owner = pt.slot_owner(w.tile, w.slot);
                if let BuildingOwner::Enterprise { enterprise_id } = owner {
                    if let Some(r) = ent_revenue.get_mut(usize::from(enterprise_id.0)) {
                        *r += ws_month[k].revenue;
                    }
                }
                if excess <= 0 {
                    continue;
                }
                let home = match owner {
                    BuildingOwner::LocalPrivate { home_sector } => Some(usize::from(home_sector.0)),
                    BuildingOwner::Enterprise { enterprise_id } => {
                        let e = usize::from(enterprise_id.0);
                        if let Some(p) = ent_profit.get_mut(e) {
                            *p += excess;
                        }
                        pt.enterprises.get(e).map(|ent| usize::from(ent.hq_sector.0))
                    }
                    _ => None,
                }
                .filter(|&h| h < sec_count);
                match home {
                    Some(h) => {
                        let tax = excess * tax_permille / 1000;
                        move_cash(&mut invs, cash, w.inv, POLITY_INV, tax);
                        let net = move_cash(&mut invs, cash, w.inv, slots[h].capital, excess - tax);
                        tax_income += tax;
                        capital_inflow[h] += net;
                        dividends_gross[h] += excess;
                    }
                    None => {
                        state_net += move_cash(&mut invs, cash, w.inv, POLITY_INV, excess);
                    }
                }
            }
            for (e_i, ent) in pt.enterprises.iter_mut().enumerate() {
                ent.last_revenue_cents = ent_revenue[e_i];
                ent.last_profit_cents = ent_profit[e_i];
            }

            // ── 8. Dividends to shareholder Pops (across all cultures & employers!) ─
            let div_recipients = catalog.dividend_recipients();
            for s in 0..sec_count {
                let holders: Vec<(usize, u32)> = sec_views[s]
                    .pops
                    .groups
                    .iter()
                    .enumerate()
                    .filter_map(|(gi, g)| {
                        (g.count > 0 && div_recipients.contains(&g.pop_type))
                            .then_some((gi, g.count))
                    })
                    .collect();
                let total: u32 = holders.iter().map(|&(_, c)| c).sum();
                if total == 0 {
                    continue;
                }
                let payout = capital_inflow[s] * DIVIDEND_PAYOUT_PERCENT / 100;
                for &(gi, c) in &holders {
                    let share = payout * i64::from(c) / i64::from(total);
                    let moved =
                        move_cash(&mut invs, cash, slots[s].capital, slots[s].pops[gi], share);
                    pop_income[s][gi] += moved;
                }
            }

            // ── 9. Head tax, 10. consumption ────────────────────────────────
            for s in 0..sec_count {
                for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                    if g.count == 0 {
                        food_met[s][gi] = 1000;
                        continue;
                    }
                    let inv = slots[s].pops[gi];
                    tax_income += move_cash(
                        &mut invs,
                        cash,
                        inv,
                        POLITY_INV,
                        i64::from(g.count) * HEAD_TAX_CENTS_PER_PERSON,
                    );
                    let need = economy::compute_apple_demand(g.count);
                    let eaten = invs[inv].consume_up_to(apple, need);
                    sec_cons[s] += eaten;
                    food_met[s][gi] = if need <= 0 {
                        1000
                    } else {
                        ((eaten * 1000) / need).clamp(0, 1000) as u16
                    };
                }
            }

            // ── 11. Spoilage on every warehouse ────────────────────────────────
            for inv in invs.iter_mut() {
                inv.apply_monthly_retention(goods);
            }

            // Restore workshop warehouses, real cash revenue, arrears flag, and cost basis.
            for (k, w) in ws_refs.iter().enumerate() {
                if let Some(ws) = pt.workshops[w.tile][w.slot.as_index()].as_mut() {
                    ws.inv = std::mem::take(&mut invs[w.inv]);
                    ws.last_output = ws_month[k].output;
                    ws.last_wage_paid_cents = ws_month[k].wage_paid;
                    ws.last_revenue_cents = ws_month[k].revenue;
                    ws.had_wage_arrears = ws_month[k].wage_paid < ws_month[k].wage_bill;
                    if ws_month[k].nominal_output > 0 {
                        ws.last_unit_cost_cents =
                            (ws_month[k].wage_bill / ws_month[k].nominal_output)
                                .clamp(MIN_ORDER_PRICE_CENTS, MAX_ORDER_PRICE_CENTS);
                    }
                }
            }
            eval_out = Some(eval);
            break;
        }
        let Some(eval) = eval_out else {
            return;
        };

        // ── Restore polity vault and fiscal statistics ───────────────────────
        for (_, pol) in world.query_mut::<&mut Polity>() {
            if pol.id == ruling {
                pol.vault = std::mem::take(&mut invs[POLITY_INV]);
                pol.last_tax_income_cents = tax_income;
                pol.last_state_enterprise_net_cents = state_net;
                pol.last_freight_revenue_cents = freight_rev;
                pol.last_seigniorage_cents = minted;
                pol.last_admin_upkeep_cents = admin_upkeep;
                pol.last_net_cashflow_cents = pol.vault.avail(cash) - vault_before;
                break;
            }
        }

        // ── Restore sectors, update Pops (satisfaction, natural growth, bankruptcy slide to `流民`) ─
        let default_pool_id = catalog.default_pool_id();
        let vag_pool_id = catalog.vagrant_pool_id();
        let has_vag = catalog.has_dedicated_vagrant_pool();
        let mut ref_by_sec: BTreeMap<SectorId, BTreeSet<PopId>> = BTreeMap::new();
        for (_, (planet, pt)) in world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == pid {
                for s in 0..sec_count {
                    let sid = SectorId(s as u16);
                    ref_by_sec.insert(sid, pt.referenced_pop_ids_in_sector(sid));
                }
                break;
            }
        }
        for (_, (sec, market, cap, pops)) in world.query_mut::<(
            &mut Sector,
            &mut SectorMarket,
            &mut SectorCapital,
            &mut SectorPops,
        )>() {
            let s = usize::from(sec.id.0);
            if sec.planet_id != pid || s >= sec_count {
                continue;
            }
            sec.state_depot = std::mem::take(&mut invs[slots[s].depot]);
            cap.vault = std::mem::take(&mut invs[slots[s].capital]);
            cap.last_dividends_cents = dividends_gross[s];
            cap.last_reinvested_cents = cap
                .vault
                .avail(cash)
                .min(capital_inflow[s] - capital_inflow[s] * DIVIDEND_PAYOUT_PERCENT / 100);
            market.apple_book = std::mem::replace(&mut books[s], SectorOrderBook::new(apple, 0));
            market.closing_depth = depth.get(s).cloned();
            market.last_production = sec_prod[s];
            market.last_subsistence_output = sec_sub[s];
            market.last_consumption = sec_cons[s];
            let ma = market.apple_book.ma_price_cents;

            for (gi, g) in pops.groups.iter_mut().enumerate() {
                g.inv = std::mem::take(&mut invs[slots[s].pops[gi]]);
                g.last_income_cents = pop_income[s][gi];
                let met = food_met[s][gi];
                g.last_food_met_permille = met;
                if g.count == 0 {
                    continue;
                }
                let g_def = catalog.get(g.pop_type);
                let is_vagrant = has_vag && g.pop_type == vag_pool_id;
                let is_between_jobs = g.employer == BuildingOwner::Unowned
                    && !g_def.is_default_pool
                    && !is_vagrant
                    && !g_def.receives_dividends;
                let food_bill = economy::compute_apple_demand(g.count) * ma;
                let mut target_sat: i16 = if is_vagrant {
                    if met >= 950 {
                        52
                    } else if met >= 700 {
                        38
                    } else {
                        22
                    }
                } else if is_between_jobs {
                    if met >= 950 {
                        70
                    } else if met >= 700 {
                        54
                    } else {
                        34
                    }
                } else if met >= 950 {
                    if pop_income[s][gi] > food_bill { 84 } else { 76 }
                } else if met >= 700 {
                    62
                } else {
                    40
                };
                // Wage-squeeze dissatisfaction if an employed Pop's contract wage was pushed < 85% baseline!
                let base_w = g_def.base_wage_per_100 * CASH_SCALE / 100;
                let (c_emp, c_bill) = eval.pop_contracted[s].get(gi).copied().unwrap_or((0, 0));
                if base_w > 0 && c_emp > 0 && c_bill * 100 < i64::from(c_emp) * base_w * 85 {
                    target_sat -= 6;
                }

                let cur = i16::from(g.satisfaction);
                let next = if cur < target_sat {
                    (cur + 2).min(target_sat)
                } else {
                    (cur - 3).max(target_sat)
                };
                g.satisfaction = next.clamp(10, 98) as u8;
            }

            let hab = sec_hab.get(&sec.id).copied().unwrap_or(0);
            let growth = pops.monthly_growth(hab, eval.sector_stats[s].has_core);
            pops.apply_natural_growth(goods, default_pool_id, growth);

            // Destitution / downward-mobility pass (`XX · 待业` -> `demote_to` -> `流民`):
            // ONLY when an `Unowned` between-jobs group has exhausted its savings (`avail(cash) < ma`)
            // AND is going hungry (`last_food_met_permille < 750`) does it slide down `demote_to`!
            let mut demotions: Vec<(PopTypeId, u16, PopTypeId, u32)> = Vec::new();
            for g in &pops.groups {
                if g.count == 0 || g.employer != BuildingOwner::Unowned {
                    continue;
                }
                let g_def = catalog.get(g.pop_type);
                if g_def.is_default_pool || !g_def.can_demote {
                    continue;
                }
                let is_vag = has_vag && g.pop_type == vag_pool_id;
                if is_vag {
                    // Long-term starving vagrants in a city with excess vagrants trickle back to rural land.
                    if g.last_food_met_permille < 700 && g.count > 30 {
                        let back = ((g.count - 30) / 10).clamp(1, 4);
                        demotions.push((g.pop_type, g.culture_id, default_pool_id, back));
                    }
                    continue;
                }
                if g.inv.avail(cash) < ma.max(CASH_SCALE) && g.last_food_met_permille < 750 {
                    let dst_pid = g_def
                        .demote_to
                        .as_deref()
                        .and_then(|id| catalog.id_of(id))
                        .unwrap_or(vag_pool_id);
                    if dst_pid != g.pop_type {
                        let step = (g.count / 4).max(1);
                        demotions.push((g.pop_type, g.culture_id, dst_pid, step));
                    }
                }
            }
            for (src_type, cid, dst_type, step) in demotions {
                if let Some(src_idx) = pops.find_index(src_type, cid, BuildingOwner::Unowned) {
                    let dst_idx =
                        pops.ensure_group_index(goods, dst_type, cid, BuildingOwner::Unowned);
                    let dst_before = pops.groups[dst_idx].count;
                    let moved = pops.transfer_between_groups(goods, src_idx, dst_idx, step);
                    if moved > 0 && has_vag && dst_type == vag_pool_id && dst_before == 0 {
                        pops.groups[dst_idx].satisfaction = 32;
                    }
                }
            }
            if let Some(ref_ids) = ref_by_sec.get(&sec.id) {
                pops.prune_empty_groups(goods, default_pool_id, ref_ids);
            }
        }
    }

    /// Total `base:cash` cents held by every inventory on a planet plus its ruling polity vault.
    pub fn planet_money_supply_cents(&self, planet_id: &str) -> i64 {
        self.planet_good_total(planet_id, self.goods.cash_id())
    }

    /// Total units of `good` held anywhere on a planet (workshops, depots, capital pools, Pops, polity vault).
    pub fn planet_good_total(&self, planet_id: &str, good: GoodId) -> i64 {
        let mut total = 0i64;
        let mut ruling = None;
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == planet_id {
                ruling = Some(planet.ruling_polity);
                for row in &pt.workshops {
                    for ws in row.iter().flatten() {
                        total += ws.inv.total(good);
                    }
                }
            }
        }
        for (_, (sec, cap, pops)) in self
            .world
            .query::<(&Sector, &SectorCapital, &SectorPops)>()
            .iter()
        {
            if sec.planet_id == planet_id {
                total += sec.state_depot.total(good) + cap.vault.total(good);
                total += pops.groups.iter().map(|g| g.inv.total(good)).sum::<i64>();
            }
        }
        for (_, pol) in self.world.query::<&Polity>().iter() {
            if Some(pol.id) == ruling {
                total += pol.vault.total(good);
            }
        }
        total
    }
}
