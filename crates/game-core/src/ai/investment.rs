//! Autonomous Private Capital, Corporate & Sub-National Polity Investment (`AI 自主建厂投资决策`).

use crate::building::{BuildingDef, BuildingTag, TileSlotKey};
use crate::economy::{
    BuildingOwner, EnterpriseId, Polity, PolityId, Sector, SectorCapital, SectorId, SectorMarket,
    SectorPops, ENTERPRISE_OUTPUT_BONUS_PERMILLE,
};
use crate::market::{compute_per_100_demand, GoodCatalog, GoodId, CASH_SCALE};
use crate::planet::{make_workshop, CashSink, Planet, PlanetTiles};
use crate::Game;
use std::collections::BTreeMap;

impl Game {
    pub(crate) fn run_autonomous_private_investment(
        &mut self,
        planet_id: &str,
        sec_allow_land: &BTreeMap<SectorId, bool>,
    ) {
        struct SectorEconSnap {
            pool_cents: i64,
            good_ma: BTreeMap<GoodId, i64>,
            unsold_asks: BTreeMap<GoodId, i64>,
            supply_avail: BTreeMap<GoodId, i64>,
            ref_demand: BTreeMap<GoodId, i64>,
            unowned_seekers: u32,
            wage_ma_by_role: Vec<i64>,
        }

        let type_count = self.pop_catalog.len();
        let mut ind_demand: BTreeMap<(SectorId, GoodId), i64> = BTreeMap::new();
        let mut planet_ind_demand: BTreeMap<GoodId, i64> = BTreeMap::new();
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id != planet_id {
                continue;
            }
            for tile in &pt.tiles {
                if tile.sector_id == u16::MAX {
                    continue;
                }
                let sid = SectorId(tile.sector_id);
                for (sk, tid) in tile.slots.iter_occupied() {
                    let bdef = self.buildings.get(tid);
                    let scale = i64::from(bdef.scale_for_slot(sk));
                    for (in_str, base_in) in &bdef.monthly_inputs {
                        if let Some(in_gid) = self.goods.id_of(in_str) {
                            let need = (*base_in * scale).max(0);
                            *ind_demand.entry((sid, in_gid)).or_insert(0) += need;
                            *planet_ind_demand.entry(in_gid).or_insert(0) += need;
                        }
                    }
                }
            }
            break;
        }

        let mut sec_snaps: BTreeMap<SectorId, SectorEconSnap> = BTreeMap::new();
        let mut planet_supply: BTreeMap<GoodId, i64> = BTreeMap::new();
        for (_, (sec, market, cap, pops)) in self
            .world
            .query::<(&Sector, &SectorMarket, &SectorCapital, &SectorPops)>()
            .iter()
        {
            if sec.planet_id != planet_id {
                continue;
            }
            let pop_total = pops.total_population();
            let mut good_ma = BTreeMap::new();
            let mut unsold_asks = BTreeMap::new();
            let mut supply_avail = BTreeMap::new();
            let mut ref_demand = BTreeMap::new();
            for gid in self.goods.tradable_good_ids() {
                let gdef = self.goods.get(gid);
                let ma = market
                    .book(gid)
                    .map(|b| b.ma_price_cents)
                    .unwrap_or(gdef.genesis_price_cents);
                good_ma.insert(gid, ma);

                let asks_qty: i64 = market
                    .closing_depth(gid)
                    .map(|ob| ob.asks.iter().map(|l| l.qty).sum())
                    .or_else(|| {
                        market
                            .book(gid)
                            .map(|b| b.asks.iter().map(|o| o.remaining_qty).sum())
                    })
                    .unwrap_or(0);
                unsold_asks.insert(gid, asks_qty);

                let traded_vol = market
                    .book(gid)
                    .map(|b| b.last_contract_volume + b.last_spot_volume)
                    .unwrap_or(0);
                let avail_total = traded_vol + asks_qty;
                supply_avail.insert(gid, avail_total);
                *planet_supply.entry(gid).or_insert(0) += avail_total;
                let local_ind = ind_demand.get(&(sec.id, gid)).copied().unwrap_or(0);
                let planet_ind = planet_ind_demand.get(&gid).copied().unwrap_or(0);
                let base_dem = if gdef.staple_demand_per_100 > 0 {
                    compute_per_100_demand(pop_total, gdef.staple_demand_per_100) + local_ind
                } else if gdef.service_demand_per_100 > 0 {
                    compute_per_100_demand(pop_total, gdef.service_demand_per_100) + local_ind
                } else if gdef.retention_permille == 0 {
                    // Grid/utility flows (`power`, `water`, freight): allow baseline pioneer buildout (`>= 16`)
                    traded_vol.max(local_ind).max(16)
                } else {
                    // Storable intermediate/industrial goods: require actual downstream industry or trade demand!
                    traded_vol.max(local_ind).max(planet_ind.min(20))
                };
                ref_demand.insert(gid, base_dem);
            }
            let unowned_seekers: u32 = pops
                .groups
                .iter()
                .filter(|g| {
                    g.employer == BuildingOwner::Unowned
                        && !self.pop_catalog.get(g.pop_type).receives_dividends
                })
                .map(|g| g.count)
                .sum();
            let wage_ma_by_role: Vec<i64> = (0..type_count)
                .map(|i| {
                    market
                        .wage_books
                        .get(i)
                        .map(|wb| wb.ma_wage_cents)
                        .unwrap_or(self.pop_catalog.defs()[i].base_wage_per_100 * CASH_SCALE / 100)
                })
                .collect();
            sec_snaps.insert(
                sec.id,
                SectorEconSnap {
                    pool_cents: cap.pool_cents(&self.goods),
                    good_ma,
                    unsold_asks,
                    supply_avail,
                    ref_demand,
                    unowned_seekers,
                    wage_ma_by_role,
                },
            );
        }

        let mut spent_sinks: Vec<(CashSink, i64)> = Vec::new();
        let buildings = &self.buildings;
        let cbd_def = buildings
            .defs()
            .iter()
            .find(|d| d.tags.has(BuildingTag::Commercial));

        let has_viable_inputs = |goods_ref: &GoodCatalog,
                                 def: &BuildingDef,
                                 sk: TileSlotKey,
                                 ts: &SectorEconSnap,
                                 p_sup: &BTreeMap<GoodId, i64>|
         -> bool {
            let scale = i64::from(def.scale_for_slot(sk));
            def.monthly_inputs.iter().all(|(in_str, base_in)| {
                let req = (*base_in * scale).max(1);
                let Some(gid) = goods_ref.id_of(in_str) else {
                    return false;
                };
                let gdef = goods_ref.get(gid);
                if gdef.retention_permille == 0 {
                    // Flow utilities (`power`, `water`, etc.): require spare resting supply >= `req`
                    // so AI never builds a power-hungry desalination plant when local grid has zero spare power!
                    let spare = ts.unsold_asks.get(&gid).copied().unwrap_or(0);
                    spare >= req
                } else {
                    let avail = if gdef.transport_channel == crate::market::TransportChannel::LocalOnly {
                        ts.supply_avail.get(&gid).copied().unwrap_or(0)
                    } else {
                        p_sup.get(&gid).copied().unwrap_or(0)
                    };
                    avail >= req
                }
            })
        };
        let is_output_oversupplied =
            |goods_ref: &GoodCatalog, def: &BuildingDef, ts: &SectorEconSnap| -> bool {
                def.monthly_output.iter().any(|(good_str, base_out)| {
                    goods_ref
                        .id_of(good_str)
                        .map(|gid| {
                            let asks = ts.unsold_asks.get(&gid).copied().unwrap_or(0);
                            let sup = ts.supply_avail.get(&gid).copied().unwrap_or(0);
                            let dem = ts.ref_demand.get(&gid).copied().unwrap_or(0);
                            if dem <= 0 {
                                return true;
                            }
                            if goods_ref.get(gid).retention_permille == 0 {
                                (sup >= dem && asks > 0)
                                    || asks >= (*base_out).max(4) / 2
                                    || asks * 4 > dem
                            } else {
                                asks > dem
                            }
                        })
                        .unwrap_or(false)
                })
            };

        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            let mut cbd_payroll_cents: BTreeMap<SectorId, i64> = BTreeMap::new();
            let mut ent_payroll_cents: BTreeMap<EnterpriseId, i64> = BTreeMap::new();
            for tile in &pt.tiles {
                if tile.sector_id == u16::MAX {
                    continue;
                }
                let t_idx = usize::from(tile.index);
                for (sk, tid) in tile.slots.iter_occupied() {
                    let bdef = buildings.get(tid);
                    if pt.workshops[t_idx][sk.as_index()].is_none()
                        && !bdef.is_state_core_or_public()
                    {
                        let bill: i64 = pt.slot_contracts[t_idx][sk.as_index()]
                            .iter()
                            .map(|c| c.monthly_bill_cents)
                            .sum();
                        match pt.slot_owner(t_idx, sk) {
                            BuildingOwner::Enterprise { enterprise_id } => {
                                *ent_payroll_cents.entry(enterprise_id).or_insert(0) += bill;
                            }
                            BuildingOwner::LocalPrivate { home_sector } => {
                                *cbd_payroll_cents.entry(home_sector).or_insert(0) += bill;
                            }
                            _ => {
                                *cbd_payroll_cents
                                    .entry(SectorId(tile.sector_id))
                                    .or_insert(0) += bill;
                            }
                        }
                    }
                }
            }

            let eval_slot_margin =
                |def: &BuildingDef, sk: TileSlotKey, bonus_permille: i64, ts: &SectorEconSnap| -> i64 {
                    let scale = def.scale_for_slot(sk);
                    let mut est_rev = 0i64;
                    for (good_str, base_out) in &def.monthly_output {
                        if let Some(gid) = self.goods.id_of(good_str) {
                            let px = ts.good_ma.get(&gid).copied().unwrap_or(0);
                            let out =
                                ((*base_out * i64::from(scale)) * (1000 + bonus_permille) + 500)
                                    / 1000;
                            est_rev += out * px;
                        }
                    }
                    let mut est_in = 0i64;
                    for (good_str, base_in) in &def.monthly_inputs {
                        if let Some(gid) = self.goods.id_of(good_str) {
                            let px = ts.good_ma.get(&gid).copied().unwrap_or(0);
                            est_in += *base_in * i64::from(scale) * px;
                        }
                    }
                    let mut est_wage = 0i64;
                    for (role_str, base_req) in &def.jobs_by_role {
                        let rid = def.resolve_role_id(&self.pop_catalog, role_str);
                        let w = ts
                            .wage_ma_by_role
                            .get(usize::from(rid.0))
                            .copied()
                            .unwrap_or(0);
                        est_wage += i64::from(*base_req * scale) * w;
                    }
                    est_rev - est_in - est_wage
                };

            // ── A. Named `Enterprise` corporate expansion from its own `Enterprise.vault` ──
            for ent_i in 0..pt.enterprises.len() {
                let ent_id = pt.enterprises[ent_i].id;
                let ent_hq = pt.enterprises[ent_i].hq_sector;
                let ent_tag = pt.enterprises[ent_i].industry_tag.clone();
                let ent_cap = pt.enterprises[ent_i].hosting_capacity;
                let ent_cash = pt.enterprises[ent_i].cash_cents(&self.goods);
                if pt.enterprise_hosted_count(buildings, ent_id) >= ent_cap {
                    continue;
                }
                let reserve =
                    ent_payroll_cents.get(&ent_id).copied().unwrap_or(0) * 6 + 60 * CASH_SCALE;
                let mut best_ent: Option<(&BuildingDef, usize, TileSlotKey, i64, i64)> = None;
                for def in buildings
                    .defs()
                    .iter()
                    .filter(|d| d.is_privatizable_industry() && !d.monthly_output.is_empty())
                {
                    if !def.tags.iter().any(|tag| tag.as_str() == ent_tag) {
                        continue;
                    }
                    for t in &pt.tiles {
                        if t.sector_id == u16::MAX
                            || !def.matches_tags(t.tags)
                            || !sec_allow_land
                                .get(&SectorId(t.sector_id))
                                .copied()
                                .unwrap_or(false)
                        {
                            continue;
                        }
                        let t_sec = SectorId(t.sector_id);
                        let Some(ts) = sec_snaps.get(&t_sec) else {
                            continue;
                        };
                        if is_output_oversupplied(&self.goods, def, ts) {
                            continue;
                        }
                        let has_primary = t.slots.primary.is_some();
                        for sk in t.slots.candidate_empty_slots(buildings, def).into_iter().flatten() {
                            if !has_viable_inputs(&self.goods, def, sk, ts, &planet_supply) {
                                continue;
                            }
                            let cost_cents = def.slot_build_cost(sk) * CASH_SCALE;
                            if ent_cash < cost_cents + reserve {
                                continue;
                            }
                            let req_jobs = def.slot_jobs_required(sk);
                            if ts.unowned_seekers < (req_jobs / 2).max(1) {
                                continue;
                            }
                            let margin =
                                eval_slot_margin(def, sk, ENTERPRISE_OUTPUT_BONUS_PERMILLE, ts);
                            if margin <= 0 || margin * 20 < cost_cents {
                                continue;
                            }
                            let sec_pref = if !sk.is_primary() && has_primary { 150 } else { 0 };
                            let hq_pref = if t_sec == ent_hq { 200 } else { 0 };
                            let score = margin
                                + i64::from(t.tags.habitability_permille()) / 4
                                + sec_pref
                                + hq_pref;
                            if best_ent.as_ref().map_or(true, |b| score > b.4) {
                                best_ent = Some((def, usize::from(t.index), sk, cost_cents, score));
                            }
                        }
                    }
                }
                if let Some((def, target_tile, target_sk, cost_cents, _)) = best_ent {
                    pt.tiles[target_tile]
                        .slots
                        .set(target_sk, Some(def.type_id));
                    pt.set_slot_owner(
                        target_tile,
                        target_sk,
                        BuildingOwner::Enterprise {
                            enterprise_id: ent_id,
                        },
                    );
                    pt.reset_slot_directives(target_tile, target_sk);
                    pt.workshops[target_tile][target_sk.as_index()] =
                        make_workshop(&self.goods, &self.pop_catalog, def, target_sk);
                    spent_sinks.push((CashSink::Enterprise(ent_id), cost_cents));
                }
            }

            // ── B. `SectorCapital` (`LocalPrivate` pool): Event-driven commercial facility emergence + free industry expansion ──
            let mut investor_order: Vec<(SectorId, i64)> = sec_snaps
                .iter()
                .map(|(&sid, s)| (sid, s.pool_cents))
                .collect();
            investor_order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            for &(inv_sec, pool_cents) in &investor_order {
                let (cbd_count, _, _, _) = pt.sector_cbd_hosting_stats(buildings, inv_sec);
                let hq_reserve_cents = cbd_payroll_cents.get(&inv_sec).copied().unwrap_or(0) * 6
                    + 40 * CASH_SCALE;

                // B1. Declarative event pulse (`EVENT_SECTOR_INVEST_COMMERCIAL`) triggers commercial/service construction
                if let (Some(cdef), Some(ev_def), Some(ts)) = (
                    cbd_def,
                    self.events.get(crate::rules::EVENT_SECTOR_INVEST_COMMERCIAL),
                    sec_snaps.get(&inv_sec),
                ) {
                    let est_pool_cap = crate::economy::BASE_SECTOR_CAPITAL_POOL_CAP_CENTS
                        + i64::from(cbd_count) * 600 * CASH_SCALE;
                    let cbd_viable_demand = pool_cents >= est_pool_cap
                        || cdef.monthly_output.iter().all(|(gs, base_out)| {
                            self.goods.id_of(gs).map_or(true, |gid| {
                                let dem = ts.ref_demand.get(&gid).copied().unwrap_or(0);
                                dem * 2 >= *base_out
                            })
                        });
                    if sec_allow_land.get(&inv_sec).copied().unwrap_or(false)
                        && cbd_viable_demand
                        && !is_output_oversupplied(&self.goods, cdef, ts)
                        && has_viable_inputs(
                            &self.goods,
                            cdef,
                            TileSlotKey::Secondary0,
                            ts,
                            &planet_supply,
                        )
                    {
                        let cbd_cost_cents =
                            cdef.slot_build_cost(TileSlotKey::Secondary0) * CASH_SCALE;
                        let est_pool_cap = crate::economy::BASE_SECTOR_CAPITAL_POOL_CAP_CENTS
                            + i64::from(cbd_count) * 600 * CASH_SCALE;
                        let svc_px_permille = cdef
                            .monthly_output
                            .first()
                            .and_then(|(gs, _)| self.goods.id_of(gs))
                            .map(|gid| {
                                let gen = self.goods.get(gid).genesis_price_cents.max(1);
                                let cur = ts.good_ma.get(&gid).copied().unwrap_or(gen);
                                (cur * 1000) / gen
                            })
                            .unwrap_or(1_000);
                        let ctx = crate::rules::RuleContext {
                            pool_cash_cents: pool_cents,
                            pool_cap_cents: (est_pool_cap * 75) / 100,
                            build_cost_cents: cbd_cost_cents + hq_reserve_cents,
                            service_price_permille: svc_px_permille,
                            ..crate::rules::RuleContext::default()
                        };
                        let approved = ev_def
                            .choose_ai_option(&ctx)
                            .map(|idx| {
                                let mut dummy = vec![crate::market::Inventory::new(self.goods.len(), 0)];
                                let mut rep = crate::rules::EffectExecutionReport::default();
                                crate::rules::execute_rule_effects(
                                    &ev_def.options[idx].effects,
                                    &ctx,
                                    &self.goods,
                                    &mut dummy,
                                    0,
                                    0,
                                    0,
                                    0,
                                    None,
                                    &[],
                                    0,
                                    &mut rep,
                                );
                                rep.investment_approved
                            })
                            .unwrap_or(false);
                        if approved {
                            let hub_slot = pt
                                .tiles
                                .iter()
                                .filter(|t| t.sector_id == inv_sec.0 && cdef.matches_tags(t.tags))
                                .find_map(|t| {
                                    t.slots
                                        .first_compatible_empty_slot(buildings, cdef)
                                        .map(|sk| (usize::from(t.index), sk))
                                });
                            if let Some((ht, hsk)) = hub_slot {
                                pt.tiles[ht].slots.set(hsk, Some(cdef.type_id));
                                pt.set_slot_owner(
                                    ht,
                                    hsk,
                                    BuildingOwner::LocalPrivate {
                                        home_sector: inv_sec,
                                    },
                                );
                                pt.reset_slot_directives(ht, hsk);
                                pt.workshops[ht][hsk.as_index()] =
                                    make_workshop(&self.goods, &self.pop_catalog, cdef, hsk);
                                spent_sinks.push((CashSink::Capital(inv_sec), cbd_cost_cents));
                                break;
                            }
                        }
                    }
                }

                // B2. Uncapped private workshop expansion driven purely by cash, labor, and expected margin
                let mut best_project: Option<(
                    &BuildingDef,
                    BuildingOwner,
                    usize,
                    TileSlotKey,
                    i64,
                    i64,
                )> = None;

                for def in buildings
                    .defs()
                    .iter()
                    .filter(|d| d.is_privatizable_industry() && !d.tags.has(BuildingTag::Commercial))
                {
                    for t in &pt.tiles {
                        if t.sector_id == u16::MAX
                            || !def.matches_tags(t.tags)
                            || !sec_allow_land
                                .get(&SectorId(t.sector_id))
                                .copied()
                                .unwrap_or(false)
                        {
                            continue;
                        }
                        let t_sec = SectorId(t.sector_id);
                        if t_sec != inv_sec && pool_cents < self.ai_config.cross_sector_invest_min_credits * CASH_SCALE + hq_reserve_cents {
                            continue;
                        }
                        let Some(ts) = sec_snaps.get(&t_sec) else {
                            continue;
                        };
                        if is_output_oversupplied(&self.goods, def, ts) {
                            continue;
                        }
                        let (_, t_hosted, _, _) = pt.sector_cbd_hosting_stats(buildings, t_sec);
                        let owner_tag = if t_sec != inv_sec && t_hosted == 0 {
                            BuildingOwner::LocalPrivate { home_sector: t_sec }
                        } else {
                            BuildingOwner::LocalPrivate {
                                home_sector: inv_sec,
                            }
                        };
                        let has_primary = t.slots.primary.is_some();
                        for sk in t.slots.candidate_empty_slots(buildings, def).into_iter().flatten() {
                            if !has_viable_inputs(&self.goods, def, sk, ts, &planet_supply) {
                                continue;
                            }
                            let build_cost_cents = def.slot_build_cost(sk) * CASH_SCALE;
                            if pool_cents < build_cost_cents + hq_reserve_cents {
                                continue;
                            }
                            let req_jobs = def.slot_jobs_required(sk);
                            if ts.unowned_seekers < (req_jobs / 2).max(1) {
                                continue;
                            }
                            let net_margin_cents = eval_slot_margin(def, sk, 0, ts);
                            if net_margin_cents <= 0 || net_margin_cents * 20 < build_cost_cents {
                                continue;
                            }
                            let same_sec_bonus = if t_sec == inv_sec { 250i64 } else { 0 };
                            let sec_slot_bonus = if !sk.is_primary() && has_primary { 180i64 } else { 0 };
                            let score = net_margin_cents
                                + i64::from(t.tags.habitability_permille()) / 4
                                + same_sec_bonus
                                + sec_slot_bonus;
                            if best_project.as_ref().map_or(true, |b| score > b.5) {
                                best_project = Some((
                                    def,
                                    owner_tag,
                                    usize::from(t.index),
                                    sk,
                                    build_cost_cents,
                                    score,
                                ));
                            }
                        }
                    }
                }

                if let Some((def, owner_tag, target_tile, target_sk, build_cost_cents, _)) =
                    best_project
                {
                    pt.tiles[target_tile]
                        .slots
                        .set(target_sk, Some(def.type_id));
                    pt.set_slot_owner(target_tile, target_sk, owner_tag);
                    pt.reset_slot_directives(target_tile, target_sk);
                    pt.workshops[target_tile][target_sk.as_index()] =
                        make_workshop(&self.goods, &self.pop_catalog, def, target_sk);
                    spent_sinks.push((CashSink::Capital(inv_sec), build_cost_cents));
                    break;
                }
            }
            break;
        }

        for (sink, cost_cents) in spent_sinks {
            self.adjust_cash(planet_id, sink, -cost_cents);
        }

        // ── C. Subordinate Local `Polity` AI Fiscal Construction (`地方政府自主建设地方公企`) ──
        let mut local_polities: Vec<(PolityId, i64, Vec<SectorId>)> = Vec::new();
        for (_, pol) in self.world.query::<&Polity>().iter() {
            if pol.parent_polity.is_none()
                || self.controlled_actor
                    == Some(crate::economy::CounterpartyId::Polity { polity_id: pol.id })
            {
                continue;
            }
            let gov_sectors: Vec<SectorId> = self
                .world
                .query::<&Sector>()
                .iter()
                .filter_map(|(_, s)| {
                    (s.planet_id == planet_id && s.admin_polity == pol.id).then_some(s.id)
                })
                .collect();
            if !gov_sectors.is_empty() {
                local_polities.push((pol.id, pol.cash_cents(&self.goods), gov_sectors));
            }
        }
        for (lpol_id, lpol_cash, gov_sectors) in local_polities {
            let reserve = self.ai_config.local_polity_reserve_credits * CASH_SCALE;
            if lpol_cash <= reserve {
                continue;
            }
            let mut chosen: Option<(usize, String, TileSlotKey)> = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                for &sid in &gov_sectors {
                    let Some(ts) = sec_snaps.get(&sid) else {
                        continue;
                    };
                    for def in self
                        .buildings
                        .defs()
                        .iter()
                        .filter(|d| d.is_privatizable_industry() && !d.tags.has(BuildingTag::Commercial))
                    {
                        if is_output_oversupplied(&self.goods, def, ts) {
                            continue;
                        }
                        for t in pt.tiles.iter().filter(|t| t.sector_id == sid.0 && def.matches_tags(t.tags)) {
                            for sk in t.slots.candidate_empty_slots(&self.buildings, def).into_iter().flatten() {
                                if !has_viable_inputs(&self.goods, def, sk, ts, &planet_supply) {
                                    continue;
                                }
                                let cost_cents = def.slot_build_cost(sk) * CASH_SCALE;
                                if lpol_cash >= cost_cents + reserve {
                                    chosen = Some((usize::from(t.index), def.id.clone(), sk));
                                    break;
                                }
                            }
                            if chosen.is_some() {
                                break;
                            }
                        }
                        if chosen.is_some() {
                            break;
                        }
                    }
                    if chosen.is_some() {
                        break;
                    }
                }
                break;
            }
            if let Some((t_idx, b_id, sk)) = chosen {
                let _ = self.build_on_tile_as(lpol_id, planet_id, t_idx, &b_id, Some(sk));
            }
        }
    }
}
