//! Autonomous Workplace Hiring, Downsizing, Renewal & Job-Seeker Quoting (`AI 招工与求职决策`).

use super::pricing::contract_redemption_priority;
use crate::building::{BuildingDef, BuildingTag, TileSlotKey};
use crate::economy::{
    BuildingOwner, EnterpriseId, Polity, PolityId, PopTypeId, Sector, SectorCapital, SectorId,
    SectorMarket, SectorPops, TrustId, ENTERPRISE_OUTPUT_BONUS_PERMILLE,
};
use crate::market::{
    clear_local_labor_book, compute_per_100_demand, GoodId, LaborAsk, LaborBid, RoleWageBook,
    CASH_SCALE, MIN_WAGE_CENTS,
};
use crate::planet::{CashSink, Planet, PlanetTiles};
use crate::Game;
use std::collections::BTreeMap;

impl Game {
    /// Phase I of monthly political-economy cycle: Autonomous Labor & Staffing Decisions.
    pub(crate) fn run_monthly_labor_decisions(&mut self, pid: &str, ruling: PolityId) {
        let goods = &self.goods;
        let buildings = &self.buildings;
        let catalog = &self.pop_catalog;
        let cash = goods.cash_id();
        let c_idx = usize::from(cash.0);
        let staples = goods.staple_goods();
        let default_pool = catalog.default_pool_id();
        let vag_pool = catalog.vagrant_pool_id();
        let type_count = catalog.len();
        let month_seq = self.counters.monthly_ticks as u32;

        let mut polity_vault_cash: BTreeMap<PolityId, i64> = BTreeMap::new();
        for (_, pol) in self.world.query::<&Polity>().iter() {
            polity_vault_cash.insert(pol.id, pol.cash_cents(goods));
        }
        let treasury_total_cash = polity_vault_cash.get(&ruling).copied().unwrap_or(0);

        let mut state_bill_by_tier = [0i64; 4];
        let mut polity_core_bills: BTreeMap<PolityId, i64> = BTreeMap::new();
        {
            let mut sec_pops_map: BTreeMap<u16, &SectorPops> = BTreeMap::new();
            let mut q = self.world.query::<(&Sector, &SectorPops)>();
            for (_, (sec, pops)) in q.iter() {
                if sec.planet_id == pid {
                    sec_pops_map.insert(sec.id.0, pops);
                }
            }
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != pid {
                    continue;
                }
                for tile in &pt.tiles {
                    let Some(spops) = sec_pops_map.get(&tile.sector_id) else {
                        continue;
                    };
                    let t_idx = usize::from(tile.index);
                    let is_cap = tile
                        .slots
                        .iter_occupied()
                        .any(|(_, btid)| buildings.get(btid).tags.has(BuildingTag::PlanetaryCapital));
                    for (sk, tid) in tile.slots.iter_occupied() {
                        let def = buildings.get(tid);
                        if pt.workshops[t_idx][sk.as_index()].is_none()
                            && def.is_state_core_or_public()
                        {
                            let core_pol = match pt.slot_owner(t_idx, sk) {
                                BuildingOwner::State { polity_id } => polity_id,
                                _ => ruling,
                            };
                            for c in &pt.slot_contracts[t_idx][sk.as_index()] {
                                if let Some(gi) = spops.index_by_id(c.pop_id) {
                                    let r_id = spops.groups[gi].pop_type;
                                    let (tier, _) =
                                        contract_redemption_priority(catalog, def, r_id, is_cap);
                                    if core_pol == ruling {
                                        state_bill_by_tier[usize::from(tier.min(3))] +=
                                            c.monthly_bill_cents;
                                    }
                                    *polity_core_bills.entry(core_pol).or_insert(0) +=
                                        c.monthly_bill_cents;
                                }
                            }
                        }
                    }
                }
                break;
            }
        }
        let mut treasury_budget = treasury_total_cash - state_bill_by_tier.iter().sum::<i64>();
        let mut polity_topup_spent: BTreeMap<PolityId, i64> = BTreeMap::new();

        let mut cap_vault_cash: BTreeMap<u16, i64> = BTreeMap::new();
        for (_, (sec, cap)) in self.world.query::<(&Sector, &SectorCapital)>().iter() {
            if sec.planet_id == pid {
                cap_vault_cash.insert(sec.id.0, cap.pool_cents(goods));
            }
        }
        let mut cap_budgets = cap_vault_cash.clone();
        let mut cap_severance_spent: BTreeMap<u16, i64> = BTreeMap::new();

        let mut ent_vault_cash: BTreeMap<u16, i64> = BTreeMap::new();
        let mut trust_vault_cash: BTreeMap<u16, i64> = BTreeMap::new();
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == pid {
                for ent in &pt.enterprises {
                    ent_vault_cash.insert(ent.id.0, ent.cash_cents(goods));
                }
                for tr in &pt.trusts {
                    trust_vault_cash.insert(tr.id.0, tr.cash_cents(goods));
                }
                break;
            }
        }
        let mut ent_budgets = ent_vault_cash.clone();
        let mut ent_severance_spent: BTreeMap<u16, i64> = BTreeMap::new();
        let mut ent_topup_spent: BTreeMap<u16, i64> = BTreeMap::new();
        let mut trust_budgets = trust_vault_cash.clone();
        let mut trust_severance_spent: BTreeMap<u16, i64> = BTreeMap::new();
        let mut trust_topup_spent: BTreeMap<u16, i64> = BTreeMap::new();

        let default_is_sub = catalog.get(default_pool).is_self_subsistence();
        let mut role_order: Vec<PopTypeId> = (0..type_count)
            .map(|i| PopTypeId(i as u16))
            .filter(|&r| r != vag_pool && !(default_is_sub && r == default_pool))
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

            let mut pops_opt: Option<SectorPops> = None;
            let mut wage_books: Vec<RoleWageBook> = Vec::new();
            let mut good_ma: BTreeMap<GoodId, i64> = BTreeMap::new();
            let mut good_unsold_asks: BTreeMap<GoodId, i64> = BTreeMap::new();
            for (_, (sec, mkt, pops)) in self
                .world
                .query_mut::<(&Sector, &mut SectorMarket, &mut SectorPops)>()
            {
                if sec.planet_id == pid && sec.id == sec_id {
                    mkt.ensure_good_books(goods);
                    for gid in goods.tradable_good_ids() {
                        let px = mkt
                            .book(gid)
                            .map(|b| b.ma_price_cents)
                            .unwrap_or(goods.get(gid).genesis_price_cents);
                        good_ma.insert(gid, px);
                        let asks_q: i64 = mkt
                            .closing_depth(gid)
                            .map(|ob| ob.asks.iter().map(|l| l.qty).sum())
                            .unwrap_or(0);
                        good_unsold_asks.insert(gid, asks_q);
                    }
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

            let staple_floor_per_worker_cents: i64 = staples
                .iter()
                .map(|&(sgid, per_100)| {
                    let px = good_ma
                        .get(&sgid)
                        .copied()
                        .unwrap_or(goods.get(sgid).genesis_price_cents);
                    (px * per_100 + 99) / 100
                })
                .sum();

            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id != pid {
                    continue;
                }

                let mut active_slots: Vec<(usize, TileSlotKey, &BuildingDef, u16)> = Vec::new();
                for tile in &pt.tiles {
                    if tile.sector_id != sec_u16 {
                        continue;
                    }
                    let t_idx = usize::from(tile.index);
                    let hab = tile.tags.habitability_permille();
                    for (sk, tid) in tile.slots.iter_occupied() {
                        active_slots.push((t_idx, sk, buildings.get(tid), hab));
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

                let mut ent_active = vec![false; pt.enterprises.len()];
                for (e_i, ent) in pt.enterprises.iter().enumerate() {
                    if let Some(ctile) = pt.tiles.get(ent.campus_tile_index) {
                        if let Some(ctid) = ctile.slots.primary {
                            let cdef = buildings.get(ctid);
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

                // ── 1A. Entity-Directed vs `LocalPrivate` Downsizing & Working-Capital Pre-Topup ──
                for &(t_idx, sk, def, _) in &active_slots {
                    let owner = pt.slot_owner(t_idx, sk);
                    let is_local_private = matches!(owner, BuildingOwner::LocalPrivate { .. });
                    let target_jobs = pt.effective_slot_target_jobs(t_idx, sk, def);
                    let honors_wages = pt.slot_honors_wages(t_idx, sk);
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

                    if let Some(ws) = pt.workshops[t_idx][sk.as_index()].as_mut() {
                        ws.inv.ensure_len(goods.len());
                        let contracts = &mut pt.slot_contracts[t_idx][sk.as_index()];
                        let cur_emp: u32 = contracts.iter().map(|c| c.count).sum();
                        let locked_bill: i64 = contracts.iter().map(|c| c.monthly_bill_cents).sum();

                        // Named entities (`State`, `Enterprise`, `Trust`) pre-fund working capital into
                        // active workshops that lack 1-month target wage cover (e.g. newly built or restarting SOEs).
                        if !is_local_private && honors_wages && target_jobs > 0 {
                            let full_req = def.slot_jobs_required(sk).max(1);
                            let scale = i64::from(def.scale_for_slot(sk));
                            let mut target_wage_bill = locked_bill;
                            if cur_emp < target_jobs {
                                let mut full_base_bill = 0i64;
                                for (role_str, base_req) in &def.jobs_by_role {
                                    let rid = def.resolve_role_id(catalog, role_str);
                                    let rw = wage_books
                                        .get(usize::from(rid.0))
                                        .map(|wb| wb.ma_wage_cents)
                                        .unwrap_or_else(|| {
                                            catalog.get(rid).base_wage_per_100 * CASH_SCALE / 100
                                        });
                                    full_base_bill += i64::from(*base_req) * scale * rw;
                                }
                                let est_target = (full_base_bill * i64::from(target_jobs))
                                    / i64::from(full_req);
                                target_wage_bill = target_wage_bill.max(est_target);
                            }
                            let ws_now = ws.inv.avail(cash);
                            let unpaid_now = ws.total_unpaid_claims_cents();
                            if ws_now < target_wage_bill + unpaid_now {
                                let need = (target_wage_bill + unpaid_now) - ws_now;
                                let grant = match owner {
                                    BuildingOwner::State { polity_id } => {
                                        let v_tot = polity_vault_cash
                                            .get(&polity_id)
                                            .copied()
                                            .unwrap_or(0);
                                        let c_keep = polity_core_bills
                                            .get(&polity_id)
                                            .copied()
                                            .unwrap_or(0);
                                        let spent = polity_topup_spent
                                            .get(&polity_id)
                                            .copied()
                                            .unwrap_or(0);
                                        let g = (v_tot - c_keep - spent).max(0).min(need);
                                        if g > 0 {
                                            *polity_topup_spent.entry(polity_id).or_insert(0) += g;
                                            if polity_id == ruling {
                                                treasury_budget -= g;
                                            }
                                        }
                                        g
                                    }
                                    BuildingOwner::Enterprise { enterprise_id } => {
                                        let v_tot = ent_vault_cash
                                            .get(&enterprise_id.0)
                                            .copied()
                                            .unwrap_or(0);
                                        let spent = ent_severance_spent
                                            .get(&enterprise_id.0)
                                            .copied()
                                            .unwrap_or(0)
                                            + ent_topup_spent
                                                .get(&enterprise_id.0)
                                                .copied()
                                                .unwrap_or(0);
                                        let g = (v_tot - spent).max(0).min(need);
                                        if g > 0 {
                                            *ent_topup_spent.entry(enterprise_id.0).or_insert(0) +=
                                                g;
                                            if let Some(b) = ent_budgets.get_mut(&enterprise_id.0) {
                                                *b -= g;
                                            }
                                        }
                                        g
                                    }
                                    BuildingOwner::Trust { trust_id } => {
                                        let v_tot = trust_vault_cash
                                            .get(&trust_id.0)
                                            .copied()
                                            .unwrap_or(0);
                                        let spent = trust_severance_spent
                                            .get(&trust_id.0)
                                            .copied()
                                            .unwrap_or(0)
                                            + trust_topup_spent
                                                .get(&trust_id.0)
                                                .copied()
                                                .unwrap_or(0);
                                        let g = (v_tot - spent).max(0).min(need);
                                        if g > 0 {
                                            *trust_topup_spent.entry(trust_id.0).or_insert(0) += g;
                                            if let Some(b) = trust_budgets.get_mut(&trust_id.0) {
                                                *b -= g;
                                            }
                                        }
                                        g
                                    }
                                    _ => 0,
                                };
                                if grant > 0 {
                                    ws.inv.available[c_idx] += grant;
                                    if ws.inv.avail(cash) >= locked_bill + unpaid_now {
                                        ws.had_wage_arrears = false;
                                    }
                                }
                            }
                        }

                        if cur_emp == 0 {
                            continue;
                        }

                        let free_vol = ws.inv.free_volume(goods);
                        let est_net_val = def.slot_expected_net_value_cents(
                            goods, &good_ma, sk, cur_emp, bonus, free_vol,
                        );
                        let ws_cash = ws.inv.avail(cash);
                        let unpaid_claims = ws.total_unpaid_claims_cents();
                        let expected_inflow = if ws.last_revenue_cents < 0 {
                            est_net_val.max(0)
                        } else if ws.had_wage_arrears {
                            ws.last_revenue_cents.max(0)
                        } else {
                            ws.last_revenue_cents.max(0).max(est_net_val.max(0) / 2)
                        };

                        let in_unresolved_default = (ws.had_wage_arrears || unpaid_claims > 0)
                            && (!honors_wages
                                || ws_cash + expected_inflow / 2 < locked_bill + unpaid_claims);

                        // Corporate entity (`Enterprise` / `Trust`) downsizing: only when the parent entity
                        // vault itself is depleted (`entity_cash_exhausted`) and the plant is losing money!
                        let corp_entity_cut = match owner {
                            BuildingOwner::Enterprise { enterprise_id } => {
                                let rem = ent_vault_cash
                                    .get(&enterprise_id.0)
                                    .copied()
                                    .unwrap_or(0)
                                    - ent_severance_spent
                                        .get(&enterprise_id.0)
                                        .copied()
                                        .unwrap_or(0)
                                    - ent_topup_spent
                                        .get(&enterprise_id.0)
                                        .copied()
                                        .unwrap_or(0);
                                rem <= locked_bill && est_net_val < (locked_bill * 70) / 100
                            }
                            BuildingOwner::Trust { trust_id } => {
                                let rem = trust_vault_cash.get(&trust_id.0).copied().unwrap_or(0)
                                    - trust_severance_spent
                                        .get(&trust_id.0)
                                        .copied()
                                        .unwrap_or(0)
                                    - trust_topup_spent.get(&trust_id.0).copied().unwrap_or(0);
                                rem <= locked_bill && est_net_val < (locked_bill * 70) / 100
                            }
                            _ => false,
                        };

                        if in_unresolved_default {
                            let sustainable_bill = if !honors_wages {
                                0
                            } else {
                                (ws_cash - unpaid_claims)
                                    .max((expected_inflow * 85) / 100 - unpaid_claims)
                                    .max(0)
                            };
                            for c in contracts.iter_mut() {
                                if c.count == 0 {
                                    continue;
                                }
                                let keep = if locked_bill <= 0 {
                                    c.count.min(target_jobs)
                                } else {
                                    (((u128::from(c.count) * u128::from(sustainable_bill as u64))
                                        / u128::from(locked_bill as u64))
                                        as u32)
                                        .min(target_jobs)
                                };
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
                                    pops.release_to_between_jobs(goods, c.pop_id, cut, 8);
                                }
                            }
                            contracts.retain(|c| c.count > 0);
                        } else if (is_local_private || corp_entity_cut)
                            && locked_bill > 0
                            && ws_cash + expected_inflow / 2 < locked_bill
                        {
                            let sustainable_bill =
                                ws_cash.max((expected_inflow * 85) / 100).max(0);
                            let floor_heads = (cur_emp / 2).min(target_jobs);
                            let mut rem_keep_floor = floor_heads;
                            for c in contracts.iter_mut() {
                                if c.count == 0 || c.monthly_bill_cents <= 0 {
                                    continue;
                                }
                                let raw_keep = ((u128::from(c.count)
                                    * u128::from(sustainable_bill as u64))
                                    / u128::from(locked_bill as u64))
                                    as u32;
                                let min_this = rem_keep_floor.min(c.count);
                                let keep = raw_keep.max(min_this).min(c.count);
                                rem_keep_floor = rem_keep_floor.saturating_sub(keep);
                                let desired_cut = c.count.saturating_sub(keep);
                                if desired_cut == 0 {
                                    continue;
                                }
                                let unit_wage = (c.monthly_bill_cents / i64::from(c.count)).max(1);
                                let max_afford_sev =
                                    (ws.inv.available[c_idx] / unit_wage).max(0) as u32;
                                let cut = desired_cut.min(max_afford_sev);
                                if cut > 0 {
                                    let cut_bill = if cut == c.count {
                                        c.monthly_bill_cents
                                    } else {
                                        (c.monthly_bill_cents * i64::from(cut)) / i64::from(c.count)
                                    };
                                    let paid_sev = cut_bill.min(ws.inv.available[c_idx]).max(0);
                                    if let Some(gi) = pops.index_by_id(c.pop_id) {
                                        pops.groups[gi].inv.ensure_len(goods.len());
                                        ws.inv.available[c_idx] -= paid_sev;
                                        pops.groups[gi].inv.available[c_idx] += paid_sev;
                                        let r_i = usize::from(pops.groups[gi].pop_type.0);
                                        if let Some(wb) = wage_books.get_mut(r_i) {
                                            wb.last_laid_off += cut;
                                        }
                                    }
                                    c.count -= cut;
                                    c.monthly_bill_cents -= cut_bill;
                                    pops.release_to_between_jobs(goods, c.pop_id, cut, 3);
                                }
                            }
                            contracts.retain(|c| c.count > 0);
                        }
                    } else if !def.is_state_core_or_public() {
                        let (avail_pool, vault_total, prev_sev) = match owner {
                            BuildingOwner::Enterprise { enterprise_id } => (
                                ent_budgets.get(&enterprise_id.0).copied().unwrap_or(0),
                                ent_vault_cash.get(&enterprise_id.0).copied().unwrap_or(0),
                                ent_severance_spent.get(&enterprise_id.0).copied().unwrap_or(0),
                            ),
                            BuildingOwner::Trust { trust_id } => (
                                trust_budgets.get(&trust_id.0).copied().unwrap_or(0),
                                trust_vault_cash.get(&trust_id.0).copied().unwrap_or(0),
                                trust_severance_spent.get(&trust_id.0).copied().unwrap_or(0),
                            ),
                            BuildingOwner::LocalPrivate { home_sector } => (
                                cap_budgets.get(&home_sector.0).copied().unwrap_or(0),
                                cap_vault_cash.get(&home_sector.0).copied().unwrap_or(0),
                                cap_severance_spent.get(&home_sector.0).copied().unwrap_or(0),
                            ),
                            _ => (
                                cap_budgets.get(&sec_u16).copied().unwrap_or(0),
                                cap_vault_cash.get(&sec_u16).copied().unwrap_or(0),
                                cap_severance_spent.get(&sec_u16).copied().unwrap_or(0),
                            ),
                        };
                        let contracts = &mut pt.slot_contracts[t_idx][sk.as_index()];
                        let locked_bill: i64 = contracts.iter().map(|c| c.monthly_bill_cents).sum();
                        if !honors_wages || locked_bill > avail_pool {
                            let sustainable = if !honors_wages { 0 } else { avail_pool.max(0) };
                            let mut pool_left = vault_total
                                .saturating_sub(prev_sev)
                                .saturating_sub(sustainable);
                            for c in contracts.iter_mut() {
                                if c.count == 0 || c.monthly_bill_cents <= 0 {
                                    continue;
                                }
                                let keep = if locked_bill <= 0 {
                                    0
                                } else {
                                    (((u128::from(c.count) * u128::from(sustainable as u64))
                                        / u128::from(locked_bill as u64))
                                        as u32)
                                        .min(target_jobs)
                                };
                                let cut = c.count.saturating_sub(keep);
                                if cut > 0 {
                                    let cut_bill = if cut == c.count {
                                        c.monthly_bill_cents
                                    } else {
                                        (c.monthly_bill_cents * i64::from(cut)) / i64::from(c.count)
                                    };
                                    let sev = cut_bill.min(pool_left.max(0));
                                    if let Some(gi) = pops.index_by_id(c.pop_id) {
                                        if sev > 0 {
                                            pops.groups[gi].inv.ensure_len(goods.len());
                                            pops.groups[gi].inv.available[c_idx] += sev;
                                            pool_left -= sev;
                                            match owner {
                                                BuildingOwner::Enterprise { enterprise_id } => {
                                                    *ent_severance_spent
                                                        .entry(enterprise_id.0)
                                                        .or_insert(0) += sev;
                                                }
                                                BuildingOwner::Trust { trust_id } => {
                                                    *trust_severance_spent
                                                        .entry(trust_id.0)
                                                        .or_insert(0) += sev;
                                                }
                                                BuildingOwner::LocalPrivate { home_sector } => {
                                                    *cap_severance_spent
                                                        .entry(home_sector.0)
                                                        .or_insert(0) += sev;
                                                }
                                                _ => {
                                                    *cap_severance_spent
                                                        .entry(sec_u16)
                                                        .or_insert(0) += sev;
                                                }
                                            }
                                        }
                                        let r_i = usize::from(pops.groups[gi].pop_type.0);
                                        if let Some(wb) = wage_books.get_mut(r_i) {
                                            wb.last_laid_off += cut;
                                        }
                                    }
                                    c.count -= cut;
                                    c.monthly_bill_cents -= cut_bill;
                                    let pen = if sev >= cut_bill { 3 } else { 6 };
                                    pops.release_to_between_jobs(goods, c.pop_id, cut, pen);
                                }
                            }
                            contracts.retain(|c| c.count > 0);
                        }
                        let rem_bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .map(|c| c.monthly_bill_cents)
                            .sum();
                        match owner {
                            BuildingOwner::Enterprise { enterprise_id } => {
                                if let Some(b) = ent_budgets.get_mut(&enterprise_id.0) {
                                    *b -= rem_bill;
                                }
                            }
                            BuildingOwner::Trust { trust_id } => {
                                if let Some(b) = trust_budgets.get_mut(&trust_id.0) {
                                    *b -= rem_bill;
                                }
                            }
                            BuildingOwner::LocalPrivate { home_sector } => {
                                if let Some(b) = cap_budgets.get_mut(&home_sector.0) {
                                    *b -= rem_bill;
                                }
                            }
                            _ => {
                                if let Some(b) = cap_budgets.get_mut(&sec_u16) {
                                    *b -= rem_bill;
                                }
                            }
                        }
                    }
                }

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

                // ── 1B. 24-Month Rolling Maturity Bilateral Renegotiation (`Pop` <-> Employer) ──
                for &(t_idx, sk, def, _) in &active_slots {
                    let is_cap_tile = pt.tiles[t_idx]
                        .slots
                        .iter_occupied()
                        .any(|(_, btid)| buildings.get(btid).tags.has(BuildingTag::PlanetaryCapital));
                    let slot_salt =
                        (t_idx as u32).wrapping_mul(7).wrapping_add(sk.as_index() as u32 * 5);
                    let phase = month_seq.wrapping_add(slot_salt) % 24;
                    let owner = pt.slot_owner(t_idx, sk);
                    let target_jobs = pt.effective_slot_target_jobs(t_idx, sk, def);
                    let honors_wages = pt.slot_honors_wages(t_idx, sk);
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
                    let mut slot_emp_now: u32 = pt.slot_contracts[t_idx][sk.as_index()]
                        .iter()
                        .map(|c| c.count)
                        .sum();
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

                        let over_target = slot_emp_now.saturating_sub(target_jobs);
                        let drop_for_target = expiring.min(over_target);
                        let renew_candidates = expiring - drop_for_target;

                        let ma_w = wage_books
                            .get(r_i)
                            .map(|wb| wb.ma_wage_cents)
                            .unwrap_or(0);
                        let cand_old_bill = if expiring > 0 {
                            (exp_bill * i64::from(renew_candidates)) / i64::from(expiring)
                        } else {
                            0
                        };
                        let drop_old_bill = exp_bill - cand_old_bill;
                        let new_cand_bill = i64::from(renew_candidates) * ma_w;

                        let employer_agrees = renew_candidates > 0
                            && honors_wages
                            && if let Some(ws) = pt.workshops[t_idx][sk.as_index()].as_ref() {
                                let is_local_private =
                                    matches!(owner, BuildingOwner::LocalPrivate { .. });
                                let inv_fill_permille =
                                    (ws.inv.used_volume(goods) * 1000) / ws.inv.capacity.max(1);
                                let req = def.slot_jobs_required(sk).max(1);
                                let net_val = def.slot_expected_net_value_cents(
                                    goods,
                                    &good_ma,
                                    sk,
                                    req,
                                    bonus,
                                    i64::MAX / 8,
                                );
                                let mrp_cents = net_val / i64::from(req);
                                let bill_after = slot_bill_now - exp_bill + new_cand_bill;
                                if is_local_private {
                                    !ws.had_wage_arrears
                                        && ws.total_unpaid_claims_cents() == 0
                                        && inv_fill_permille < 880
                                        && mrp_cents >= (ma_w * 85) / 100
                                        && (ws.inv.avail(cash) >= bill_after
                                            || ws.last_revenue_cents < 0)
                                } else {
                                    !ws.had_wage_arrears
                                        && ws.total_unpaid_claims_cents() == 0
                                        && (ws.inv.avail(cash) >= bill_after
                                            || ws.last_revenue_cents < 0)
                                }
                            } else if def.is_state_core_or_public() {
                                let (tier, _) =
                                    contract_redemption_priority(catalog, def, role_id, is_cap_tile);
                                let t_u = usize::from(tier.min(3));
                                let committed_up_to_tier: i64 =
                                    state_bill_by_tier[..=t_u].iter().sum();
                                treasury_total_cash - committed_up_to_tier + exp_bill
                                    >= new_cand_bill
                            } else {
                                let avail = match owner {
                                    BuildingOwner::Enterprise { enterprise_id } => {
                                        ent_budgets.get(&enterprise_id.0).copied().unwrap_or(0)
                                    }
                                    BuildingOwner::Trust { trust_id } => {
                                        trust_budgets.get(&trust_id.0).copied().unwrap_or(0)
                                    }
                                    BuildingOwner::LocalPrivate { home_sector } => {
                                        cap_budgets.get(&home_sector.0).copied().unwrap_or(0)
                                    }
                                    _ => cap_budgets.get(&sec_u16).copied().unwrap_or(0),
                                };
                                avail + exp_bill >= new_cand_bill
                            };

                        let tier_idx_opt = if pt.workshops[t_idx][sk.as_index()].is_none()
                            && def.is_state_core_or_public()
                        {
                            let (tier, _) =
                                contract_redemption_priority(catalog, def, role_id, is_cap_tile);
                            Some(usize::from(tier.min(3)))
                        } else {
                            None
                        };

                        let adjust_non_state_budget =
                            |owner: BuildingOwner,
                             delta: i64,
                             ent_b: &mut BTreeMap<u16, i64>,
                             tr_b: &mut BTreeMap<u16, i64>,
                             cap_b: &mut BTreeMap<u16, i64>| {
                                match owner {
                                    BuildingOwner::Enterprise { enterprise_id } => {
                                        if let Some(b) = ent_b.get_mut(&enterprise_id.0) {
                                            *b += delta;
                                        }
                                    }
                                    BuildingOwner::Trust { trust_id } => {
                                        if let Some(b) = tr_b.get_mut(&trust_id.0) {
                                            *b += delta;
                                        }
                                    }
                                    BuildingOwner::LocalPrivate { home_sector } => {
                                        if let Some(b) = cap_b.get_mut(&home_sector.0) {
                                            *b += delta;
                                        }
                                    }
                                    _ => {
                                        if let Some(b) = cap_b.get_mut(&sec_u16) {
                                            *b += delta;
                                        }
                                    }
                                }
                            };

                        if drop_for_target > 0 {
                            c.count -= drop_for_target;
                            c.monthly_bill_cents -= drop_old_bill;
                            slot_emp_now -= drop_for_target;
                            slot_bill_now -= drop_old_bill;
                            if pt.workshops[t_idx][sk.as_index()].is_none() {
                                if let Some(ti) = tier_idx_opt {
                                    state_bill_by_tier[ti] -= drop_old_bill;
                                    treasury_budget += drop_old_bill;
                                } else {
                                    adjust_non_state_budget(
                                        owner,
                                        drop_old_bill,
                                        &mut ent_budgets,
                                        &mut trust_budgets,
                                        &mut cap_budgets,
                                    );
                                }
                            }
                            pops.release_to_between_jobs(goods, c.pop_id, drop_for_target, 1);
                        }

                        if renew_candidates == 0 {
                            continue;
                        }

                        if employer_agrees {
                            c.monthly_bill_cents =
                                c.monthly_bill_cents - cand_old_bill + new_cand_bill;
                            slot_bill_now = slot_bill_now - cand_old_bill + new_cand_bill;
                            if pt.workshops[t_idx][sk.as_index()].is_none() {
                                if let Some(ti) = tier_idx_opt {
                                    state_bill_by_tier[ti] =
                                        state_bill_by_tier[ti] - cand_old_bill + new_cand_bill;
                                    treasury_budget =
                                        treasury_budget + cand_old_bill - new_cand_bill;
                                } else {
                                    adjust_non_state_budget(
                                        owner,
                                        cand_old_bill - new_cand_bill,
                                        &mut ent_budgets,
                                        &mut trust_budgets,
                                        &mut cap_budgets,
                                    );
                                }
                            }
                        } else {
                            c.count -= renew_candidates;
                            c.monthly_bill_cents -= cand_old_bill;
                            slot_emp_now -= renew_candidates;
                            slot_bill_now -= cand_old_bill;
                            if pt.workshops[t_idx][sk.as_index()].is_none() {
                                if let Some(ti) = tier_idx_opt {
                                    state_bill_by_tier[ti] -= cand_old_bill;
                                    treasury_budget += cand_old_bill;
                                } else {
                                    adjust_non_state_budget(
                                        owner,
                                        cand_old_bill,
                                        &mut ent_budgets,
                                        &mut trust_budgets,
                                        &mut cap_budgets,
                                    );
                                }
                            }
                            pops.release_to_between_jobs(goods, c.pop_id, renew_candidates, 2);
                        }
                    }
                    pt.slot_contracts[t_idx][sk.as_index()].retain(|c| c.count > 0);
                }

                // ── 1C. Local Labor Order Book Quoting & Neutral Clearing ──────────────
                for &role_id in &role_order {
                    let r_i = usize::from(role_id.0);
                    let r_def = catalog.get(role_id);
                    let ma_w = wage_books[r_i].ma_wage_cents;

                    let mut bids: Vec<LaborBid> = Vec::new();
                    for (slot_ref, &(t_idx, sk, def, _)) in active_slots.iter().enumerate() {
                        if !pt.slot_honors_wages(t_idx, sk) {
                            continue;
                        }
                        let full_slot_req = def.slot_jobs_required(sk);
                        let target_slot_jobs = pt.effective_slot_target_jobs(t_idx, sk, def);
                        if full_slot_req == 0 || target_slot_jobs == 0 {
                            continue;
                        }
                        let scale = def.scale_for_slot(sk);
                        let mut full_role_req = 0u32;
                        for (role_str, base_req) in &def.jobs_by_role {
                            if def.resolve_role_id(catalog, role_str) == role_id {
                                full_role_req += *base_req * scale;
                            }
                        }
                        if full_role_req == 0 {
                            continue;
                        }
                        let req = if target_slot_jobs >= full_slot_req {
                            full_role_req
                        } else {
                            ((u64::from(full_role_req) * u64::from(target_slot_jobs))
                                / u64::from(full_slot_req))
                                as u32
                        };
                        if req == 0 {
                            continue;
                        }
                        let cur_total_emp: u32 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .map(|c| c.count)
                            .sum();
                        if cur_total_emp >= target_slot_jobs {
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
                        let raw_vac = (req - cur_role_emp).min(target_slot_jobs - cur_total_emp);
                        let owner = pt.slot_owner(t_idx, sk);

                        if let Some(ws) = pt.workshops[t_idx][sk.as_index()].as_ref() {
                            let is_local_private =
                                matches!(owner, BuildingOwner::LocalPrivate { .. });
                            let is_state_ws = matches!(owner, BuildingOwner::State { .. });
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
                            let net_val = def.slot_expected_net_value_cents(
                                goods,
                                &good_ma,
                                sk,
                                full_slot_req,
                                bonus,
                                i64::MAX / 8,
                            );
                            let mrp_cents = net_val / i64::from(full_slot_req.max(1));
                            let cur_bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                                .iter()
                                .map(|c| c.monthly_bill_cents)
                                .sum();
                            let free_cash =
                                (ws.inv.avail(cash) - cur_bill - ws.total_unpaid_claims_cents())
                                    .max(0);
                            let inv_fill_permille =
                                (ws.inv.used_volume(goods) * 1000) / ws.inv.capacity.max(1);

                            let demand_saturated = is_local_private
                                && cur_role_emp > 0
                                && ws.last_revenue_cents >= 0
                                && def.monthly_output.iter().any(|(out_str, _)| {
                                    goods.id_of(out_str).map_or(false, |ogid| {
                                        let unsold =
                                            good_unsold_asks.get(&ogid).copied().unwrap_or(0);
                                        let out_ma = good_ma.get(&ogid).copied().unwrap_or(100);
                                        if goods.get(ogid).retention_permille == 0 {
                                            unsold > 0
                                                && ws.last_output > 0
                                                && ws.last_revenue_cents
                                                    < (ws.last_output * out_ma * 80) / 100
                                        } else {
                                            inv_fill_permille >= 450
                                                && ws.last_output > 0
                                                && ws.last_revenue_cents
                                                    < (ws.last_output * out_ma * 70) / 100
                                        }
                                    })
                                });

                            if ws.had_wage_arrears
                                || ws.total_unpaid_claims_cents() > 0
                                || (is_local_private && inv_fill_permille >= 900)
                                || demand_saturated
                                || (is_local_private
                                    && ma_w > 0
                                    && mrp_cents < (ma_w * 80) / 100
                                    && cur_role_emp > 0)
                            {
                                continue;
                            }
                            let bid_wage = if ma_w <= 0 {
                                0
                            } else if is_state_ws {
                                if cur_role_emp * 10 < req * 8 {
                                    (ma_w * 106 + 50) / 100
                                } else {
                                    ma_w
                                }
                            } else if is_local_private && mrp_cents < ma_w {
                                mrp_cents.max(MIN_WAGE_CENTS)
                            } else if cur_role_emp * 10 < req * 8 && mrp_cents >= (ma_w * 125) / 100
                            {
                                ((ma_w * 108 + 50) / 100)
                                    .min((mrp_cents * 85) / 100)
                                    .max(ma_w)
                            } else {
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
                            let pool_cash = match owner {
                                BuildingOwner::Enterprise { enterprise_id } => {
                                    ent_budgets.get(&enterprise_id.0).copied().unwrap_or(0)
                                }
                                BuildingOwner::Trust { trust_id } => {
                                    trust_budgets.get(&trust_id.0).copied().unwrap_or(0)
                                }
                                BuildingOwner::LocalPrivate { home_sector } => {
                                    cap_budgets.get(&home_sector.0).copied().unwrap_or(0)
                                }
                                _ => cap_budgets.get(&sec_u16).copied().unwrap_or(0),
                            };
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

                    let mut iter_sources: Vec<PopTypeId> = vec![role_id];
                    for sid in &r_def.promote_from {
                        if let Some(pid_src) = catalog.id_of(sid) {
                            if !iter_sources.contains(&pid_src) {
                                iter_sources.push(pid_src);
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
                                let monthly_food_bill: i64 = staples
                                    .iter()
                                    .map(|&(sgid, per_100)| {
                                        compute_per_100_demand(g.count, per_100)
                                            * good_ma
                                                .get(&sgid)
                                                .copied()
                                                .unwrap_or(goods.get(sgid).genesis_price_cents)
                                    })
                                    .sum();
                                let sav = g.inv.avail(cash);
                                let met = g.last_food_met_permille;
                                if met >= 900 && sav >= monthly_food_bill * 2 {
                                    ma_w.max(staple_floor_per_worker_cents)
                                } else if met >= 750 && sav >= monthly_food_bill / 2 {
                                    (ma_w * 92) / 100
                                } else {
                                    (ma_w * 82) / 100
                                }
                            } else if is_vagrant_src {
                                let sav = g.inv.avail(cash);
                                if g.last_food_met_permille >= 850 && sav > 0 {
                                    (ma_w * 90) / 100
                                } else {
                                    (ma_w * 78) / 100
                                }
                            } else {
                                ma_w.max((genesis_role_wage * 95) / 100)
                                    .max((staple_floor_per_worker_cents * 110) / 100)
                            };
                            asks.push(LaborAsk {
                                seeker_group_idx: gi,
                                ask_wage_cents: ask_wage,
                                available_heads: offer_heads,
                            });
                        }
                    }

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
                                    match owner {
                                        BuildingOwner::Enterprise { enterprise_id } => {
                                            if let Some(b) = ent_budgets.get_mut(&enterprise_id.0) {
                                                *b -= add_bill;
                                            }
                                        }
                                        BuildingOwner::Trust { trust_id } => {
                                            if let Some(b) = trust_budgets.get_mut(&trust_id.0) {
                                                *b -= add_bill;
                                            }
                                        }
                                        BuildingOwner::LocalPrivate { home_sector } => {
                                            if let Some(b) = cap_budgets.get_mut(&home_sector.0) {
                                                *b -= add_bill;
                                            }
                                        }
                                        _ => {
                                            if let Some(b) = cap_budgets.get_mut(&sec_u16) {
                                                *b -= add_bill;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

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

        for (pol_id, spent) in polity_topup_spent {
            if spent > 0 {
                self.adjust_cash(pid, CashSink::Polity(pol_id), -spent);
            }
        }
        for (sec_u16, spent) in cap_severance_spent {
            if spent > 0 {
                self.adjust_cash(pid, CashSink::Capital(SectorId(sec_u16)), -spent);
            }
        }
        for (ent_u16, spent) in ent_severance_spent {
            if spent > 0 {
                self.adjust_cash(pid, CashSink::Enterprise(EnterpriseId(ent_u16)), -spent);
            }
        }
        for (ent_u16, spent) in ent_topup_spent {
            if spent > 0 {
                self.adjust_cash(pid, CashSink::Enterprise(EnterpriseId(ent_u16)), -spent);
            }
        }
        for (tr_u16, spent) in trust_severance_spent {
            if spent > 0 {
                self.adjust_cash(pid, CashSink::Trust(TrustId(tr_u16)), -spent);
            }
        }
        for (tr_u16, spent) in trust_topup_spent {
            if spent > 0 {
                self.adjust_cash(pid, CashSink::Trust(TrustId(tr_u16)), -spent);
            }
        }
    }

}
