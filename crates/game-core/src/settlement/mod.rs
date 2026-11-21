//! Objective Physical & Clearing Settlement Layer (`客观物理与清算层`).
//!
//! Pure mechanism layer: zero hardcoded specific good IDs (`GOOD_APPLE`), zero hardcoded role IDs,
//! and zero hardcoded bilateral breach penalty formulas (contract & coupon obligations invoke
//! `EventCatalog` decision events evaluated via `if-else +-` AI weights or queued for player choice):
//! 1) invoke `decision::run_monthly_labor_decisions` (Phase I entity staffing & labor market)
//! 2) generic recipe production (`monthly_inputs` -> `monthly_output`, `produce_capped`)
//! 3) central bank minting per `MonetaryPolicy`
//! 4) Channel-3 bilateral contracts & standardized coupon claims via `EventCatalog` (`rules.rs`)
//! 5) multi-good spatial clearing (`GoodCatalog::tradable_good_ids()` -> `clear_spatial_good_market`)
//! 6) priority-ordered contract wage & senior `UnpaidWageClaim` redemption (`PopTypeDef.redemption_tier`)
//! 7) three-vault unconditional profit sweep (`Polity.vault` / `Enterprise.vault` & `Trust.vault` / `SectorCapital.vault`)
//! 8) dividends & capital-pool overflow distribution -> 9) head tax -> 10) multi-good staple consumption -> 11) spoilage
//! 12) objective 3-month unresolved arrears bankruptcy exit, natural growth & destitution slide.

use super::*;
use std::collections::{BTreeMap, BTreeSet};
use crate::economy::{
    CounterpartyId, DepotPolicy, BASE_SECTOR_CAPITAL_POOL_CAP_CENTS, BASE_SECTOR_DEPOT_CAPACITY,
    CAPITAL_DEPOT_CAPACITY_BONUS, CORE_DEPOT_CAPACITY_BONUS,
};
use crate::market::{
    clear_bundled_transport_markets, clear_planetary_good_market, compute_per_100_demand,
    release_resting_orders, BundledModeSpec, BundledTransportNetwork, GoodId, Inventory,
    OrderBookSnapshot, SectorOrderBook, TransportChannel, CASH_SCALE, MAX_ORDER_PRICE_CENTS,
    MIN_ORDER_PRICE_CENTS,
};
use crate::rules::{
    execute_rule_effects, EffectExecutionReport, PendingEvent, PendingEventPayload, RuleContext,
    EVENT_BILATERAL_CONTRACT_DUE, EVENT_COUPON_CLAIM_DUE, EVENT_PRIVATE_BANKRUPTCY,
};

#[cfg(test)]
pub(crate) use crate::ai::compute_workshop_ask_cents;

mod audit;
mod fiscal;
use fiscal::{cash_idx, collect_tax_at_source, move_cash, PolitySettleMeta};

const WORKSHOP_RESERVE_MONTHS: i64 = 2;
const WORKSHOP_MIN_RESERVE_CENTS: i64 = 20 * CASH_SCALE;
const DIVIDEND_PAYOUT_PERCENT: i64 = 65;
const ENTERPRISE_DIVIDEND_PERCENT: i64 = 50;
const TRUST_DIVIDEND_PERCENT: i64 = 60;
const HEAD_TAX_CENTS_PER_PERSON: i64 = 4;
const BANKRUPTCY_ARREARS_MONTHS: u8 = 3;

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

#[derive(Default, Clone)]
struct WorkshopMonth {
    staffed_output: i64,
    output: i64,
    staffed_input_cost_cents: i64,
    wage_bill: i64,
    wage_paid: i64,
    revenue: i64,
    new_arrears: Vec<(PopId, i64)>,
}

struct PendingContractLine {
    priority: (u8, u8),
    tile: usize,
    slot: TileSlotKey,
    sector: usize,
    ws_k: Option<usize>,
    payer_inv: usize,
    honors_wages: bool,
    pop_id: PopId,
    group_idx: usize,
    bill_cents: i64,
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

    pub(crate) fn settle_planet_month(&mut self, pid: &str, ruling: PolityId) {
        // ── Phase I: Autonomous Entity Labor & Staffing Decisions ────────────
        self.run_monthly_labor_decisions(pid, ruling);

        // ── Phase II: Objective Physical Production, Market Clearing & Redemption ──
        let goods = &self.goods;
        let buildings = &self.buildings;
        let catalog = &self.pop_catalog;
        let transport_hubs = &self.transport_hubs;
        let events = &self.events;
        let controlled_actor = self.controlled_actor;
        let pending_events = &mut self.pending_events;
        let next_event_id = &mut self.next_event_instance_id;
        let world = &mut self.world;
        let cash = goods.cash_id();
        let tradable_gids = goods.tradable_good_ids();
        let staples = goods.staple_goods();
        let primary_staple_gid = staples.first().map(|&(gid, _)| gid);

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

        // ── 2. Extract every participant Inventory (ALL Polities + Sectors + Pops) ──
        let mut invs: Vec<Inventory> = Vec::new();
        let mut polity_meta: BTreeMap<PolityId, PolitySettleMeta> = BTreeMap::new();
        let mut polity_tariffs: BTreeMap<PolityId, (i64, BTreeSet<PolityId>)> = BTreeMap::new();
        let mut mint_permille = 0i64;
        for (_, pol) in world.query_mut::<&mut Polity>() {
            polity_tariffs.insert(
                pol.id,
                (pol.import_tariff_cents_per_vol, pol.embargoed_polities.clone()),
            );
            let inv_idx = invs.len();
            let mut v = std::mem::take(&mut pol.vault);
            v.ensure_len(goods.len());
            let vb = v.avail(cash);
            invs.push(v);
            if pol.id == ruling {
                mint_permille = pol.monetary_policy.monthly_permille();
            }
            polity_meta.insert(
                pol.id,
                PolitySettleMeta {
                    inv: inv_idx,
                    parent: pol.parent_polity,
                    tax_rate_permille: i64::from(pol.tax_rate_permille),
                    upward_share_permille: i64::from(pol.upward_tax_share_permille.min(1_000)),
                    credit_rating: pol.credit_rating,
                    credit_delta: 0,
                    vault_before: vb,
                    tax_income: 0,
                    state_net: 0,
                    freight_rev: 0,
                    seigniorage: 0,
                    admin_upkeep: 0,
                },
            );
        }
        let Some(ruling_inv) = polity_meta.get(&ruling).map(|m| m.inv) else {
            // Restore any extracted vaults before early return
            for (_, pol) in world.query_mut::<&mut Polity>() {
                if let Some(m) = polity_meta.get(&pol.id) {
                    pol.vault = std::mem::take(&mut invs[m.inv]);
                }
            }
            return;
        };

        let mut slots: Vec<SectorSlots> = vec![SectorSlots::default(); sec_count];
        let mut sec_admin_polity: Vec<PolityId> = vec![ruling; sec_count];
        let mut sector_geo_region: Vec<u16> = vec![u16::MAX; sec_count];
        let mut sector_owner: Vec<PolityId> = vec![ruling; sec_count];
        let mut good_books: BTreeMap<GoodId, Vec<SectorOrderBook>> = BTreeMap::new();
        for &gid in &tradable_gids {
            let gen_px = goods.get(gid).genesis_price_cents;
            good_books.insert(
                gid,
                (0..sec_count)
                    .map(|_| SectorOrderBook::new(gid, gen_px))
                    .collect(),
            );
        }
        let mut depot_policy = vec![DepotPolicy::Idle; sec_count];
        let mut pop_inv_owner: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
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
            market.ensure_good_books(goods);
            slots[s].depot = invs.len();
            invs.push(std::mem::take(&mut sec.state_depot));
            slots[s].capital = invs.len();
            invs.push(std::mem::take(&mut cap.vault));
            slots[s].pops = pops
                .groups
                .iter_mut()
                .enumerate()
                .map(|(gi, g)| {
                    g.sync_capacity();
                    let idx = invs.len();
                    invs.push(std::mem::take(&mut g.inv));
                    pop_inv_owner.insert(idx, (s, gi));
                    idx
                })
                .collect();
            for &gid in &tradable_gids {
                let gen_px = goods.get(gid).genesis_price_cents;
                if let Some(book) = market.good_books.remove(&gid) {
                    if let Some(vec_b) = good_books.get_mut(&gid) {
                        vec_b[s] = book;
                    }
                } else if let Some(vec_b) = good_books.get_mut(&gid) {
                    vec_b[s] = SectorOrderBook::new(gid, gen_px);
                }
            }
            depot_policy[s] = sec.depot_policy;
            sector_geo_region[s] = sec.geo_region_id;
            sector_owner[s] = sec.owner_polity;
            sec_admin_polity[s] = if polity_meta.contains_key(&sec.admin_polity) {
                sec.admin_polity
            } else {
                ruling
            };
        }
        for inv in invs.iter_mut() {
            inv.ensure_len(goods.len());
        }

        let services = goods.service_goods();
        let pool_cap_goods = goods.pool_cap_goods();
        let mut sec_prod = vec![0i64; sec_count];
        let mut sec_good_prod: BTreeMap<(usize, GoodId), i64> = BTreeMap::new();
        let mut sec_sub = vec![0i64; sec_count];
        let mut sec_cons = vec![0i64; sec_count];
        let mut depot_capacity = vec![BASE_SECTOR_DEPOT_CAPACITY; sec_count];
        let mut closing_depths: Vec<BTreeMap<GoodId, OrderBookSnapshot>> =
            vec![BTreeMap::new(); sec_count];
        let mut pop_income: Vec<Vec<i64>> = sec_views
            .iter()
            .map(|v| vec![0i64; v.pops.groups.len()])
            .collect();
        let mut food_met: Vec<Vec<u16>> = sec_views
            .iter()
            .map(|v| vec![1000u16; v.pops.groups.len()])
            .collect();
        let mut service_sat_bonus: Vec<Vec<i16>> = sec_views
            .iter()
            .map(|v| vec![0i16; v.pops.groups.len()])
            .collect();
        let mut bad_debt_sat_hit: Vec<Vec<u8>> = sec_views
            .iter()
            .map(|v| vec![0u8; v.pops.groups.len()])
            .collect();
        let mut bankrupt_released: Vec<(SectorId, Vec<StaffContract>)> = Vec::new();
        let mut bankrupt_acquired: Vec<(usize, TileSlotKey, BuildingOwner)> = Vec::new();
        let mut capital_inflow = vec![0i64; sec_count];
        let mut dividends_paid = vec![0i64; sec_count];
        let mut eval_out: Option<PlanetSimEvaluation> = None;

        for (_, (planet, pt)) in world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != pid {
                continue;
            }
            let eval = evaluate_planet_economy(buildings, catalog, pt, &sec_views);

            for tile in &pt.tiles {
                let s = usize::from(tile.sector_id);
                if s >= sec_count {
                    continue;
                }
                if let Some(tid) = tile.slots.primary {
                    let def = buildings.get(tid);
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

            let mut ent_invs: Vec<usize> = Vec::with_capacity(pt.enterprises.len());
            for ent in &mut pt.enterprises {
                let mut v = std::mem::take(&mut ent.vault);
                v.ensure_len(goods.len());
                ent_invs.push(invs.len());
                invs.push(v);
            }
            let mut trust_invs: Vec<usize> = Vec::with_capacity(pt.trusts.len());
            for tr in &mut pt.trusts {
                let mut v = std::mem::take(&mut tr.vault);
                v.ensure_len(goods.len());
                trust_invs.push(invs.len());
                invs.push(v);
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

            // ── 3. Generic recipe production (`monthly_inputs` -> `monthly_output`) ──
            for (k, w) in ws_refs.iter().enumerate() {
                let Some(tid) = pt.tiles[w.tile].slots.get(w.slot) else {
                    continue;
                };
                let def = buildings.get(tid);
                let scale = i64::from(def.scale_for_slot(w.slot));
                let emp = eval.slot_employed[w.tile][w.slot.as_index()];
                let req_jobs = def.slot_jobs_required(w.slot);
                let bonus = eval.slot_bonus_permille[w.tile][w.slot.as_index()];
                let s = usize::from(pt.tiles[w.tile].sector_id);

                for (in_str, base_in) in &def.monthly_inputs {
                    let needed = scale_output_by_staffing(*base_in * scale, emp, req_jobs);
                    if needed > 0 {
                        if let Some(in_gid) = goods.id_of(in_str) {
                            let in_ma = good_books
                                .get(&in_gid)
                                .and_then(|v| v.get(s))
                                .map(|b| b.ma_price_cents)
                                .unwrap_or(0);
                            ws_month[k].staffed_input_cost_cents += needed * in_ma;
                        }
                    }
                }
                for (out_str, base_out) in &def.monthly_output {
                    if goods.id_of(out_str).is_some() {
                        let with_bonus = ((*base_out * scale) * (1000 + bonus) + 500) / 1000;
                        ws_month[k].staffed_output +=
                            scale_output_by_staffing(with_bonus, emp, req_jobs);
                    }
                }

                let mut input_ratio_permille = 1000i64;
                for (in_str, base_in) in &def.monthly_inputs {
                    let needed = scale_output_by_staffing(*base_in * scale, emp, req_jobs);
                    if needed > 0 {
                        if let Some(in_gid) = goods.id_of(in_str) {
                            let avail = invs[w.inv].avail(in_gid);
                            let r = ((avail * 1000) / needed).clamp(0, 1000);
                            input_ratio_permille = input_ratio_permille.min(r);
                        } else {
                            input_ratio_permille = 0;
                        }
                    }
                }
                if input_ratio_permille > 0 {
                    for (in_str, base_in) in &def.monthly_inputs {
                        let needed = scale_output_by_staffing(*base_in * scale, emp, req_jobs);
                        let burn = (needed * input_ratio_permille + 500) / 1000;
                        if burn > 0 {
                            if let Some(in_gid) = goods.id_of(in_str) {
                                let _ = invs[w.inv].consume_up_to(in_gid, burn);
                            }
                        }
                    }
                    for (out_str, base_out) in &def.monthly_output {
                        let Some(out_gid) = goods.id_of(out_str) else {
                            continue;
                        };
                        let with_bonus = ((*base_out * scale) * (1000 + bonus) + 500) / 1000;
                        let staffed = scale_output_by_staffing(with_bonus, emp, req_jobs);
                        let raw = (staffed * input_ratio_permille + 500) / 1000;
                        let made = invs[w.inv].produce_capped(goods, out_gid, raw);
                        ws_month[k].output += made;
                        if s < sec_count {
                            *sec_good_prod.entry((s, out_gid)).or_insert(0) += made;
                            if Some(out_gid) == primary_staple_gid {
                                sec_prod[s] += made;
                            }
                        }
                    }
                }
                // Wipe any unburned flow-good inputs (`retention_permille == 0`) carried from last month's Step 5
                // so idle workshops can never accumulate >1 month of electricity or industrial water!
                for (in_str, _) in &def.monthly_inputs {
                    if let Some(in_gid) = goods.id_of(in_str) {
                        if goods.get(in_gid).retention_permille == 0 {
                            let rem = invs[w.inv].avail(in_gid);
                            if rem > 0 {
                                invs[w.inv].consume_up_to(in_gid, rem);
                            }
                        }
                    }
                }
            }

            // ── 3B. State Core public baseline outputs (`PlanetaryCapital` / `RegionalCore` -> `Sector.state_depot`) ─
            for tile in &pt.tiles {
                let s = usize::from(tile.sector_id);
                if s >= sec_count {
                    continue;
                }
                let t = usize::from(tile.index);
                for (sk, tid) in tile.slots.iter_occupied() {
                    if ws_at[t][sk.as_index()].is_some() {
                        continue;
                    }
                    let def = buildings.get(tid);
                    if !def.is_state_core_or_public() || def.monthly_output.is_empty() {
                        continue;
                    }
                    let scale = i64::from(def.scale_for_slot(sk));
                    let emp = eval.slot_employed[t][sk.as_index()];
                    let req = def.slot_jobs_required(sk);
                    for (out_str, base_out) in &def.monthly_output {
                        if let Some(out_gid) = goods.id_of(out_str) {
                            if goods.get(out_gid).non_market {
                                continue;
                            }
                            let staffed = scale_output_by_staffing(*base_out * scale, emp, req);
                            if staffed > 0 {
                                let made =
                                    invs[slots[s].depot].produce_capped(goods, out_gid, staffed);
                                *sec_good_prod.entry((s, out_gid)).or_insert(0) += made;
                            }
                        }
                    }
                }
            }
            if let Some(staple_gid) = primary_staple_gid {
                for s in 0..sec_count {
                    for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                        let sub_rate = catalog.get(g.pop_type).subsistence_apple_per_100;
                        if sub_rate > 0 && g.count > 0 {
                            let raw = (i64::from(g.count) * sub_rate) / 100;
                            sec_sub[s] +=
                                invs[slots[s].pops[gi]].produce_capped(goods, staple_gid, raw);
                        }
                    }
                }
            }

            // ── 4. Central bank mints new credits into the sovereign ruling vault ─
            let supply: i64 = invs.iter().map(|i| i.total(cash)).sum();
            let minted = supply * mint_permille / 1000;
            invs[ruling_inv].available[cash_idx(cash)] += minted;
            if let Some(rm) = polity_meta.get_mut(&ruling) {
                rm.seigniorage += minted;
            }

            // ── 4B. Channel-3 Contracts & Standardized Coupon Claims via Declarative `EventCatalog` ─
            let resolve_counterparty_invs =
                |cp: CounterpartyId, sec: usize, good_id: GoodId, pm: &BTreeMap<PolityId, PolitySettleMeta>| -> Option<(usize, usize)> {
                    let is_weightless = goods.get(good_id).unit_volume == 0;
                    match cp {
                        CounterpartyId::Polity { polity_id } => {
                            let p_inv = pm.get(&polity_id)?.inv;
                            let g_inv = if is_weightless || sec >= sec_count {
                                p_inv
                            } else {
                                slots[sec].depot
                            };
                            Some((g_inv, p_inv))
                        }
                        CounterpartyId::Enterprise { enterprise_id } => ent_invs
                            .get(usize::from(enterprise_id.0))
                            .copied()
                            .map(|idx| (idx, idx)),
                        CounterpartyId::Trust { trust_id } => trust_invs
                            .get(usize::from(trust_id.0))
                            .copied()
                            .map(|idx| (idx, idx)),
                    }
                };

            let read_rating = |pt_ref: &PlanetTiles, cp: CounterpartyId, pm: &BTreeMap<PolityId, PolitySettleMeta>| -> i16 {
                match cp {
                    CounterpartyId::Polity { polity_id } => pm
                        .get(&polity_id)
                        .map(|m| (m.credit_rating + m.credit_delta).clamp(0, 100))
                        .unwrap_or(100),
                    CounterpartyId::Enterprise { enterprise_id } => pt_ref
                        .enterprises
                        .get(usize::from(enterprise_id.0))
                        .map(|e| e.credit_rating)
                        .unwrap_or(100),
                    CounterpartyId::Trust { trust_id } => pt_ref
                        .trusts
                        .get(usize::from(trust_id.0))
                        .map(|t| t.credit_rating)
                        .unwrap_or(100),
                }
            };

            let apply_rating =
                |pt_ref: &mut PlanetTiles, cp: CounterpartyId, delta: i16, pm: &mut BTreeMap<PolityId, PolitySettleMeta>| {
                    if delta == 0 {
                        return;
                    }
                    match cp {
                        CounterpartyId::Polity { polity_id } => {
                            if let Some(m) = pm.get_mut(&polity_id) {
                                m.credit_delta = m.credit_delta.saturating_add(delta);
                            }
                        }
                        CounterpartyId::Enterprise { enterprise_id } => {
                            if let Some(ent) = pt_ref.enterprises.get_mut(usize::from(enterprise_id.0))
                            {
                                ent.credit_rating = (ent.credit_rating + delta).clamp(0, 100);
                            }
                        }
                        CounterpartyId::Trust { trust_id } => {
                            if let Some(tr) = pt_ref.trusts.get_mut(usize::from(trust_id.0)) {
                                tr.credit_rating = (tr.credit_rating + delta).clamp(0, 100);
                            }
                        }
                    }
                };

            let mut ent_revenue = vec![0i64; pt.enterprises.len()];
            let mut trust_revenue = vec![0i64; pt.trusts.len()];

            if let Some(contract_ev_def) = events.get(EVENT_BILATERAL_CONTRACT_DUE) {
                let c_len = pt.bilateral_contracts.len();
                for c_i in 0..c_len {
                    let bc_snap = pt.bilateral_contracts[c_i].clone();
                    if bc_snap.remaining_months == 0 || bc_snap.monthly_qty <= 0 {
                        continue;
                    }
                    let s = usize::from(bc_snap.sector_id.0);
                    let Some((seller_g, seller_c)) =
                        resolve_counterparty_invs(bc_snap.seller, s, bc_snap.good, &polity_meta)
                    else {
                        continue;
                    };
                    let Some((buyer_g, buyer_c)) =
                        resolve_counterparty_invs(bc_snap.buyer, s, bc_snap.good, &polity_meta)
                    else {
                        continue;
                    };

                    let ctx = RuleContext {
                        seller_honors: bc_snap.seller_honors,
                        buyer_honors: bc_snap.buyer_honors,
                        seller_cash_cents: invs[seller_c].avail(cash),
                        buyer_cash_cents: invs[buyer_c].avail(cash),
                        seller_good_avail: invs[seller_g].avail(bc_snap.good),
                        buyer_good_fit: invs[buyer_g].max_receivable_units(goods, bc_snap.good),
                        seller_credit_rating: read_rating(pt, bc_snap.seller, &polity_meta),
                        buyer_credit_rating: read_rating(pt, bc_snap.buyer, &polity_meta),
                        contract_qty: bc_snap.monthly_qty,
                        unit_price_cents: bc_snap.unit_price_cents,
                        ..RuleContext::default()
                    };

                    // If a human player explicitly controls either party, queue a `PendingEvent` for manual choice!
                    let player_party = controlled_actor.and_then(|actor| {
                        if actor == bc_snap.seller || actor == bc_snap.buyer {
                            Some(actor)
                        } else {
                            None
                        }
                    });
                    if let Some(target_actor) = player_party {
                        let already_queued = pending_events.iter().any(|pe| {
                            matches!(
                                pe.payload,
                                PendingEventPayload::BilateralContract { contract_id, .. }
                                    if contract_id == bc_snap.id
                            )
                        });
                        if !already_queued {
                            let enabled_options = contract_ev_def
                                .options
                                .iter()
                                .enumerate()
                                .filter(|(_, opt)| opt.trigger.eval(&ctx))
                                .map(|(idx, opt)| (idx, opt.id.clone(), opt.label.clone()))
                                .collect();
                            *next_event_id = (*next_event_id).max(1);
                            let inst_id = *next_event_id;
                            *next_event_id += 1;
                            pending_events.push(PendingEvent {
                                instance_id: inst_id,
                                event_id: contract_ev_def.id.clone(),
                                title: contract_ev_def.title.clone(),
                                target_actor,
                                payload: PendingEventPayload::BilateralContract {
                                    planet_id: pid.to_string(),
                                    contract_id: bc_snap.id,
                                    seller: bc_snap.seller,
                                    buyer: bc_snap.buyer,
                                    sector_id: bc_snap.sector_id,
                                    good: bc_snap.good,
                                    qty: bc_snap.monthly_qty,
                                    unit_price_cents: bc_snap.unit_price_cents,
                                },
                                enabled_options,
                            });
                        }
                        continue;
                    }

                    // Autonomous AI evaluates `if-else +-` weights on `EventDef` and executes `RuleEffect`s!
                    if let Some(opt_idx) = contract_ev_def.choose_ai_option(&ctx) {
                        let mut report = EffectExecutionReport::default();
                        execute_rule_effects(
                            &contract_ev_def.options[opt_idx].effects,
                            &ctx,
                            goods,
                            &mut invs,
                            seller_g,
                            seller_c,
                            buyer_g,
                            buyer_c,
                            Some(bc_snap.good),
                            &[],
                            0,
                            &mut report,
                        );
                        if report.cash_paid_cents > 0 {
                            if let CounterpartyId::Enterprise { enterprise_id } = bc_snap.seller {
                                if let Some(r) = ent_revenue.get_mut(usize::from(enterprise_id.0)) {
                                    *r += report.cash_paid_cents;
                                }
                            }
                            if let CounterpartyId::Trust { trust_id } = bc_snap.seller {
                                if let Some(r) = trust_revenue.get_mut(usize::from(trust_id.0)) {
                                    *r += report.cash_paid_cents;
                                }
                            }
                        }
                        apply_rating(
                            pt,
                            bc_snap.seller,
                            report.seller_rating_delta,
                            &mut polity_meta,
                        );
                        apply_rating(
                            pt,
                            bc_snap.buyer,
                            report.buyer_rating_delta,
                            &mut polity_meta,
                        );
                        let bc_mut = &mut pt.bilateral_contracts[c_i];
                        bc_mut.default_count =
                            bc_mut.default_count.saturating_add(report.default_increment);
                        if report.contract_cancelled {
                            bc_mut.remaining_months = 0;
                        } else {
                            bc_mut.remaining_months = bc_mut.remaining_months.saturating_sub(1);
                        }
                    }
                }
                pt.bilateral_contracts.retain(|bc| bc.remaining_months > 0);
            }

            // ── 4C. Standardized Coupon-Bearing Debt Claims (`GoodDef.coupon_permille_per_month > 0`) ─
            if let Some(coupon_ev_def) = events.get(EVENT_COUPON_CLAIM_DUE) {
                for (claim_gid, issuer_key, coupon_cents_per_unit) in goods.coupon_claim_goods() {
                    let Some(issuer) = CounterpartyId::parse_key(&issuer_key) else {
                        continue;
                    };
                    let Some((_, issuer_cash_inv)) =
                        resolve_counterparty_invs(issuer, 0, cash, &polity_meta)
                    else {
                        continue;
                    };
                    let mut holders: Vec<(usize, i64)> = Vec::new();
                    let mut total_held = 0i64;
                    for (inv_i, inv) in invs.iter().enumerate() {
                        if inv_i == issuer_cash_inv {
                            continue;
                        }
                        let held = inv.total(claim_gid);
                        if held > 0 {
                            holders.push((inv_i, held));
                            total_held += held;
                        }
                    }
                    if total_held <= 0 {
                        continue;
                    }
                    let ctx = RuleContext {
                        seller_honors: true,
                        buyer_honors: true,
                        seller_cash_cents: invs[issuer_cash_inv].avail(cash),
                        seller_credit_rating: read_rating(pt, issuer, &polity_meta),
                        contract_qty: total_held,
                        unit_price_cents: coupon_cents_per_unit,
                        ..RuleContext::default()
                    };
                    if controlled_actor == Some(issuer) {
                        let already_queued = pending_events.iter().any(|pe| {
                            matches!(
                                pe.payload,
                                PendingEventPayload::CouponClaim { claim_good, .. }
                                    if claim_good == claim_gid
                            )
                        });
                        if !already_queued {
                            let enabled_options = coupon_ev_def
                                .options
                                .iter()
                                .enumerate()
                                .filter(|(_, opt)| opt.trigger.eval(&ctx))
                                .map(|(idx, opt)| (idx, opt.id.clone(), opt.label.clone()))
                                .collect();
                            *next_event_id = (*next_event_id).max(1);
                            let inst_id = *next_event_id;
                            *next_event_id += 1;
                            pending_events.push(PendingEvent {
                                instance_id: inst_id,
                                event_id: coupon_ev_def.id.clone(),
                                title: coupon_ev_def.title.clone(),
                                target_actor: issuer,
                                payload: PendingEventPayload::CouponClaim {
                                    planet_id: pid.to_string(),
                                    issuer,
                                    claim_good: claim_gid,
                                    coupon_cents_per_unit,
                                },
                                enabled_options,
                            });
                        }
                        continue;
                    }
                    if let Some(opt_idx) = coupon_ev_def.choose_ai_option(&ctx) {
                        let holder_cash_before: Vec<(usize, i64)> = holders
                            .iter()
                            .map(|&(h_inv, _)| (h_inv, invs[h_inv].avail(cash)))
                            .collect();
                        let mut report = EffectExecutionReport::default();
                        execute_rule_effects(
                            &coupon_ev_def.options[opt_idx].effects,
                            &ctx,
                            goods,
                            &mut invs,
                            issuer_cash_inv,
                            issuer_cash_inv,
                            issuer_cash_inv,
                            issuer_cash_inv,
                            Some(claim_gid),
                            &holders,
                            coupon_cents_per_unit,
                            &mut report,
                        );
                        for (h_inv, before_c) in holder_cash_before {
                            let gain = (invs[h_inv].avail(cash) - before_c).max(0);
                            if gain > 0 {
                                if let Some(&(s, gi)) = pop_inv_owner.get(&h_inv) {
                                    pop_income[s][gi] += gain;
                                    dividends_paid[s] += gain;
                                }
                            }
                        }
                        apply_rating(
                            pt,
                            issuer,
                            report.seller_rating_delta,
                            &mut polity_meta,
                        );
                    }
                }
            }

            // ── 5. Multi-Good Spatial Order-Book Quoting & Clearing ───────────
            let depot_invs: Vec<usize> = slots.iter().map(|s| s.depot).collect();
            let pop_invs: Vec<Vec<usize>> = slots.iter().map(|s| s.pops.clone()).collect();
            let ws_tuples: Vec<(usize, TileSlotKey, usize)> =
                ws_refs.iter().map(|w| (w.tile, w.slot, w.inv)).collect();

            let ws_cash_before: Vec<i64> = ws_refs.iter().map(|w| invs[w.inv].avail(cash)).collect();
            let bundled_modes: Vec<BundledModeSpec> = transport_hubs
                .iter()
                .filter_map(|h| {
                    goods.id_of(&h.capacity_good_id).map(|cap_good| BundledModeSpec {
                        kind: h.kind,
                        cap_good,
                        endpoint_shares: h.endpoint_shares,
                        steps_per_extra_share: h.steps_per_extra_share,
                        max_extra_shares: h.max_extra_shares,
                    })
                })
                .collect();

            if bundled_modes.is_empty() {
                for &gid in &tradable_gids {
                    let Some(books_for_gid) = good_books.get_mut(&gid) else {
                        continue;
                    };
                    let (sells, buys) = crate::ai::collect_monthly_commodity_orders(
                        goods,
                        buildings,
                        catalog,
                        pt,
                        &sec_views,
                        &depot_policy,
                        books_for_gid,
                        &invs,
                        ruling_inv,
                        &depot_invs,
                        &pop_invs,
                        &ws_tuples,
                        gid,
                    );
                    clear_planetary_good_market(
                        goods,
                        gid,
                        books_for_gid,
                        &pt.logistics,
                        &mut invs,
                        ruling_inv,
                        sells,
                        buys,
                    );
                    for (d, book) in books_for_gid.iter().enumerate() {
                        let f_rev: i64 = book
                            .recent_trades
                            .iter()
                            .filter(|f| usize::from(f.to_sector) == d)
                            .map(|f| f.freight_cents * f.qty)
                            .sum();
                        if let Some(rm) = polity_meta.get_mut(&ruling) {
                            rm.freight_rev += f_rev;
                        }
                        closing_depths[d].insert(gid, book.to_snapshot(5));
                    }
                    release_resting_orders(goods, gid, books_for_gid, &mut invs);
                }
            } else {
                let mut cap_gid_set: BTreeSet<GoodId> =
                    bundled_modes.iter().map(|m| m.cap_good).collect();
                let mut bundled_orders: BTreeMap<GoodId, (Vec<crate::market::SellSubmission>, Vec<crate::market::BuySubmission>)> =
                    BTreeMap::new();

                for &gid in &tradable_gids {
                    let ch = goods.get(gid).transport_channel;
                    let use_bundled = cap_gid_set.contains(&gid)
                        || matches!(ch, TransportChannel::Physical | TransportChannel::PowerGrid);
                    if use_bundled {
                        let Some(books_ref) = good_books.get(&gid) else {
                            continue;
                        };
                        let orders = crate::ai::collect_monthly_commodity_orders(
                            goods,
                            buildings,
                            catalog,
                            pt,
                            &sec_views,
                            &depot_policy,
                            books_ref,
                            &invs,
                            ruling_inv,
                            &depot_invs,
                            &pop_invs,
                            &ws_tuples,
                            gid,
                        );
                        bundled_orders.insert(gid, orders);
                    } else {
                        let Some(books_for_gid) = good_books.get_mut(&gid) else {
                            continue;
                        };
                        let (sells, buys) = crate::ai::collect_monthly_commodity_orders(
                            goods,
                            buildings,
                            catalog,
                            pt,
                            &sec_views,
                            &depot_policy,
                            books_for_gid,
                            &invs,
                            ruling_inv,
                            &depot_invs,
                            &pop_invs,
                            &ws_tuples,
                            gid,
                        );
                        clear_planetary_good_market(
                            goods,
                            gid,
                            books_for_gid,
                            &pt.logistics,
                            &mut invs,
                            ruling_inv,
                            sells,
                            buys,
                        );
                        for (d, book) in books_for_gid.iter().enumerate() {
                            closing_depths[d].insert(gid, book.to_snapshot(5));
                        }
                        release_resting_orders(goods, gid, books_for_gid, &mut invs);
                    }
                }

                // Build sector customs & geography lookup tables for `clear_bundled_transport_markets`
                let mut embargoed = vec![false; sec_count * sec_count];
                let mut tariff_per_vol_cents = vec![0i64; sec_count * sec_count];
                let mut tariff_receiver_inv = vec![ruling_inv; sec_count];
                for d in 0..sec_count {
                    let dst_pol = sector_owner[d];
                    if let Some(pm) = polity_meta.get(&dst_pol) {
                        tariff_receiver_inv[d] = pm.inv;
                    }
                    let (t_rate, emb_set) = polity_tariffs
                        .get(&dst_pol)
                        .cloned()
                        .unwrap_or_default();
                    for s in 0..sec_count {
                        let src_pol = sector_owner[s];
                        if src_pol != dst_pol {
                            embargoed[s * sec_count + d] = emb_set.contains(&src_pol);
                            tariff_per_vol_cents[s * sec_count + d] = t_rate.max(0);
                        }
                    }
                }

                let mut polity_freight_by_inv: BTreeMap<usize, i64> = polity_meta
                    .values()
                    .map(|m| (m.inv, 0i64))
                    .collect();

                let mut inv_coast_id = vec![u16::MAX; invs.len()];
                for w in &ws_refs {
                    if let Some(&cid) = pt.geo_topology.tile_coast_id.get(w.tile) {
                        inv_coast_id[w.inv] = cid;
                    }
                }

                let bundled_gids: Vec<GoodId> = bundled_orders.keys().copied().collect();
                let net_ctx = BundledTransportNetwork {
                    sector_count: sec_count,
                    sector_geo_region: &sector_geo_region,
                    geo: &pt.geo_topology,
                    modes: &bundled_modes,
                    inv_coast_id: &inv_coast_id,
                    embargoed: &embargoed,
                    tariff_per_vol_cents: &tariff_per_vol_cents,
                    tariff_receiver_inv: &tariff_receiver_inv,
                };
                clear_bundled_transport_markets(
                    goods,
                    &net_ctx,
                    &mut good_books,
                    bundled_orders,
                    &mut invs,
                    &mut polity_freight_by_inv,
                );

                for pm in polity_meta.values_mut() {
                    if let Some(&f_rev) = polity_freight_by_inv.get(&pm.inv) {
                        pm.freight_rev += f_rev;
                    }
                }
                for &gid in &bundled_gids {
                    if let Some(books_for_gid) = good_books.get_mut(&gid) {
                        for (d, book) in books_for_gid.iter().enumerate() {
                            closing_depths[d].insert(gid, book.to_snapshot(5));
                        }
                        release_resting_orders(goods, gid, books_for_gid, &mut invs);
                    }
                }
                cap_gid_set.clear();
            }
            for (k, w) in ws_refs.iter().enumerate() {
                ws_month[k].revenue = invs[w.inv].avail(cash) - ws_cash_before[k];
            }

            // ── 6A. Priority-Ordered Contract Wage Redemption (`PopTypeDef.redemption_tier`) ─
            let mut lines: Vec<PendingContractLine> = Vec::new();
            let mut cbd_bill_by_sec = vec![0i64; sec_count];
            let mut ent_campus_bill = vec![0i64; pt.enterprises.len()];
            let mut trust_hub_bill = vec![0i64; pt.trusts.len()];

            for tile in &pt.tiles {
                let s = usize::from(tile.sector_id);
                if s >= sec_count {
                    continue;
                }
                let t = usize::from(tile.index);
                let is_cap_tile = tile
                    .slots
                    .iter_occupied()
                    .any(|(_, btid)| buildings.get(btid).tags.has(crate::BuildingTag::PlanetaryCapital));
                for (sk, tid) in tile.slots.iter_occupied() {
                    let def = buildings.get(tid);
                    let ws_k = ws_at[t][sk.as_index()];
                    let honors = pt.slot_honors_wages(t, sk);
                    let owner = pt.slot_owner(t, sk);
                    let payer_inv = if let Some(k) = ws_k {
                        ws_refs[k].inv
                    } else if def.is_state_core_or_public() {
                        match owner {
                            BuildingOwner::State { polity_id } => polity_meta
                                .get(&polity_id)
                                .map(|m| m.inv)
                                .unwrap_or(ruling_inv),
                            _ => ruling_inv,
                        }
                    } else {
                        match owner {
                            BuildingOwner::Enterprise { enterprise_id } => ent_invs
                                .get(usize::from(enterprise_id.0))
                                .copied()
                                .unwrap_or(slots[s].capital),
                            BuildingOwner::Trust { trust_id } => trust_invs
                                .get(usize::from(trust_id.0))
                                .copied()
                                .unwrap_or(slots[s].capital),
                            BuildingOwner::LocalPrivate { home_sector } => {
                                let h = usize::from(home_sector.0);
                                if h < sec_count {
                                    slots[h].capital
                                } else {
                                    slots[s].capital
                                }
                            }
                            BuildingOwner::State { polity_id } => polity_meta
                                .get(&polity_id)
                                .map(|m| m.inv)
                                .unwrap_or(ruling_inv),
                            BuildingOwner::Unowned => ruling_inv,
                        }
                    };

                    for c in &pt.slot_contracts[t][sk.as_index()] {
                        if c.count == 0 || c.monthly_bill_cents <= 0 {
                            continue;
                        }
                        if ws_k.is_none() && !def.is_state_core_or_public() {
                            match owner {
                                BuildingOwner::Enterprise { enterprise_id } => {
                                    if let Some(eb) =
                                        ent_campus_bill.get_mut(usize::from(enterprise_id.0))
                                    {
                                        *eb += c.monthly_bill_cents;
                                    }
                                }
                                BuildingOwner::Trust { trust_id } => {
                                    if let Some(tb) =
                                        trust_hub_bill.get_mut(usize::from(trust_id.0))
                                    {
                                        *tb += c.monthly_bill_cents;
                                    }
                                }
                                BuildingOwner::LocalPrivate { home_sector } => {
                                    let h = usize::from(home_sector.0);
                                    if h < sec_count {
                                        cbd_bill_by_sec[h] += c.monthly_bill_cents;
                                    }
                                }
                                _ => {}
                            }
                        }
                        let Some(gi) = sec_views[s].pops.index_by_id(c.pop_id) else {
                            continue;
                        };
                        let role_id = sec_views[s].pops.groups[gi].pop_type;
                        let priority = crate::ai::contract_redemption_priority(
                            catalog,
                            def,
                            role_id,
                            is_cap_tile,
                        );
                        lines.push(PendingContractLine {
                            priority,
                            tile: t,
                            slot: sk,
                            sector: s,
                            ws_k,
                            payer_inv,
                            honors_wages: honors,
                            pop_id: c.pop_id,
                            group_idx: gi,
                            bill_cents: c.monthly_bill_cents,
                        });
                    }
                }
            }
            lines.sort_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| a.tile.cmp(&b.tile))
                    .then_with(|| a.slot.as_index().cmp(&b.slot.as_index()))
                    .then_with(|| a.pop_id.cmp(&b.pop_id))
            });

            for line in lines {
                if let Some(k) = line.ws_k {
                    ws_month[k].wage_bill += line.bill_cents;
                }
                let gross = if line.honors_wages {
                    line.bill_cents.min(invs[line.payer_inv].avail(cash)).max(0)
                } else {
                    0
                };
                if gross > 0 {
                    let admin_pid = sec_admin_polity[line.sector];
                    let sec_tax_rate = polity_meta
                        .get(&admin_pid)
                        .map(|m| m.tax_rate_permille)
                        .unwrap_or(150);
                    let t_tax = gross * sec_tax_rate / 1000;
                    move_cash(
                        &mut invs,
                        cash,
                        line.payer_inv,
                        slots[line.sector].pops[line.group_idx],
                        gross - t_tax,
                    );
                    collect_tax_at_source(
                        &mut invs,
                        cash,
                        line.payer_inv,
                        t_tax,
                        admin_pid,
                        &mut polity_meta,
                        ruling,
                    );
                    pop_income[line.sector][line.group_idx] += gross - t_tax;
                    for m in polity_meta.values_mut() {
                        if m.inv == line.payer_inv && line.ws_k.is_none() {
                            m.admin_upkeep += gross;
                            break;
                        }
                    }
                    if let Some(k) = line.ws_k {
                        ws_month[k].wage_paid += gross;
                    }
                }
                let unpaid = line.bill_cents - gross;
                if unpaid > 0 {
                    if let Some(k) = line.ws_k {
                        ws_month[k].new_arrears.push((line.pop_id, unpaid));
                    }
                }
            }

            // ── 6B. Senior `UnpaidWageClaim` Redemption ───────────────────────
            for (k, w) in ws_refs.iter().enumerate() {
                let s = usize::from(pt.tiles[w.tile].sector_id);
                if s >= sec_count {
                    continue;
                }
                let honors_wages = pt.slot_honors_wages(w.tile, w.slot);
                let Some(ws) = pt.workshops[w.tile][w.slot.as_index()].as_mut() else {
                    continue;
                };
                for &(pop_id, unpaid) in &ws_month[k].new_arrears {
                    ws.record_unpaid_claim(pop_id, unpaid);
                }
                if !honors_wages || ws.unpaid_claims.is_empty() {
                    continue;
                }
                for claim in ws.unpaid_claims.iter_mut() {
                    if claim.owed_cents <= 0 {
                        continue;
                    }
                    let avail_cash = invs[w.inv].avail(cash);
                    if avail_cash <= 0 {
                        break;
                    }
                    let Some(gi) = sec_views[s].pops.index_by_id(claim.pop_id) else {
                        claim.owed_cents = 0;
                        continue;
                    };
                    let gross = claim.owed_cents.min(avail_cash);
                    if gross > 0 {
                        let admin_pid = sec_admin_polity[s];
                        let sec_tax_rate = polity_meta
                            .get(&admin_pid)
                            .map(|m| m.tax_rate_permille)
                            .unwrap_or(150);
                        let t_tax = gross * sec_tax_rate / 1000;
                        move_cash(&mut invs, cash, w.inv, slots[s].pops[gi], gross - t_tax);
                        collect_tax_at_source(
                            &mut invs,
                            cash,
                            w.inv,
                            t_tax,
                            admin_pid,
                            &mut polity_meta,
                            ruling,
                        );
                        pop_income[s][gi] += gross - t_tax;
                        claim.owed_cents -= gross;
                    }
                }
                ws.unpaid_claims.retain(|c| c.owed_cents > 0);
            }

            // ── 7. Three-Vault Bilateral Profit Sweep & Deficit Coverage ────────
            let mut ent_profit = vec![0i64; pt.enterprises.len()];
            let mut ent_net_inflow = vec![0i64; pt.enterprises.len()];
            let mut trust_profit = vec![0i64; pt.trusts.len()];
            let mut trust_net_inflow = vec![0i64; pt.trusts.len()];

            for (k, w) in ws_refs.iter().enumerate() {
                let owner = pt.slot_owner(w.tile, w.slot);
                match owner {
                    BuildingOwner::Enterprise { enterprise_id } => {
                        if let Some(r) = ent_revenue.get_mut(usize::from(enterprise_id.0)) {
                            *r += ws_month[k].revenue;
                        }
                    }
                    BuildingOwner::Trust { trust_id } => {
                        if let Some(r) = trust_revenue.get_mut(usize::from(trust_id.0)) {
                            *r += ws_month[k].revenue;
                        }
                    }
                    _ => {}
                }

                let s = usize::from(pt.tiles[w.tile].sector_id);
                let admin_pid = if s < sec_count {
                    sec_admin_polity[s]
                } else {
                    ruling
                };
                let sec_tax_rate = polity_meta
                    .get(&admin_pid)
                    .map(|m| m.tax_rate_permille)
                    .unwrap_or(150);

                let honors_wages = pt.slot_honors_wages(w.tile, w.slot);
                let unpaid_total = pt.workshops[w.tile][w.slot.as_index()]
                    .as_ref()
                    .map(|ws| ws.total_unpaid_claims_cents())
                    .unwrap_or(0);
                let has_unpaid = unpaid_total > 0 || ws_month[k].wage_paid < ws_month[k].wage_bill;

                let is_named_entity = matches!(
                    owner,
                    BuildingOwner::State { .. }
                        | BuildingOwner::Enterprise { .. }
                        | BuildingOwner::Trust { .. }
                );
                let reserve = if ws_month[k].wage_bill > 0 || ws_month[k].staffed_input_cost_cents > 0 {
                    ((ws_month[k].wage_bill + ws_month[k].staffed_input_cost_cents)
                        * WORKSHOP_RESERVE_MONTHS)
                        .max(WORKSHOP_MIN_RESERVE_CENTS)
                } else if is_named_entity && honors_wages {
                    if let Some(tid) = pt.tiles[w.tile].slots.get(w.slot) {
                        let def = buildings.get(tid);
                        let target_jobs = pt.effective_slot_target_jobs(w.tile, w.slot, def);
                        let full_jobs = def.slot_jobs_required(w.slot).max(1);
                        if target_jobs == 0 {
                            0
                        } else {
                            let scale = i64::from(def.scale_for_slot(w.slot));
                            let mut base_wage_cents = 0i64;
                            for (role_str, base_req) in &def.jobs_by_role {
                                let rid = def.resolve_role_id(catalog, role_str);
                                let rw = catalog.get(rid).base_wage_per_100 * CASH_SCALE / 100;
                                base_wage_cents += i64::from(*base_req) * scale * rw;
                            }
                            let mut base_in_cents = 0i64;
                            for (in_str, base_amt) in &def.monthly_inputs {
                                if let Some(igid) = goods.id_of(in_str) {
                                    let px = good_books
                                        .get(&igid)
                                        .and_then(|v| v.get(s))
                                        .map(|b| b.ma_price_cents)
                                        .unwrap_or_else(|| goods.get(igid).genesis_price_cents);
                                    base_in_cents += base_amt * scale * px;
                                }
                            }
                            (((base_wage_cents + base_in_cents) * i64::from(target_jobs))
                                / i64::from(full_jobs))
                            .max(WORKSHOP_MIN_RESERVE_CENTS)
                        }
                    } else {
                        WORKSHOP_MIN_RESERVE_CENTS
                    }
                } else {
                    WORKSHOP_MIN_RESERVE_CENTS
                };

                let cur_avail = invs[w.inv].avail(cash);
                let deficit_floor = if ws_month[k].wage_bill > 0
                    || ws_month[k].staffed_input_cost_cents > 0
                {
                    (ws_month[k].wage_bill + ws_month[k].staffed_input_cost_cents)
                        .max(WORKSHOP_MIN_RESERVE_CENTS)
                } else {
                    reserve
                };

                // Named entities (`State`, `Enterprise`, `Trust`) cover workshop working-capital deficits
                // below 1-month operating floor (plus any unpaid wages) from their own entity vaults;
                // `LocalPrivate` absorbs losses on its own!
                if is_named_entity && honors_wages && cur_avail < deficit_floor + unpaid_total {
                    let needed = (deficit_floor + unpaid_total) - cur_avail;
                    let moved = match owner {
                        BuildingOwner::State { polity_id } => {
                            let (owner_inv, core_keep) = polity_meta
                                .get(&polity_id)
                                .map(|m| (m.inv, m.admin_upkeep))
                                .unwrap_or((ruling_inv, 0));
                            let transferable =
                                (invs[owner_inv].avail(cash) - core_keep).max(0).min(needed);
                            let m_cash = move_cash(&mut invs, cash, owner_inv, w.inv, transferable);
                            if m_cash > 0 {
                                if let Some(m) = polity_meta.get_mut(&polity_id) {
                                    m.state_net -= m_cash;
                                }
                            }
                            m_cash
                        }
                        BuildingOwner::Enterprise { enterprise_id } => {
                            let e = usize::from(enterprise_id.0);
                            if let Some(&e_inv) = ent_invs.get(e) {
                                let m_cash = move_cash(&mut invs, cash, e_inv, w.inv, needed);
                                if m_cash > 0 {
                                    ent_profit[e] -= m_cash;
                                    ent_net_inflow[e] -= m_cash;
                                }
                                m_cash
                            } else {
                                0
                            }
                        }
                        BuildingOwner::Trust { trust_id } => {
                            let tr = usize::from(trust_id.0);
                            if let Some(&tr_inv) = trust_invs.get(tr) {
                                let m_cash = move_cash(&mut invs, cash, tr_inv, w.inv, needed);
                                if m_cash > 0 {
                                    trust_profit[tr] -= m_cash;
                                    trust_net_inflow[tr] -= m_cash;
                                }
                                m_cash
                            } else {
                                0
                            }
                        }
                        _ => 0,
                    };
                    if moved > 0 && s < sec_count {
                        if let Some(ws) = pt.workshops[w.tile][w.slot.as_index()].as_mut() {
                            if !ws.unpaid_claims.is_empty() {
                                for claim in ws.unpaid_claims.iter_mut() {
                                    if claim.owed_cents <= 0 {
                                        continue;
                                    }
                                    let avail_now = invs[w.inv].avail(cash);
                                    if avail_now <= 0 {
                                        break;
                                    }
                                    let Some(gi) = sec_views[s].pops.index_by_id(claim.pop_id) else {
                                        claim.owed_cents = 0;
                                        continue;
                                    };
                                    let gross = claim.owed_cents.min(avail_now);
                                    if gross > 0 {
                                        let t_tax = gross * sec_tax_rate / 1000;
                                        move_cash(
                                            &mut invs,
                                            cash,
                                            w.inv,
                                            slots[s].pops[gi],
                                            gross - t_tax,
                                        );
                                        collect_tax_at_source(
                                            &mut invs,
                                            cash,
                                            w.inv,
                                            t_tax,
                                            admin_pid,
                                            &mut polity_meta,
                                            ruling,
                                        );
                                        pop_income[s][gi] += gross - t_tax;
                                        ws_month[k].wage_paid += gross;
                                        claim.owed_cents -= gross;
                                    }
                                }
                                ws.unpaid_claims.retain(|c| c.owed_cents > 0);
                            }
                        }
                    }
                    continue;
                }

                if has_unpaid {
                    continue;
                }
                let excess = cur_avail - reserve;
                if excess <= 0 {
                    continue;
                }

                match owner {
                    BuildingOwner::LocalPrivate { home_sector } => {
                        let h = usize::from(home_sector.0);
                        if h < sec_count {
                            let tax = excess * sec_tax_rate / 1000;
                            let paid_tax = collect_tax_at_source(
                                &mut invs,
                                cash,
                                w.inv,
                                tax,
                                admin_pid,
                                &mut polity_meta,
                                ruling,
                            );
                            let net = move_cash(
                                &mut invs,
                                cash,
                                w.inv,
                                slots[h].capital,
                                excess - paid_tax,
                            );
                            capital_inflow[h] += net;
                        }
                    }
                    BuildingOwner::Enterprise { enterprise_id } => {
                        let e = usize::from(enterprise_id.0);
                        if let Some(&e_inv) = ent_invs.get(e) {
                            let tax = excess * sec_tax_rate / 1000;
                            let paid_tax = collect_tax_at_source(
                                &mut invs,
                                cash,
                                w.inv,
                                tax,
                                admin_pid,
                                &mut polity_meta,
                                ruling,
                            );
                            let net =
                                move_cash(&mut invs, cash, w.inv, e_inv, excess - paid_tax);
                            ent_profit[e] += excess;
                            ent_net_inflow[e] += net;
                        }
                    }
                    BuildingOwner::Trust { trust_id } => {
                        let tr = usize::from(trust_id.0);
                        if let Some(&tr_inv) = trust_invs.get(tr) {
                            let tax = excess * sec_tax_rate / 1000;
                            let paid_tax = collect_tax_at_source(
                                &mut invs,
                                cash,
                                w.inv,
                                tax,
                                admin_pid,
                                &mut polity_meta,
                                ruling,
                            );
                            let net =
                                move_cash(&mut invs, cash, w.inv, tr_inv, excess - paid_tax);
                            trust_profit[tr] += excess;
                            trust_net_inflow[tr] += net;
                        }
                    }
                    BuildingOwner::State { polity_id } => {
                        // Cross-polity & domestic SOEs: pay host-sector tax if owned by an external sovereign tree,
                        // then sweep 100% of remaining post-tax profit directly into `polity_id`'s own `Polity.vault`!
                        let owner_inv = polity_meta
                            .get(&polity_id)
                            .map(|m| m.inv)
                            .unwrap_or(ruling_inv);
                        let same_sovereign = {
                            let root_of = |mut p: PolityId| -> PolityId {
                                for _ in 0..16 {
                                    if let Some(par) =
                                        polity_meta.get(&p).and_then(|m| m.parent)
                                    {
                                        p = par;
                                    } else {
                                        break;
                                    }
                                }
                                p
                            };
                            root_of(polity_id) == root_of(admin_pid)
                        };
                        let paid_tax = if !same_sovereign {
                            let tax = excess * sec_tax_rate / 1000;
                            collect_tax_at_source(
                                &mut invs,
                                cash,
                                w.inv,
                                tax,
                                admin_pid,
                                &mut polity_meta,
                                ruling,
                            )
                        } else {
                            0
                        };
                        let net = move_cash(&mut invs, cash, w.inv, owner_inv, excess - paid_tax);
                        if let Some(m) = polity_meta.get_mut(&polity_id) {
                            m.state_net += net;
                        }
                    }
                    BuildingOwner::Unowned => {
                        let net = move_cash(&mut invs, cash, w.inv, ruling_inv, excess);
                        if let Some(m) = polity_meta.get_mut(&ruling) {
                            m.state_net += net;
                        }
                    }
                }
            }
            for (e_i, ent) in pt.enterprises.iter_mut().enumerate() {
                ent.last_revenue_cents = ent_revenue[e_i];
                ent.last_profit_cents = ent_profit[e_i];
            }
            for (t_i, tr) in pt.trusts.iter_mut().enumerate() {
                tr.last_revenue_cents = trust_revenue[t_i];
                tr.last_profit_cents = trust_profit[t_i];
            }

            // ── 8A. Dynamic Service-Scaled `SectorCapital` Cap, Household Surplus Savings Injection & Dividends ─
            let div_recipients = catalog.dividend_recipients();
            for s in 0..sec_count {
                let mut dynamic_service_cap = 0i64;
                for &(cgid, cents_per_unit) in &pool_cap_goods {
                    let prod = sec_good_prod.get(&(s, cgid)).copied().unwrap_or(0);
                    dynamic_service_cap += prod.saturating_mul(cents_per_unit);
                }
                let pool_cap =
                    BASE_SECTOR_CAPITAL_POOL_CAP_CENTS + dynamic_service_cap + cbd_bill_by_sec[s] * 6;

                // Household surplus investment into `SectorCapital` when pool has room below `80%` of `pool_cap`
                if invs[slots[s].capital].avail(cash) < (pool_cap * 80) / 100 {
                    for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                        if g.count == 0 {
                            continue;
                        }
                        let pdef = catalog.get(g.pop_type);
                        if pdef.surplus_invest_permille == 0 {
                            continue;
                        }
                        let mut basic_monthly = i64::from(g.count) * HEAD_TAX_CENTS_PER_PERSON;
                        for &(sgid, per_100) in &staples {
                            basic_monthly += compute_per_100_demand(g.count, per_100)
                                * goods.get(sgid).genesis_price_cents;
                        }
                        for &(svgid, per_100, _) in &services {
                            basic_monthly += compute_per_100_demand(g.count, per_100)
                                * goods.get(svgid).genesis_price_cents;
                        }
                        let reserve =
                            basic_monthly.max(CASH_SCALE) * i64::from(pdef.reserve_months.max(1));
                        let avail_c = invs[slots[s].pops[gi]].avail(cash);
                        if avail_c > reserve {
                            let room = ((pool_cap * 80) / 100 - invs[slots[s].capital].avail(cash))
                                .max(0);
                            if room == 0 {
                                break;
                            }
                            let inject = (((avail_c - reserve)
                                * i64::from(pdef.surplus_invest_permille))
                                / 2_000)
                                .min(room);
                            if inject > 0 {
                                move_cash(
                                    &mut invs,
                                    cash,
                                    slots[s].pops[gi],
                                    slots[s].capital,
                                    inject,
                                );
                            }
                        }
                    }
                }

                let mut holders: Vec<(usize, u32)> = sec_views[s]
                    .pops
                    .groups
                    .iter()
                    .enumerate()
                    .filter_map(|(gi, g)| {
                        (g.count > 0
                            && div_recipients.contains(&g.pop_type)
                            && matches!(
                                g.employer,
                                BuildingOwner::LocalPrivate { .. } | BuildingOwner::Unowned
                            ))
                        .then_some((gi, g.count))
                    })
                    .collect();
                if holders.is_empty() {
                    holders = sec_views[s]
                        .pops
                        .groups
                        .iter()
                        .enumerate()
                        .filter_map(|(gi, g)| {
                            (g.count > 0 && div_recipients.contains(&g.pop_type))
                                .then_some((gi, g.count))
                        })
                        .collect();
                }
                let total: u32 = holders.iter().map(|&(_, c)| c).sum();
                if total == 0 {
                    continue;
                }
                let base_payout = capital_inflow[s] * DIVIDEND_PAYOUT_PERCENT / 100;
                let pool_now = invs[slots[s].capital].avail(cash);
                let overflow = (pool_now - base_payout - pool_cap).max(0);
                let total_payout = (base_payout + overflow).min(pool_now).max(0);

                let mut paid_sum = 0i64;
                for &(gi, c) in &holders {
                    let share = total_payout * i64::from(c) / i64::from(total);
                    let moved =
                        move_cash(&mut invs, cash, slots[s].capital, slots[s].pops[gi], share);
                    pop_income[s][gi] += moved;
                    paid_sum += moved;
                }
                dividends_paid[s] += paid_sum;
            }

            // ── 8B. Corporate `Enterprise.vault` Dividends ────────────────────
            for (e_i, ent) in pt.enterprises.iter_mut().enumerate() {
                let e_inv = ent_invs[e_i];
                let e_owner = BuildingOwner::Enterprise {
                    enterprise_id: ent.id,
                };
                let mut holders: Vec<(usize, usize, u32)> = Vec::new();
                for s in 0..sec_count {
                    for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                        if g.count > 0
                            && div_recipients.contains(&g.pop_type)
                            && g.employer == e_owner
                        {
                            holders.push((s, gi, g.count));
                        }
                    }
                }
                let total: u32 = holders.iter().map(|&(_, _, c)| c).sum();
                if total == 0 {
                    continue;
                }
                let vault_now = invs[e_inv].avail(cash);
                let corp_reserve = ent_campus_bill[e_i] * 6 + 150 * CASH_SCALE;
                let distributable = (vault_now - corp_reserve).max(0);
                let base_div =
                    (ent_net_inflow[e_i] * ENTERPRISE_DIVIDEND_PERCENT / 100).min(distributable);
                let corp_cap = 900 * CASH_SCALE + ent_campus_bill[e_i] * 6;
                let overflow = (vault_now - base_div - corp_cap).max(0);
                let total_payout = (base_div + overflow).min(distributable);
                for &(s, gi, c) in &holders {
                    let share = total_payout * i64::from(c) / i64::from(total);
                    let moved = move_cash(&mut invs, cash, e_inv, slots[s].pops[gi], share);
                    pop_income[s][gi] += moved;
                    dividends_paid[s] += moved;
                }
            }

            // ── 8C. Standardized `Trust` Equity Share Dividends ───────────────
            for (t_i, tr) in pt.trusts.iter().enumerate() {
                let Some(eq_gid) = tr.equity_good else {
                    continue;
                };
                let tr_inv = trust_invs[t_i];
                let mut share_holders: Vec<(usize, i64)> = Vec::new();
                let mut total_held = 0i64;
                for (inv_idx, inv) in invs.iter().enumerate() {
                    if inv_idx == tr_inv {
                        continue;
                    }
                    let held = inv.total(eq_gid);
                    if held > 0 {
                        share_holders.push((inv_idx, held));
                        total_held += held;
                    }
                }
                if total_held <= 0 {
                    continue;
                }
                let vault_now = invs[tr_inv].avail(cash);
                let tr_reserve = trust_hub_bill[t_i] * 6 + 150 * CASH_SCALE;
                let distributable = (vault_now - tr_reserve).max(0);
                let payout = (trust_net_inflow[t_i] * TRUST_DIVIDEND_PERCENT / 100)
                    .max((vault_now - (1_200 * CASH_SCALE + tr_reserve)).max(0))
                    .min(distributable);
                if payout <= 0 {
                    continue;
                }
                for (holder_inv, shares) in share_holders {
                    let amt =
                        ((i128::from(payout) * i128::from(shares)) / i128::from(total_held)) as i64;
                    let moved = move_cash(&mut invs, cash, tr_inv, holder_inv, amt);
                    if moved > 0 {
                        if let Some(&(s, gi)) = pop_inv_owner.get(&holder_inv) {
                            pop_income[s][gi] += moved;
                            dividends_paid[s] += moved;
                        }
                    }
                }
            }

            // ── 9. Source-shared Head Tax, 10. Dual-track Staple & Service Consumption ─
            for s in 0..sec_count {
                let admin_pid = sec_admin_polity[s];
                for (gi, g) in sec_views[s].pops.groups.iter().enumerate() {
                    if g.count == 0 {
                        food_met[s][gi] = 1000;
                        continue;
                    }
                    let inv = slots[s].pops[gi];
                    collect_tax_at_source(
                        &mut invs,
                        cash,
                        inv,
                        i64::from(g.count) * HEAD_TAX_CENTS_PER_PERSON,
                        admin_pid,
                        &mut polity_meta,
                        ruling,
                    );
                    let mut total_need = 0i64;
                    let mut total_eaten = 0i64;
                    for &(sgid, per_100) in &staples {
                        let need = compute_per_100_demand(g.count, per_100);
                        let eaten = invs[inv].consume_up_to(sgid, need);
                        if Some(sgid) == primary_staple_gid {
                            sec_cons[s] += eaten;
                        }
                        total_need += need;
                        total_eaten += eaten;
                    }
                    food_met[s][gi] = if total_need <= 0 {
                        1000
                    } else {
                        ((total_eaten * 1000) / total_need).clamp(0, 1000) as u16
                    };

                    // Service consumption: consumes baseline + surplus purchases; excess converts into satisfaction bonus!
                    let mut sat_bonus = 0i16;
                    for &(svgid, svc_per_100, bonus_pts) in &services {
                        let need = compute_per_100_demand(g.count, svc_per_100);
                        if need <= 0 {
                            continue;
                        }
                        let held = invs[inv].avail(svgid);
                        let used = invs[inv].consume_up_to(svgid, held);
                        if used >= need {
                            sat_bonus = sat_bonus.saturating_add(2);
                            let extra = used - need;
                            if extra > 0 && bonus_pts > 0 {
                                let extra_pts =
                                    ((extra * i64::from(bonus_pts)) / need).clamp(0, 14) as i16;
                                sat_bonus = sat_bonus.saturating_add(extra_pts);
                            }
                        }
                    }
                    service_sat_bonus[s][gi] = sat_bonus.clamp(0, 16);
                }
            }

            // ── 11. Spoilage on every warehouse (preserving 1x purchased flow-good inputs in workshops for next Step 3!) ─
            let mut saved_ws_flow_inputs: Vec<(usize, GoodId, i64)> = Vec::new();
            for w in &ws_refs {
                if let Some(tid) = pt.tiles[w.tile].slots.get(w.slot) {
                    let def = buildings.get(tid);
                    let scale = i64::from(def.scale_for_slot(w.slot));
                    for (in_str, base_in) in &def.monthly_inputs {
                        if let Some(in_gid) = goods.id_of(in_str) {
                            if goods.get(in_gid).retention_permille == 0 {
                                let keep = invs[w.inv].avail(in_gid).min(*base_in * scale);
                                if keep > 0 {
                                    saved_ws_flow_inputs.push((w.inv, in_gid, keep));
                                }
                            }
                        }
                    }
                }
            }
            for inv in invs.iter_mut() {
                inv.apply_monthly_retention(goods);
            }
            for (w_inv, in_gid, keep) in saved_ws_flow_inputs {
                invs[w_inv].produce_capped(goods, in_gid, keep);
            }

            // ── 12. Restore corporate & workshop warehouses, execute `LocalPrivate` 3-month Bankruptcy ─
            let bankruptcy_ev_opt = events.get(EVENT_PRIVATE_BANKRUPTCY).cloned();
            for (k, w) in ws_refs.iter().enumerate() {
                let s = usize::from(pt.tiles[w.tile].sector_id);
                let owner = pt.slot_owner(w.tile, w.slot);
                let is_local_private = matches!(owner, BuildingOwner::LocalPrivate { .. });

                let mut trigger_bankruptcy = false;
                if let Some(ws) = pt.workshops[w.tile][w.slot.as_index()].as_mut() {
                    ws.inv = std::mem::take(&mut invs[w.inv]);
                    ws.last_output = ws_month[k].output;
                    ws.last_wage_paid_cents = ws_month[k].wage_paid;
                    ws.last_revenue_cents = ws_month[k].revenue;
                    let unresolved = ws.total_unpaid_claims_cents() > 0;
                    ws.had_wage_arrears = ws_month[k].wage_paid < ws_month[k].wage_bill || unresolved;
                    if ws.had_wage_arrears {
                        ws.consecutive_arrears_months =
                            ws.consecutive_arrears_months.saturating_add(1);
                    } else {
                        ws.consecutive_arrears_months = 0;
                    }
                    if ws_month[k].staffed_output > 0 {
                        ws.last_unit_cost_cents = ((ws_month[k].wage_bill
                            + ws_month[k].staffed_input_cost_cents)
                            / ws_month[k].staffed_output)
                            .clamp(MIN_ORDER_PRICE_CENTS, MAX_ORDER_PRICE_CENTS);
                    } else {
                        ws.last_unit_cost_cents = 0;
                    }
                    if is_local_private
                        && ws.consecutive_arrears_months >= BANKRUPTCY_ARREARS_MONTHS
                        && unresolved
                        && ws.inv.used_volume(goods) == 0
                    {
                        trigger_bankruptcy = true;
                    }
                }

                if trigger_bankruptcy && s < sec_count {
                    let BuildingOwner::LocalPrivate { home_sector } = owner else {
                        continue;
                    };
                    let h = usize::from(home_sector.0);
                    let cap_dest = if h < sec_count {
                        slots[h].capital
                    } else {
                        slots[s].capital
                    };
                    let admin_pid = sec_admin_polity[s];
                    let admin_inv = polity_meta
                        .get(&admin_pid)
                        .map(|m| m.inv)
                        .unwrap_or(ruling_inv);
                    let sec_tax_rate = polity_meta
                        .get(&admin_pid)
                        .map(|m| m.tax_rate_permille)
                        .unwrap_or(150);

                    if let Some(mut dead_ws) = pt.workshops[w.tile][w.slot.as_index()].take() {
                        // First apply any residual cash in the bankrupt workshop toward unpaid wages
                        let c = cash_idx(cash);
                        for claim in &mut dead_ws.unpaid_claims {
                            if claim.owed_cents > 0 {
                                if let Some(gi) = sec_views[s].pops.index_by_id(claim.pop_id) {
                                    let rem = dead_ws.inv.available[c].min(claim.owed_cents).max(0);
                                    if rem > 0 {
                                        dead_ws.inv.available[c] -= rem;
                                        invs[slots[s].pops[gi]].available[c] += rem;
                                        pop_income[s][gi] += rem;
                                        claim.owed_cents -= rem;
                                    }
                                }
                            }
                        }
                        dead_ws.unpaid_claims.retain(|cl| cl.owed_cents > 0);
                        let rem_unpaid: i64 = dead_ws.total_unpaid_claims_cents();

                        let (salvage_cents, restart_reserve, margin_permille) = pt.tiles[w.tile]
                            .slots
                            .get(w.slot)
                            .map(|tid| {
                                let def = buildings.get(tid);
                                let build_cents = def.slot_build_cost(w.slot) * CASH_SCALE;
                                let salvage = (build_cents * 25) / 100;
                                let restart = (ws_month[k].wage_bill
                                    + ws_month[k].staffed_input_cost_cents)
                                    .max(WORKSHOP_MIN_RESERVE_CENTS);
                                let mut good_ma_map = BTreeMap::new();
                                for &gid in &tradable_gids {
                                    let px = good_books
                                        .get(&gid)
                                        .and_then(|v| v.get(s))
                                        .map(|b| b.ma_price_cents)
                                        .unwrap_or_else(|| goods.get(gid).genesis_price_cents);
                                    good_ma_map.insert(gid, px);
                                }
                                let req = def.slot_jobs_required(w.slot).max(1);
                                let net_v = def.slot_expected_net_value_cents(
                                    goods,
                                    &good_ma_map,
                                    w.slot,
                                    req,
                                    0,
                                    i64::MAX / 8,
                                );
                                let m_perm = if build_cents > 0 {
                                    (net_v * 1000) / build_cents
                                } else {
                                    0
                                };
                                (salvage, restart, m_perm)
                            })
                            .unwrap_or((0, WORKSHOP_MIN_RESERVE_CENTS, 0));

                        let full_acq_bill = salvage_cents + rem_unpaid + restart_reserve;
                        let core_keep = polity_meta
                            .get(&admin_pid)
                            .map(|m| m.admin_upkeep)
                            .unwrap_or(0);
                        let buyer_avail = (invs[admin_inv].avail(cash) - core_keep).max(0);

                        let ctx = RuleContext {
                            buyer_cash_cents: buyer_avail,
                            contract_qty: 1,
                            buyer_good_fit: 1,
                            unit_price_cents: full_acq_bill,
                            margin_permille,
                            build_cost_cents: salvage_cents,
                            ..RuleContext::default()
                        };

                        let target_cp = CounterpartyId::Polity {
                            polity_id: admin_pid,
                        };
                        if controlled_actor == Some(target_cp) {
                            if let Some(ref ev_def) = bankruptcy_ev_opt {
                                let already_queued = pending_events.iter().any(|pe| {
                                    matches!(
                                        pe.payload,
                                        PendingEventPayload::BankruptSlot {
                                            tile_index,
                                            slot,
                                            ..
                                        } if tile_index == w.tile && slot == w.slot
                                    )
                                });
                                if !already_queued {
                                    let enabled_options = ev_def
                                        .options
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, opt)| opt.trigger.eval(&ctx))
                                        .map(|(idx, opt)| (idx, opt.id.clone(), opt.label.clone()))
                                        .collect();
                                    *next_event_id = (*next_event_id).max(1);
                                    let inst_id = *next_event_id;
                                    *next_event_id += 1;
                                    pending_events.push(PendingEvent {
                                        instance_id: inst_id,
                                        event_id: ev_def.id.clone(),
                                        title: ev_def.title.clone(),
                                        target_actor: target_cp,
                                        payload: PendingEventPayload::BankruptSlot {
                                            planet_id: pid.to_string(),
                                            tile_index: w.tile,
                                            slot: w.slot,
                                            admin_polity: admin_pid,
                                            home_sector,
                                            salvage_price_cents: salvage_cents,
                                        },
                                        enabled_options,
                                    });
                                }
                                pt.workshops[w.tile][w.slot.as_index()] = Some(dead_ws);
                                continue;
                            }
                        }

                        let mut acquire_approved = false;
                        if let Some(ref ev_def) = bankruptcy_ev_opt {
                            if let Some(opt_idx) = ev_def.choose_ai_option(&ctx) {
                                let mut report = EffectExecutionReport::default();
                                execute_rule_effects(
                                    &ev_def.options[opt_idx].effects,
                                    &ctx,
                                    goods,
                                    &mut invs,
                                    cap_dest,
                                    cap_dest,
                                    admin_inv,
                                    admin_inv,
                                    None,
                                    &[],
                                    0,
                                    &mut report,
                                );
                                acquire_approved = report.bankruptcy_acquisition_approved
                                    && buyer_avail >= full_acq_bill;
                            }
                        }

                        if acquire_approved {
                            // Option 1: Host/local Polity acquires the bankrupt plant at salvage value,
                            // settles remaining worker wage claims, and injects working capital!
                            let paid_salvage =
                                move_cash(&mut invs, cash, admin_inv, cap_dest, salvage_cents);
                            capital_inflow[if h < sec_count { h } else { s }] += paid_salvage;
                            for claim in &mut dead_ws.unpaid_claims {
                                if claim.owed_cents > 0 {
                                    if let Some(gi) = sec_views[s].pops.index_by_id(claim.pop_id) {
                                        let gross =
                                            claim.owed_cents.min(invs[admin_inv].avail(cash)).max(0);
                                        if gross > 0 {
                                            let t_tax = gross * sec_tax_rate / 1000;
                                            move_cash(
                                                &mut invs,
                                                cash,
                                                admin_inv,
                                                slots[s].pops[gi],
                                                gross - t_tax,
                                            );
                                            collect_tax_at_source(
                                                &mut invs,
                                                cash,
                                                admin_inv,
                                                t_tax,
                                                admin_pid,
                                                &mut polity_meta,
                                                ruling,
                                            );
                                            pop_income[s][gi] += gross - t_tax;
                                            claim.owed_cents -= gross;
                                        }
                                    }
                                }
                            }
                            dead_ws.unpaid_claims.clear();
                            dead_ws.had_wage_arrears = false;
                            dead_ws.consecutive_arrears_months = 0;
                            let topup = restart_reserve.min(invs[admin_inv].avail(cash)).max(0);
                            if topup > 0 {
                                invs[admin_inv].available[c] -= topup;
                                dead_ws.inv.available[c] += topup;
                            }
                            if let Some(m) = polity_meta.get_mut(&admin_pid) {
                                m.state_net -= paid_salvage + rem_unpaid + topup;
                            }
                            let new_owner = BuildingOwner::State {
                                polity_id: admin_pid,
                            };
                            pt.workshops[w.tile][w.slot.as_index()] = Some(dead_ws);
                            pt.set_slot_owner(w.tile, w.slot, new_owner);
                            pt.reset_slot_directives(w.tile, w.slot);
                            bankrupt_acquired.push((w.tile, w.slot, new_owner));
                        } else {
                            // Option 2: Decline acquisition and demolish the bankrupt slot cleanly!
                            for claim in &dead_ws.unpaid_claims {
                                if claim.owed_cents > 0 {
                                    if let Some(gi) = sec_views[s].pops.index_by_id(claim.pop_id) {
                                        bad_debt_sat_hit[s][gi] =
                                            bad_debt_sat_hit[s][gi].saturating_add(6).min(18);
                                    }
                                }
                            }
                            let leftover_cash = dead_ws.inv.avail(cash);
                            if leftover_cash > 0 {
                                dead_ws.inv.available[c] = 0;
                                invs[cap_dest].available[c] += leftover_cash;
                            }
                            let released =
                                std::mem::take(&mut pt.slot_contracts[w.tile][w.slot.as_index()]);
                            if !released.is_empty() {
                                bankrupt_released.push((SectorId(s as u16), released));
                            }
                            pt.tiles[w.tile].slots.set(w.slot, None);
                            pt.set_slot_owner(w.tile, w.slot, BuildingOwner::Unowned);
                            pt.reset_slot_directives(w.tile, w.slot);

                            // If a demolished primary slot had dependent secondary buildings with `required_primary_tags`,
                            // clean up only those dependent secondary slots while keeping ordinary independent secondaries running!
                            if w.slot.is_primary() {
                                for sec_sk in [
                                    TileSlotKey::Secondary0,
                                    TileSlotKey::Secondary1,
                                    TileSlotKey::Secondary2,
                                ] {
                                    let dep = pt.tiles[w.tile]
                                        .slots
                                        .get(sec_sk)
                                        .map(|stid| {
                                            buildings.get(stid).required_primary_tags
                                                != BuildingTags::EMPTY
                                        })
                                        .unwrap_or(false);
                                    if dep {
                                        if let Some(mut dep_ws) =
                                            pt.workshops[w.tile][sec_sk.as_index()].take()
                                        {
                                            let dep_cash = dep_ws.inv.avail(cash);
                                            if dep_cash > 0 {
                                                dep_ws.inv.available[c] = 0;
                                                invs[cap_dest].available[c] += dep_cash;
                                            }
                                        }
                                        let dep_rel = std::mem::take(
                                            &mut pt.slot_contracts[w.tile][sec_sk.as_index()],
                                        );
                                        if !dep_rel.is_empty() {
                                            bankrupt_released.push((SectorId(s as u16), dep_rel));
                                        }
                                        pt.tiles[w.tile].slots.set(sec_sk, None);
                                        pt.set_slot_owner(w.tile, sec_sk, BuildingOwner::Unowned);
                                        pt.reset_slot_directives(w.tile, sec_sk);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            for (e_i, ent) in pt.enterprises.iter_mut().enumerate() {
                ent.vault = std::mem::take(&mut invs[ent_invs[e_i]]);
            }
            for (t_i, tr) in pt.trusts.iter_mut().enumerate() {
                tr.vault = std::mem::take(&mut invs[trust_invs[t_i]]);
            }
            eval_out = Some(eval);
            break;
        }
        let Some(eval) = eval_out else {
            return;
        };

        // ── Restore all Polity vaults and fiscal statistics ──────────────────
        for (_, pol) in world.query_mut::<&mut Polity>() {
            if let Some(m) = polity_meta.get(&pol.id) {
                pol.vault = std::mem::take(&mut invs[m.inv]);
                pol.credit_rating = (m.credit_rating + m.credit_delta).clamp(0, 100);
                pol.last_tax_income_cents = m.tax_income;
                pol.last_state_enterprise_net_cents = m.state_net;
                pol.last_freight_revenue_cents = m.freight_rev;
                pol.last_seigniorage_cents = m.seigniorage;
                pol.last_admin_upkeep_cents = m.admin_upkeep;
                pol.last_net_cashflow_cents = pol.vault.avail(cash) - m.vault_before;
            }
        }

        // ── Restore sectors, update Pops (satisfaction, natural growth, destitution slide) ─
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
            cap.last_dividends_cents = dividends_paid[s];
            cap.last_reinvested_cents = cap
                .vault
                .avail(cash)
                .min((capital_inflow[s] - dividends_paid[s]).max(0));
            for &gid in &tradable_gids {
                if let Some(vec_b) = good_books.get_mut(&gid) {
                    let gen_px = goods.get(gid).genesis_price_cents;
                    let b = std::mem::replace(&mut vec_b[s], SectorOrderBook::new(gid, gen_px));
                    market.good_books.insert(gid, b);
                }
            }
            market.closing_depths = closing_depths.get(s).cloned().unwrap_or_default();
            market.last_production = sec_prod[s];
            market.last_subsistence_output = sec_sub[s];
            market.last_consumption = sec_cons[s];
            let primary_ma = primary_staple_gid
                .and_then(|sgid| market.book(sgid).map(|b| b.ma_price_cents))
                .unwrap_or(CASH_SCALE * 10);

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
                    && !g_def.is_self_subsistence()
                    && !is_vagrant
                    && !g_def.receives_dividends;
                let food_bill: i64 = staples
                    .iter()
                    .map(|&(sgid, per_100)| {
                        let px = market
                            .book(sgid)
                            .map(|b| b.ma_price_cents)
                            .unwrap_or(goods.get(sgid).genesis_price_cents);
                        compute_per_100_demand(g.count, per_100) * px
                    })
                    .sum();
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
                    if pop_income[s][gi] > food_bill {
                        84
                    } else {
                        76
                    }
                } else if met >= 700 {
                    62
                } else {
                    40
                };
                target_sat = target_sat.saturating_add(service_sat_bonus[s][gi]);
                let base_w = g_def.base_wage_per_100 * CASH_SCALE / 100;
                let (c_emp, c_bill) = eval.pop_contracted[s].get(gi).copied().unwrap_or((0, 0));
                if base_w > 0 && c_emp > 0 && c_bill * 100 < i64::from(c_emp) * base_w * 85 {
                    target_sat -= 6;
                }

                let cur = i16::from(g.satisfaction) - i16::from(bad_debt_sat_hit[s][gi]);
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

            let mut demotions: Vec<(PopTypeId, u16, PopTypeId, u32)> = Vec::new();
            for g in &pops.groups {
                if g.count == 0 || g.employer != BuildingOwner::Unowned {
                    continue;
                }
                let g_def = catalog.get(g.pop_type);
                if g_def.is_self_subsistence() || !g_def.can_demote {
                    continue;
                }
                let is_vag = has_vag && g.pop_type == vag_pool_id;
                if is_vag {
                    if g.last_food_met_permille < 700
                        && g.count > 30
                        && catalog.get(default_pool_id).is_self_subsistence()
                    {
                        let back = ((g.count - 30) / 10).clamp(1, 4);
                        demotions.push((g.pop_type, g.culture_id, default_pool_id, back));
                    }
                    continue;
                }
                if g.inv.avail(cash) < primary_ma.max(CASH_SCALE) && g.last_food_met_permille < 750
                {
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

        if !bankrupt_acquired.is_empty() {
            self.migrate_slot_contracts_owners(pid, &bankrupt_acquired);
        }
        for (sid, contracts) in bankrupt_released {
            self.release_contracts_to_between_jobs(pid, sid, contracts, 8);
        }
    }
}
