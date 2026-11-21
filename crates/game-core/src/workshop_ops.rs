//! Tile Slots, Workshops, Ownership Transitions & Labor Directives (`地块工场、产权流转与劳动用工层`).

use crate::building::{BuildingDef, BuildingTag, BuildingTags, TileSlotKey, BUILDING_COLONIAL_OUTPOST};
use crate::economy::{
    self, BuildingOwner, Enterprise, EnterpriseId, Polity, PolityId, PopId, PopTypeId, Sector,
    SectorCapital, SectorId, SectorMarket, SectorPops, StaffContract, Workshop,
    CAMPUS_HOSTING_CAPACITY, ENTERPRISE_FOUNDING_MIN_PRIVATE,
};
use crate::market::{GoodId, Inventory, CASH_SCALE};
use crate::planet::{make_workshop, CashSink, Planet, PlanetTiles};
use crate::protocol::PlanetSnapshot;
use crate::rules::{self, RuleContext, EVENT_FORCED_EXPROPRIATION};
use crate::surface::TileTag;
use crate::Game;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet};

impl Game {
    /// Releases all `StaffContract`s previously held on `(tile_index, slot)` into each staff group's
    /// own between-jobs row `(same_role, same_culture, Unowned)`.
    pub(crate) fn release_contracts_to_between_jobs(
        &mut self,
        planet_id: &str,
        sector_id: SectorId,
        contracts: Vec<StaffContract>,
        satisfaction_penalty: u8,
    ) {
        if contracts.is_empty() {
            return;
        }
        let def_pool = self.pop_catalog.default_pool_id();
        let mut ref_ids = BTreeSet::new();
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == planet_id {
                ref_ids = pt.referenced_pop_ids_in_sector(sector_id);
                break;
            }
        }
        for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.planet_id == planet_id && sec.id == sector_id {
                for c in contracts {
                    pops.release_to_between_jobs(
                        &self.goods,
                        c.pop_id,
                        c.count,
                        satisfaction_penalty,
                    );
                }
                pops.prune_empty_groups(&self.goods, def_pool, &ref_ids);
                break;
            }
        }
    }

    /// Migrates in-force `StaffContract`s on each `(tile_index, slot)` to `new_owner`'s matching
    /// `(pop_type, culture_id, new_owner)` `PopGroup`, preserving locked `monthly_bill_cents` & household assets!
    pub(crate) fn migrate_slot_contracts_owners(
        &mut self,
        planet_id: &str,
        transitions: &[(usize, TileSlotKey, BuildingOwner)],
    ) {
        if transitions.is_empty() {
            return;
        }
        let def_pool = self.pop_catalog.default_pool_id();
        for &(tile_index, slot, new_owner) in transitions {
            let mut tile_sec_opt = None;
            let mut old_contracts = Vec::new();
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id == planet_id && tile_index < pt.tiles.len() {
                    let s_raw = pt.tiles[tile_index].sector_id;
                    if s_raw != u16::MAX {
                        tile_sec_opt = Some(SectorId(s_raw));
                        old_contracts =
                            std::mem::take(&mut pt.slot_contracts[tile_index][slot.as_index()]);
                    }
                    break;
                }
            }
            let Some(tile_sec) = tile_sec_opt else {
                continue;
            };
            if old_contracts.is_empty() {
                continue;
            }

            let mut updated_contracts: Vec<StaffContract> = Vec::with_capacity(old_contracts.len());
            for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                if sec.planet_id == planet_id && sec.id == tile_sec {
                    for c in old_contracts {
                        if c.count == 0 {
                            continue;
                        }
                        if let Some(src_idx) = pops.index_by_id(c.pop_id) {
                            let role = pops.groups[src_idx].pop_type;
                            let cid = pops.groups[src_idx].culture_id;
                            let dst_idx = pops.ensure_group_index(&self.goods, role, cid, new_owner);
                            let dst_id = pops.groups[dst_idx].id;
                            let moved = pops.transfer_between_groups(
                                &self.goods,
                                src_idx,
                                dst_idx,
                                c.count,
                            );
                            if moved > 0 {
                                let bill = if moved == c.count {
                                    c.monthly_bill_cents
                                } else {
                                    (c.monthly_bill_cents * i64::from(moved)) / i64::from(c.count)
                                };
                                if let Some(ex) =
                                    updated_contracts.iter_mut().find(|u| u.pop_id == dst_id)
                                {
                                    ex.count += moved;
                                    ex.monthly_bill_cents += bill;
                                } else {
                                    updated_contracts.push(StaffContract {
                                        pop_id: dst_id,
                                        count: moved,
                                        monthly_bill_cents: bill,
                                    });
                                }
                            }
                        }
                    }
                    break;
                }
            }

            let mut ref_ids = BTreeSet::new();
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id == planet_id {
                    pt.slot_contracts[tile_index][slot.as_index()] = updated_contracts;
                    ref_ids = pt.referenced_pop_ids_in_sector(tile_sec);
                    break;
                }
            }
            for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                if sec.planet_id == planet_id && sec.id == tile_sec {
                    pops.prune_empty_groups(&self.goods, def_pool, &ref_ids);
                    break;
                }
            }
        }
    }

    pub fn build_on_tile(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        building_id: &str,
        slot: Option<TileSlotKey>,
    ) -> Result<PlanetSnapshot, String> {
        let ruling_polity = self
            .world
            .query::<&Planet>()
            .iter()
            .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
            .ok_or_else(|| format!("找不到星球：{planet_id}"))?;
        self.build_on_tile_as(ruling_polity, planet_id, tile_index, building_id, slot)
    }

    /// Polity-authenticated construction (`国内跨级直投央企` / `地方政府公企` / `外国凭投资权直接建厂`).
    pub fn build_on_tile_as(
        &mut self,
        actor_polity: PolityId,
        planet_id: &str,
        tile_index: usize,
        building_id: &str,
        slot: Option<TileSlotKey>,
    ) -> Result<PlanetSnapshot, String> {
        let def = self.buildings.by_id(building_id)?.clone();

        let (ruling_polity, tile_sector_opt) = {
            let mut found = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id == planet_id {
                    if tile_index >= pt.tiles.len() {
                        return Err(format!("地块序号 {tile_index} 超出范围。"));
                    }
                    let sec_raw = pt.tiles[tile_index].sector_id;
                    let sec_opt = (sec_raw != u16::MAX).then_some(SectorId(sec_raw));
                    found = Some((planet.ruling_polity, sec_opt));
                    break;
                }
            }
            found.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let is_campus = def.tags.has(BuildingTag::CorporateCampus);
        let is_core_hub = def.tags.has(BuildingTag::RegionalCore);

        let Some(sid) = tile_sector_opt else {
            return Err(format!(
                "地块 #{:04} 属于尚未纳入政区直辖的无主荒野，请先消耗行政管理力框入辖区后再建造「{}」。",
                tile_index + 1,
                def.name
            ));
        };

        {
            if !self.has_sector_investment_right(actor_polity, planet_id, sid) {
                return Err("当前政权尚未获得该辖区的投资建造准入权。".into());
            }
            if def.is_state_core_or_public() {
                let sec_owner = self
                    .world
                    .query::<&Sector>()
                    .iter()
                    .find_map(|(_, s)| {
                        (s.planet_id == planet_id && s.id == sid).then_some(s.owner_polity)
                    })
                    .unwrap_or(ruling_polity);
                if !self.is_sovereign_ancestor(actor_polity, sec_owner) {
                    return Err("外国投资政权不可在东道国领土建造主权行政核心设施。".into());
                }
            }
        }

        // All amounts below are whole credits; vault balances are converted from cents.
        let treasury_avail = self.polity_cash_cents(actor_polity) / CASH_SCALE;
        let sector_pool_avail = tile_sector_opt
            .map(|sid| self.sector_pool_cents(planet_id, sid) / CASH_SCALE)
            .unwrap_or(0);

        let mut treasury_charge: i64 = 0;
        let mut private_pool_charge: i64 = 0;
        let mut compensation_sink: Option<(CashSink, i64)> = None;
        let mut liquidated: Option<(Workshop, CashSink)> = None;
        let mut replaced_contracts: Vec<StaffContract> = Vec::new();
        let mut owner_transitions: Vec<(usize, TileSlotKey, BuildingOwner)> = Vec::new();

        let buildings = &self.buildings;
        for (_, (planet, planet_tiles)) in self
            .world
            .query_mut::<(&Planet, &mut PlanetTiles)>()
        {
            if planet.id != planet_id {
                continue;
            }
            let tile_tags = planet_tiles.tiles[tile_index].tags;
            if !def.matches_tags(tile_tags) {
                return Err(format!(
                    "地块 #{:04} 的地形标签不满足「{}」的建造要求。",
                    tile_index + 1,
                    def.name
                ));
            }

            let target_slot = match slot {
                Some(explicit) => {
                    if !def.can_use_slot(explicit) {
                        return Err(format!(
                            "「{}」不能放入{}。",
                            def.name,
                            explicit.label()
                        ));
                    }
                    if let Some(existing_tid) = planet_tiles.tiles[tile_index].slots.get(explicit) {
                        if !explicit.is_primary() {
                            return Err(format!(
                                "地块 #{:04} 的{}已有建筑，请先拆除后再建。",
                                tile_index + 1,
                                explicit.label()
                            ));
                        }
                        let existing_def = buildings.get(existing_tid);
                        if existing_tid == def.type_id {
                            return Err(format!(
                                "地块 #{:04} 的主建筑槽已是「{}」。",
                                tile_index + 1,
                                def.name
                            ));
                        }
                        let can_upgrade_outpost = existing_def.id == BUILDING_COLONIAL_OUTPOST
                            && def.tags.has(BuildingTag::RegionalCore);
                        if (existing_def.is_state_core_or_public() && !can_upgrade_outpost)
                            || existing_def.tags.has(BuildingTag::CorporateCampus)
                        {
                            return Err(format!(
                                "地块 #{:04} 的主建筑槽为「{}」，不可直接覆盖改建。",
                                tile_index + 1,
                                existing_def.name
                            ));
                        }
                        if let BuildingOwner::State { polity_id } =
                            planet_tiles.slot_owner(tile_index, explicit)
                        {
                            if polity_id != actor_polity {
                                return Err(
                                    "一级政府一级产权：不可直接覆盖其他政权出资持有的国有槽位。"
                                        .into(),
                                );
                            }
                        }
                        for &sec_tid in planet_tiles.tiles[tile_index].slots.secondary.iter().flatten() {
                            let sec_def = buildings.get(sec_tid);
                            if sec_def.required_primary_tags != BuildingTags::EMPTY
                                && !def.tags.intersects(sec_def.required_primary_tags)
                            {
                                return Err(format!(
                                    "次建筑槽中的「{}」需依托满足前置条件的主槽设施运行，请先拆除后再改建主槽。",
                                    sec_def.name
                                ));
                            }
                        }
                    }
                    explicit
                }
                None => planet_tiles.tiles[tile_index]
                    .slots
                    .first_compatible_empty_slot(buildings, &def)
                    .ok_or_else(|| {
                        if !def.matches_primary_slot(
                            buildings,
                            &planet_tiles.tiles[tile_index].slots,
                            TileSlotKey::Secondary0,
                        ) {
                            let req_names: Vec<&str> = def
                                .required_primary_tags
                                .iter()
                                .map(|t| t.display_name())
                                .collect();
                            format!(
                                "「{}」受设定约束，要求同地块主建筑槽建有：{}。",
                                def.name,
                                req_names.join(" / ")
                            )
                        } else {
                            format!(
                                "地块 #{:04} 已无适配「{}」的空置建筑槽。",
                                tile_index + 1,
                                def.name
                            )
                        }
                    })?,
            };

            if !def.matches_primary_slot(
                buildings,
                &planet_tiles.tiles[tile_index].slots,
                target_slot,
            ) {
                let req_names: Vec<&str> = def
                    .required_primary_tags
                    .iter()
                    .map(|t| t.display_name())
                    .collect();
                return Err(format!(
                    "「{}」受设定约束，要求同地块主建筑槽建有：{}。",
                    def.name,
                    req_names.join(" / ")
                ));
            }

            let replacing_slot = planet_tiles.tiles[tile_index]
                .slots
                .get(target_slot)
                .map(|_| target_slot);
            planet_tiles.check_uniqueness(buildings, tile_index, replacing_slot, &def)?;

            let campus_industry_tag = if is_campus {
                let sid = tile_sector_opt.unwrap();
                let total_dom =
                    planet_tiles.dominant_private_industry_in_sector(buildings, sid, None);
                let rem_dom = planet_tiles.dominant_private_industry_in_sector(
                    buildings,
                    sid,
                    replacing_slot.map(|s| (tile_index, s)),
                );
                let (dom_tag, total_cnt) = total_dom.unwrap_or((BuildingTag::Agriculture, 0));
                let rem_cnt = rem_dom
                    .filter(|(t, _)| *t == dom_tag)
                    .map(|(_, c)| c)
                    .unwrap_or(0);
                if usize::from(total_cnt) < ENTERPRISE_FOUNDING_MIN_PRIVATE
                    || usize::from(rem_cnt) < 2
                {
                    return Err(format!(
                        "本辖区需先积累至少 {} 座同行业个体私产（当前 {} 座），方可批地组建「企业园区」。",
                        ENTERPRISE_FOUNDING_MIN_PRIVATE, total_cnt
                    ));
                }
                Some(dom_tag)
            } else {
                None
            };

            let cost = def.slot_build_cost(target_slot);
            if is_campus {
                let use_priv = sector_pool_avail.max(0).min(cost);
                let rem_treasury = cost - use_priv;
                if treasury_avail < rem_treasury {
                    return Err(format!(
                        "资金不足：批建「企业园区」需 {cost} 资金（当前本区民间投资池 {sector_pool_avail} + 国库 {treasury_avail}）。"
                    ));
                }
                private_pool_charge = use_priv;
                treasury_charge = rem_treasury;
            } else {
                let buyout = if let Some(rep_s) = replacing_slot {
                    match planet_tiles.slot_owner(tile_index, rep_s) {
                        BuildingOwner::LocalPrivate { home_sector } => {
                            compensation_sink = Some((CashSink::Capital(home_sector), 150));
                            150
                        }
                        BuildingOwner::Enterprise { enterprise_id } => {
                            compensation_sink = Some((CashSink::Enterprise(enterprise_id), 150));
                            150
                        }
                        BuildingOwner::Trust { trust_id } => {
                            compensation_sink = Some((CashSink::Trust(trust_id), 150));
                            150
                        }
                        _ => 0,
                    }
                } else {
                    0
                };
                if treasury_avail < cost + buyout {
                    return Err(format!(
                        "政权金库资金不足：建造「{}」需 {} 资金（当前可用 {}）。",
                        def.name,
                        cost + buyout,
                        treasury_avail
                    ));
                }
                treasury_charge = cost + buyout;
            }

            // Replacing a productive building liquidates its warehouse (cash returns to old owner)
            // and releases its contracted staff into `失业者`.
            replaced_contracts =
                std::mem::take(&mut planet_tiles.slot_contracts[tile_index][target_slot.as_index()]);
            if let Some(old_ws) =
                planet_tiles.workshops[tile_index][target_slot.as_index()].take()
            {
                let fallback = tile_sector_opt.unwrap_or(SectorId(0));
                let sink = planet_tiles.owner_cash_sink(
                    planet_tiles.slot_owner(tile_index, target_slot),
                    ruling_polity,
                    fallback,
                );
                liquidated = Some((old_ws, sink));
            }
            planet_tiles.tiles[tile_index]
                .slots
                .set(target_slot, Some(def.type_id));
            planet_tiles.reset_slot_directives(tile_index, target_slot);
            planet_tiles.workshops[tile_index][target_slot.as_index()] =
                make_workshop(&self.goods, &self.pop_catalog, &def, target_slot);

            if let Some(dom_tag) = campus_industry_tag {
                let sid = tile_sector_opt.unwrap();
                let ent_id = EnterpriseId(planet_tiles.enterprises.len() as u16);
                let reg_short = planet_tiles
                    .regions
                    .get(usize::from(planet_tiles.tiles[tile_index].region_id))
                    .and_then(|r| r.name.split(" · ").nth(1))
                    .unwrap_or("泰拉");
                let seq = planet_tiles
                    .enterprises
                    .iter()
                    .filter(|e| e.hq_sector == sid)
                    .count()
                    + 1;
                let ind_label = if dom_tag == BuildingTag::Agriculture {
                    "农垦"
                } else {
                    dom_tag.display_name()
                };
                let ent_name = if seq == 1 {
                    format!("{reg_short}{ind_label}集团")
                } else {
                    format!("{reg_short}{ind_label}集团·{seq}号")
                };

                let campus_seed_cents =
                    def.slot_monthly_wage_bill_cents(&self.pop_catalog, target_slot) * 2;
                planet_tiles.enterprises.push(Enterprise::new(
                    &self.goods,
                    ent_id,
                    ent_name,
                    dom_tag.as_str().to_string(),
                    sid,
                    tile_index,
                    CAMPUS_HOSTING_CAPACITY,
                    campus_seed_cents,
                ));
                let ent_owner = BuildingOwner::Enterprise {
                    enterprise_id: ent_id,
                };
                planet_tiles.set_slot_owner(tile_index, target_slot, ent_owner);

                let mut absorbed = 0u16;
                for t_i in 0..planet_tiles.tiles.len() {
                    for sk in TileSlotKey::ALL {
                        if absorbed >= CAMPUS_HOSTING_CAPACITY {
                            break;
                        }
                        if let Some(tid) = planet_tiles.tiles[t_i].slots.get(sk) {
                            let bdef = buildings.get(tid);
                            if bdef.tags.has(dom_tag)
                                && !bdef.tags.has(BuildingTag::Commercial)
                                && matches!(
                                    planet_tiles.slot_owner(t_i, sk),
                                    BuildingOwner::LocalPrivate { home_sector } if home_sector == sid
                                )
                            {
                                planet_tiles.set_slot_owner(t_i, sk, ent_owner);
                                owner_transitions.push((t_i, sk, ent_owner));
                                absorbed += 1;
                            }
                        }
                    }
                }
            } else {
                planet_tiles.set_slot_owner(
                    tile_index,
                    target_slot,
                    BuildingOwner::State {
                        polity_id: actor_polity,
                    },
                );
            }
            break;
        }

        // ponytail: construction spending leaves the economy (acts as a money sink against minting);
        // a construction-materials market can replace this later.
        if treasury_charge != 0 {
            self.adjust_cash(
                planet_id,
                CashSink::Polity(actor_polity),
                -treasury_charge * CASH_SCALE,
            );
        }
        if private_pool_charge != 0 {
            if let Some(sid) = tile_sector_opt {
                self.adjust_cash(planet_id, CashSink::Capital(sid), -private_pool_charge * CASH_SCALE);
            }
        }
        if let Some((sink, amount)) = compensation_sink {
            self.adjust_cash(planet_id, sink, amount * CASH_SCALE);
        }
        if let Some((ws, sink)) = liquidated {
            let cash = ws.inv.total(self.goods.cash_id());
            self.adjust_cash(planet_id, sink, cash);
        }
        if !replaced_contracts.is_empty() {
            if let Some(sid) = tile_sector_opt {
                self.release_contracts_to_between_jobs(planet_id, sid, replaced_contracts, 5);
            }
        }
        if !owner_transitions.is_empty() {
            self.migrate_slot_contracts_owners(planet_id, &owner_transitions);
        }

        let mut constructed_slot = None;
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == planet_id && tile_index < pt.tiles.len() {
                for sk in TileSlotKey::ALL {
                    if pt.tiles[tile_index].slots.get(sk) == Some(def.type_id)
                        && pt.slot_contracts[tile_index][sk.as_index()].is_empty()
                    {
                        constructed_slot = Some(sk);
                        break;
                    }
                }
                break;
            }
        }

        // When founding a `殖民据点` / `大区核心` in a sparsely populated or newly opened frontier `Sector`,
        // conserve-migrate a pioneer team from homologous sectors and immediately staff the core hub!
        if def.is_state_core_or_public() {
            if let Some(target_sk) = constructed_slot {
                let req_jobs = def.slot_jobs_required(target_sk);
                if is_core_hub && req_jobs > 0 {
                    self.migrate_pioneers_if_frontier(
                        actor_polity,
                        planet_id,
                        sid,
                        req_jobs.saturating_add(50),
                    );
                }
                self.staff_state_slot_immediately(
                    actor_polity,
                    planet_id,
                    sid,
                    tile_index,
                    target_sk,
                    &def,
                );
            }
        }

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn build_in_region(
        &mut self,
        planet_id: &str,
        region_id: u16,
        building_id: &str,
    ) -> Result<PlanetSnapshot, String> {
        let def = self.buildings.by_id(building_id)?.clone();
        let buildings = &self.buildings;
        let mut chosen_target: Option<(usize, TileSlotKey)> = None;

        for (_, (planet, planet_tiles)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id != planet_id {
                continue;
            }
            if usize::from(region_id) >= planet_tiles.regions.len() {
                return Err(format!("无效的大区编号：{region_id}"));
            }
            if let Some(any_in_region) = planet_tiles.tiles.iter().find(|t| t.region_id == region_id)
            {
                if def.unique_planet_tags != BuildingTags::EMPTY
                    || def.unique_region_tags != BuildingTags::EMPTY
                {
                    planet_tiles.check_uniqueness(
                        buildings,
                        usize::from(any_in_region.index),
                        None,
                        &def,
                    )?;
                }
            }
            let actor_polity = match self.controlled_actor {
                Some(economy::CounterpartyId::Polity { polity_id }) => polity_id,
                _ => planet.ruling_polity,
            };
            let treasury_avail = self.polity_cash_cents(actor_polity) / CASH_SCALE;
            let is_ocean_region = planet_tiles.regions[usize::from(region_id)].is_ocean;

            let def_ref = &def;
            let best = planet_tiles
                .tiles
                .iter()
                .filter(|t| {
                    t.region_id == region_id
                        && t.sector_id != u16::MAX
                        && self.has_sector_investment_right(
                            actor_polity,
                            planet_id,
                            SectorId(t.sector_id),
                        )
                        && def_ref.matches_tags(t.tags)
                        && planet_tiles
                            .check_uniqueness(buildings, usize::from(t.index), None, def_ref)
                            .is_ok()
                })
                .flat_map(|t| {
                    let has_primary = t.slots.primary.is_some();
                    t.slots
                        .candidate_empty_slots(buildings, def_ref)
                        .into_iter()
                        .flatten()
                        .map(move |slot| {
                            let affordable_bonus = if treasury_avail >= def_ref.slot_build_cost(slot) {
                                200.0
                            } else {
                                0.0
                            };
                            let tier_bonus = if !slot.is_primary() && has_primary {
                                50.0
                            } else if slot.is_primary() {
                                8.0
                            } else {
                                0.0
                            };
                            let domain_bonus = if !is_ocean_region && t.tags.has(TileTag::Land) {
                                15.0
                            } else {
                                0.0
                            };
                            let score =
                                affordable_bonus + tier_bonus + domain_bonus + t.tags.habitability();
                            (usize::from(t.index), slot, score)
                        })
                })
                .max_by(|a, b| {
                    a.2.partial_cmp(&b.2)
                        .unwrap_or(CmpOrdering::Equal)
                        .then_with(|| b.0.cmp(&a.0))
                });

            chosen_target = best.map(|(idx, slot, _)| (idx, slot));
            break;
        }

        let Some((tile_index, slot)) = chosen_target else {
            return Err(format!(
                "该大区已无满足「{}」建造条件与槽位的空置地块。",
                def.name
            ));
        };
        self.build_on_tile(planet_id, tile_index, building_id, Some(slot))
    }

    pub fn demolish_on_tile(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let mut found = false;
        let mut demolished_liquidated: Option<(Workshop, CashSink)> = None;
        let mut demolished_contracts: Option<(SectorId, Vec<StaffContract>)> = None;
        let buildings = &self.buildings;
        for (_, (planet, planet_tiles)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            found = true;
            if tile_index >= planet_tiles.tiles.len() {
                return Err(format!("地块序号 {tile_index} 超出范围。"));
            }
            let Some(tid) = planet_tiles.tiles[tile_index].slots.get(slot) else {
                return Err("该建筑槽当前为空，无需拆除。".to_string());
            };
            let def = buildings.get(tid);
            if def.tags.has(BuildingTag::PlanetaryCapital) {
                return Err("「行星首府」不可直接拆除。".to_string());
            }
            if slot.is_primary() {
                for &sec_tid in planet_tiles.tiles[tile_index].slots.secondary.iter().flatten() {
                    let sec_def = buildings.get(sec_tid);
                    if sec_def.required_primary_tags != BuildingTags::EMPTY {
                        return Err(format!(
                            "次建筑槽中的「{}」需依托当前主槽设施运行：请先拆除相关次槽建筑。",
                            sec_def.name
                        ));
                    }
                }
            }
            if def.tags.has(BuildingTag::CorporateCampus) {
                if let BuildingOwner::Enterprise { enterprise_id } =
                    planet_tiles.slot_owner(tile_index, slot)
                {
                    let hosted = planet_tiles.enterprise_hosted_count(buildings, enterprise_id);
                    if hosted > 0 {
                        return Err(format!(
                            "该企业园区名下仍托管有 {hosted} 座企业建筑，请先赎买国有化后再拆除。"
                        ));
                    }
                }
            }

            let sec_raw = planet_tiles.tiles[tile_index].sector_id;
            if sec_raw != u16::MAX {
                let c = std::mem::take(&mut planet_tiles.slot_contracts[tile_index][slot.as_index()]);
                if !c.is_empty() {
                    demolished_contracts = Some((SectorId(sec_raw), c));
                }
            }
            if let Some(old_ws) = planet_tiles.workshops[tile_index][slot.as_index()].take() {
                let fallback = SectorId(if sec_raw != u16::MAX { sec_raw } else { 0 });
                let sink = planet_tiles.owner_cash_sink(
                    planet_tiles.slot_owner(tile_index, slot),
                    planet.ruling_polity,
                    fallback,
                );
                demolished_liquidated = Some((old_ws, sink));
            }
            planet_tiles.tiles[tile_index].slots.set(slot, None);
            planet_tiles.set_slot_owner(tile_index, slot, BuildingOwner::Unowned);
            planet_tiles.reset_slot_directives(tile_index, slot);
            break;
        }
        if !found {
            return Err(format!("找不到星球：{planet_id}"));
        }
        if let Some((ws, sink)) = demolished_liquidated {
            let cash = ws.inv.total(self.goods.cash_id());
            self.adjust_cash(planet_id, sink, cash);
        }
        if let Some((sid, contracts)) = demolished_contracts {
            self.release_contracts_to_between_jobs(planet_id, sid, contracts, 6);
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn privatize_slot(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (seller_polity, local_sector_opt, price, def_tags) = {
            let mut info = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该槽位当前没有建筑。".to_string());
                };
                let def = self.buildings.get(tid);
                if !def.is_privatizable_industry() {
                    return Err(format!("「{}」属于公共或产权中枢，不可挂牌私有化。", def.name));
                }
                let BuildingOwner::State { polity_id } = pt.slot_owner(tile_index, slot) else {
                    return Err("该建筑已属民间或企业产权，无需重复私有化。".to_string());
                };
                let sale_price = (def.slot_build_cost(slot) * 80) / 100;
                let sec_raw = pt.tiles[tile_index].sector_id;
                let sec_opt = (sec_raw != u16::MAX).then_some(SectorId(sec_raw));
                info = Some((polity_id, sec_opt, sale_price, def.tags));
                break;
            }
            info.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let mut pools: BTreeMap<SectorId, i64> = BTreeMap::new();
        for (_, (sec, cap)) in self.world.query::<(&Sector, &SectorCapital)>().iter() {
            if sec.planet_id == planet_id {
                pools.insert(sec.id, cap.pool_cents(&self.goods) / CASH_SCALE);
            }
        }

        let mut chosen_owner: Option<(BuildingOwner, CashSink, i64)> = None;

        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id != planet_id {
                continue;
            }
            for ent in &pt.enterprises {
                if def_tags.iter().any(|t| t.as_str() == ent.industry_tag) {
                    let hosted = pt.enterprise_hosted_count(&self.buildings, ent.id);
                    let ent_pool = ent.cash_cents(&self.goods) / CASH_SCALE;
                    if hosted < ent.hosting_capacity && ent_pool >= price {
                        chosen_owner = Some((
                            BuildingOwner::Enterprise {
                                enterprise_id: ent.id,
                            },
                            CashSink::Enterprise(ent.id),
                            price,
                        ));
                        break;
                    }
                }
            }
            if chosen_owner.is_some() {
                break;
            }

            let mut candidate_sectors: Vec<(SectorId, i64)> =
                pools.iter().map(|(&k, &v)| (k, v)).collect();
            candidate_sectors.sort_by(|a, b| {
                let a_local = Some(a.0) == local_sector_opt;
                let b_local = Some(b.0) == local_sector_opt;
                b_local
                    .cmp(&a_local)
                    .then_with(|| b.1.cmp(&a.1))
                    .then_with(|| a.0.cmp(&b.0))
            });

            for (cand_sec, pool) in candidate_sectors {
                if pool >= price {
                    chosen_owner = Some((
                        BuildingOwner::LocalPrivate {
                            home_sector: cand_sec,
                        },
                        CashSink::Capital(cand_sec),
                        price,
                    ));
                    break;
                }
            }
            break;
        }

        let Some((new_owner, buyer_sink, charge_credits)) = chosen_owner else {
            return Err(format!(
                "挂牌私有化未成交：需买方金库至少 {price} 资金。"
            ));
        };

        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            pt.set_slot_owner(tile_index, slot, new_owner);
            break;
        }

        self.migrate_slot_contracts_owners(planet_id, &[(tile_index, slot, new_owner)]);
        self.adjust_cash(planet_id, buyer_sink, -charge_credits * CASH_SCALE);
        self.adjust_cash(planet_id, CashSink::Polity(seller_polity), price * CASH_SCALE);

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn nationalize_slot(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (ruling_polity, prev_owner_sink, prev_owner_sector, buyout_cost) = {
            let mut info = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该槽位当前没有建筑。".to_string());
                };
                let def = self.buildings.get(tid);
                if !def.is_privatizable_industry() {
                    return Err(format!("「{}」不支持赎买国有化操作。", def.name));
                }
                let tile_sec = SectorId(if pt.tiles[tile_index].sector_id != u16::MAX {
                    pt.tiles[tile_index].sector_id
                } else {
                    0
                });
                let (sink, owner_sec) = match pt.slot_owner(tile_index, slot) {
                    BuildingOwner::LocalPrivate { home_sector } => {
                        (CashSink::Capital(home_sector), home_sector)
                    }
                    BuildingOwner::Enterprise { enterprise_id } => (
                        CashSink::Enterprise(enterprise_id),
                        pt.enterprises
                            .get(usize::from(enterprise_id.0))
                            .map(|e| e.hq_sector)
                            .unwrap_or(tile_sec),
                    ),
                    BuildingOwner::Trust { trust_id } => (CashSink::Trust(trust_id), tile_sec),
                    BuildingOwner::State { polity_id } if polity_id != planet.ruling_polity => {
                        (CashSink::Polity(polity_id), tile_sec)
                    }
                    _ => return Err("该建筑已是本国国有产权。".to_string()),
                };
                info = Some((
                    planet.ruling_polity,
                    sink,
                    owner_sec,
                    def.slot_build_cost(slot),
                ));
                break;
            }
            info.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let treasury_avail = self.polity_cash_cents(ruling_polity) / CASH_SCALE;
        if treasury_avail < buyout_cost {
            return Err(format!(
                "国库资金不足：赎买国有化需 {buyout_cost} 资金（当前国库 {treasury_avail}）。"
            ));
        }

        let state_owner = BuildingOwner::State {
            polity_id: ruling_polity,
        };
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                pt.set_slot_owner(tile_index, slot, state_owner);
                break;
            }
        }
        self.migrate_slot_contracts_owners(planet_id, &[(tile_index, slot, state_owner)]);
        self.adjust_cash(planet_id, CashSink::Polity(ruling_polity), -buyout_cost * CASH_SCALE);
        self.adjust_cash(planet_id, prev_owner_sink, buyout_cost / 2 * CASH_SCALE);
        let div_recipients = self.pop_catalog.dividend_recipients();
        for (_, (sec, cap, pops)) in
            self.world
                .query_mut::<(&Sector, &mut SectorCapital, &mut SectorPops)>()
        {
            if sec.planet_id == planet_id && sec.id == prev_owner_sector {
                let _ = cap;
                for g in &mut pops.groups {
                    if div_recipients.contains(&g.pop_type) {
                        g.satisfaction = g.satisfaction.saturating_sub(6);
                    }
                }
                break;
            }
        }

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    /// Sovereign forced nationalization without buyout payment (`强制无偿没收国有化`).
    /// Seizes foreign `Polity` / `Enterprise` / `Trust` / `LocalPrivate` slot ownership into `host_polity`
    /// and executes declarative `EVENT_FORCED_EXPROPRIATION` (applying credit-rating & diplomatic consequences).
    pub fn expropriate_slot(
        &mut self,
        host_polity: PolityId,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let tile_sec = {
            let mut found = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该槽位当前没有建筑。".to_string());
                };
                let def = self.buildings.get(tid);
                if def.is_state_core_or_public() {
                    return Err(format!("「{}」不可执行没收操作。", def.name));
                }
                let cur_owner = pt.slot_owner(tile_index, slot);
                if let BuildingOwner::State { polity_id } = cur_owner {
                    if self.is_sovereign_ancestor(host_polity, polity_id)
                        || self.is_sovereign_ancestor(polity_id, host_polity)
                    {
                        return Err("不可对本国主权体系内的公有产权执行涉外强制没收。".into());
                    }
                } else if cur_owner == BuildingOwner::Unowned {
                    return Err("该槽位无有效产权主体。".into());
                }
                let s_raw = pt.tiles[tile_index].sector_id;
                if s_raw == u16::MAX {
                    return Err("该地块未归属任何实控辖区。".into());
                }
                found = Some(SectorId(s_raw));
                break;
            }
            found.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let sec_owner = self
            .world
            .query::<&Sector>()
            .iter()
            .find_map(|(_, s)| (s.planet_id == planet_id && s.id == tile_sec).then_some(s.owner_polity))
            .ok_or_else(|| format!("找不到辖区 #{}", tile_sec.0))?;
        if !self.is_sovereign_ancestor(host_polity, sec_owner) {
            return Err("仅东道主权政权可对境内资产执行强制没收。".into());
        }

        let new_owner = BuildingOwner::State {
            polity_id: host_polity,
        };
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                pt.set_slot_owner(tile_index, slot, new_owner);
                break;
            }
        }
        self.migrate_slot_contracts_owners(planet_id, &[(tile_index, slot, new_owner)]);

        if let Some(ev_def) = self.events.get(EVENT_FORCED_EXPROPRIATION).cloned() {
            let cur_rating = self
                .world
                .query::<&Polity>()
                .iter()
                .find_map(|(_, p)| (p.id == host_polity).then_some(p.credit_rating))
                .unwrap_or(100);
            let ctx = RuleContext {
                seller_honors: false,
                buyer_honors: true,
                seller_credit_rating: cur_rating,
                buyer_credit_rating: 100,
                ..RuleContext::default()
            };
            if let Some(opt_idx) = ev_def.choose_ai_option(&ctx) {
                let mut dummy_invs = vec![Inventory::new(self.goods.len(), 0)];
                let mut rep = rules::EffectExecutionReport::default();
                rules::execute_rule_effects(
                    &ev_def.options[opt_idx].effects,
                    &ctx,
                    &self.goods,
                    &mut dummy_invs,
                    0,
                    0,
                    0,
                    0,
                    None,
                    &[],
                    0,
                    &mut rep,
                );
                if rep.seller_rating_delta != 0 {
                    for (_, pol) in self.world.query_mut::<&mut Polity>() {
                        if pol.id == host_polity {
                            pol.credit_rating =
                                (pol.credit_rating + rep.seller_rating_delta).clamp(0, 100);
                            break;
                        }
                    }
                }
            }
        }

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    /// Acquires a distressed/bankrupt `LocalPrivate` workshop on behalf of `buyer_polity` (host or
    /// subordinate local government) at its `25%` salvage teardown value, settles unpaid worker wage
    /// claims, and injects 1-month base working capital.
    pub fn acquire_bankrupt_slot_as(
        &mut self,
        buyer_polity: PolityId,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let cash = self.goods.cash_id();
        let c_idx = usize::from(cash.0);
        let (tile_sec, home_sector, salvage_cents, base_reserve_cents, unpaid_list, sec_admin_pol) = {
            let mut found = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该槽位当前没有建筑。".into());
                };
                let BuildingOwner::LocalPrivate { home_sector } = pt.slot_owner(tile_index, slot)
                else {
                    return Err("仅本地私营（LocalPrivate）破产工场可执行破产残值收购。".into());
                };
                let Some(ws) = pt.workshops[tile_index][slot.as_index()].as_ref() else {
                    return Err("该槽位无独立工场账本，不可执行破产收购。".into());
                };
                let has_pending = self.pending_events.iter().any(|pe| {
                    matches!(
                        pe.payload,
                        crate::rules::PendingEventPayload::BankruptSlot {
                            planet_id: ref p,
                            tile_index: ti,
                            slot: sk,
                            ..
                        } if p == planet_id && ti == tile_index && sk == slot
                    )
                });
                let in_distress =
                    ws.consecutive_arrears_months >= 2 && ws.total_unpaid_claims_cents() > 0;
                if !in_distress && !has_pending {
                    return Err("该私营工场当前未进入破产清算期，请使用常规赎买国有化。".into());
                }
                let s_raw = pt.tiles[tile_index].sector_id;
                if s_raw == u16::MAX {
                    return Err("该地块未归属任何实控辖区。".into());
                }
                let def = self.buildings.get(tid);
                let build_cents = def.slot_build_cost(slot) * CASH_SCALE;
                let salvage = (build_cents * 25) / 100;
                let cur_bill: i64 = pt.slot_contracts[tile_index][slot.as_index()]
                    .iter()
                    .map(|c| c.monthly_bill_cents)
                    .sum();
                let base_res = cur_bill.max(20 * CASH_SCALE);
                let claims: Vec<(crate::economy::PopId, i64)> = ws
                    .unpaid_claims
                    .iter()
                    .filter(|cl| cl.owed_cents > 0)
                    .map(|cl| (cl.pop_id, cl.owed_cents))
                    .collect();
                found = Some((SectorId(s_raw), home_sector, salvage, base_res, claims));
                break;
            }
            let (t_sec, h_sec, salv, b_res, cl) =
                found.ok_or_else(|| format!("找不到星球：{planet_id}"))?;
            let (admin_pol, owner_pol) = self
                .world
                .query::<&Sector>()
                .iter()
                .find_map(|(_, s)| {
                    (s.planet_id == planet_id && s.id == t_sec)
                        .then_some((s.admin_polity, s.owner_polity))
                })
                .ok_or_else(|| format!("找不到辖区 #{}", t_sec.0))?;
            if buyer_polity != admin_pol
                && buyer_polity != owner_pol
                && !self.is_sovereign_ancestor(buyer_polity, owner_pol)
            {
                return Err("仅本辖区管辖政府或上级主权政权可收购辖区内破产工场。".into());
            }
            (t_sec, h_sec, salv, b_res, cl, admin_pol)
        };

        let rem_unpaid: i64 = unpaid_list.iter().map(|&(_, amt)| amt).sum();
        let total_needed = salvage_cents + rem_unpaid + base_reserve_cents;
        let pol_avail = self.polity_cash_cents(buyer_polity);
        if pol_avail < total_needed {
            return Err(format!(
                "政府金库资金不足：破产收购加清偿欠薪与启动周转共需 {} 资金（当前可用 {}）。",
                total_needed / CASH_SCALE,
                pol_avail / CASH_SCALE
            ));
        }

        let tax_rate_permille = self
            .world
            .query::<&Polity>()
            .iter()
            .find_map(|(_, p)| (p.id == sec_admin_pol).then_some(i64::from(p.tax_rate_permille)))
            .unwrap_or(150);

        // 1) Pay salvage value to `home_sector` capital pool
        self.adjust_cash(planet_id, CashSink::Polity(buyer_polity), -salvage_cents);
        self.adjust_cash(planet_id, CashSink::Capital(home_sector), salvage_cents);

        // 2) Settle unpaid worker claims (with source tax routed to `sec_admin_pol`)
        let mut total_tax = 0i64;
        for &(pop_id, owed) in &unpaid_list {
            if owed <= 0 {
                continue;
            }
            let t_tax = owed * tax_rate_permille / 1000;
            let net_pay = owed - t_tax;
            let mut paid_ok = false;
            for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                if sec.planet_id == planet_id && sec.id == tile_sec {
                    if let Some(gi) = pops.index_by_id(pop_id) {
                        pops.groups[gi].inv.ensure_len(self.goods.len());
                        pops.groups[gi].inv.available[c_idx] += net_pay;
                        paid_ok = true;
                    }
                    break;
                }
            }
            if paid_ok {
                self.adjust_cash(planet_id, CashSink::Polity(buyer_polity), -owed);
                total_tax += t_tax;
            }
        }
        if total_tax > 0 {
            self.adjust_cash(planet_id, CashSink::Polity(sec_admin_pol), total_tax);
        }

        // 3) Inject base working capital & transfer ownership + contracts to `buyer_polity`
        self.adjust_cash(
            planet_id,
            CashSink::Polity(buyer_polity),
            -base_reserve_cents,
        );
        let new_owner = BuildingOwner::State {
            polity_id: buyer_polity,
        };
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                if let Some(ws) = pt.workshops[tile_index][slot.as_index()].as_mut() {
                    ws.inv.ensure_len(self.goods.len());
                    ws.inv.available[c_idx] += base_reserve_cents;
                    ws.unpaid_claims.clear();
                    ws.had_wage_arrears = false;
                    ws.consecutive_arrears_months = 0;
                }
                pt.set_slot_owner(tile_index, slot, new_owner);
                pt.reset_slot_directives(tile_index, slot);
                break;
            }
        }
        self.migrate_slot_contracts_owners(planet_id, &[(tile_index, slot, new_owner)]);
        self.pending_events.retain(|pe| {
            !matches!(
                pe.payload,
                crate::rules::PendingEventPayload::BankruptSlot {
                    planet_id: ref p,
                    tile_index: ti,
                    slot: sk,
                    ..
                } if p == planet_id && ti == tile_index && sk == slot
            )
        });

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    /// Acquires a bankrupt `LocalPrivate` slot using the local sector's administering polity (`sec.admin_polity`).
    pub fn acquire_bankrupt_slot(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
        buyer_polity: Option<PolityId>,
    ) -> Result<PlanetSnapshot, String> {
        let pol = if let Some(pid) = buyer_polity {
            pid
        } else {
            let s_raw = self
                .world
                .query::<(&Planet, &PlanetTiles)>()
                .iter()
                .find_map(|(_, (p, pt))| {
                    (p.id == planet_id)
                        .then(|| pt.tiles.get(tile_index).map(|t| t.sector_id))
                        .flatten()
                })
                .ok_or_else(|| format!("找不到星球或地块：{planet_id} #{tile_index}"))?;
            self.world
                .query::<&Sector>()
                .iter()
                .find_map(|(_, s)| {
                    (s.planet_id == planet_id && s.id == SectorId(s_raw)).then_some(s.admin_polity)
                })
                .ok_or_else(|| "该地块未归属有效辖区。".to_string())?
        };
        self.acquire_bankrupt_slot_as(pol, planet_id, tile_index, slot)
    }

    /// Cleanly tears down a bankrupt `LocalPrivate` slot when government acquisition is declined.
    pub(crate) fn demolish_bankrupt_slot(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        let cash = self.goods.cash_id();
        let mut released: Vec<(SectorId, Vec<crate::economy::StaffContract>)> = Vec::new();
        let mut cash_to_cap: Option<(SectorId, i64)> = None;
        let buildings = &self.buildings;

        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id || tile_index >= pt.tiles.len() {
                continue;
            }
            let s_raw = pt.tiles[tile_index].sector_id;
            let t_sec = SectorId(if s_raw != u16::MAX { s_raw } else { 0 });
            let home_sec = match pt.slot_owner(tile_index, slot) {
                BuildingOwner::LocalPrivate { home_sector } => home_sector,
                _ => t_sec,
            };
            let mut swept = 0i64;
            if let Some(ws) = pt.workshops[tile_index][slot.as_index()].take() {
                swept += ws.inv.total(cash);
            }
            let rel = std::mem::take(&mut pt.slot_contracts[tile_index][slot.as_index()]);
            if !rel.is_empty() && s_raw != u16::MAX {
                released.push((t_sec, rel));
            }
            pt.tiles[tile_index].slots.set(slot, None);
            pt.set_slot_owner(tile_index, slot, BuildingOwner::Unowned);
            pt.reset_slot_directives(tile_index, slot);

            if slot.is_primary() {
                for sec_sk in [
                    TileSlotKey::Secondary0,
                    TileSlotKey::Secondary1,
                    TileSlotKey::Secondary2,
                ] {
                    let dep = pt.tiles[tile_index]
                        .slots
                        .get(sec_sk)
                        .map(|stid| {
                            buildings.get(stid).required_primary_tags != BuildingTags::EMPTY
                        })
                        .unwrap_or(false);
                    if dep {
                        if let Some(dep_ws) = pt.workshops[tile_index][sec_sk.as_index()].take() {
                            swept += dep_ws.inv.total(cash);
                        }
                        let dep_rel =
                            std::mem::take(&mut pt.slot_contracts[tile_index][sec_sk.as_index()]);
                        if !dep_rel.is_empty() && s_raw != u16::MAX {
                            released.push((t_sec, dep_rel));
                        }
                        pt.tiles[tile_index].slots.set(sec_sk, None);
                        pt.set_slot_owner(tile_index, sec_sk, BuildingOwner::Unowned);
                        pt.reset_slot_directives(tile_index, sec_sk);
                    }
                }
            }
            if swept > 0 {
                cash_to_cap = Some((home_sec, swept));
            }
            break;
        }
        if let Some((h_sec, amt)) = cash_to_cap {
            self.adjust_cash(planet_id, CashSink::Capital(h_sec), amt);
        }
        for (sid, c) in released {
            self.release_contracts_to_between_jobs(planet_id, sid, c, 8);
        }
        Ok(())
    }

    /// Channel-1 ownership guard: only the actual owner of a slot may directly edit its staffing
    /// cap or wage-honoring switch (Polity must nationalize non-state slots first).
    pub fn check_channel1_slot_authority(
        actor: BuildingOwner,
        slot_owner: BuildingOwner,
    ) -> Result<(), String> {
        if actor == BuildingOwner::Unowned {
            return Err("通道一权限拒绝：无产权身份不可下达槽位管理指令。".into());
        }
        if actor != slot_owner {
            return Err(
                "通道一权限拒绝：主体仅可直接调整自身产权名下的建筑槽；政权干预非国有槽位请先执行赎买国有化。"
                    .into(),
            );
        }
        Ok(())
    }

    /// Polity-scoped Channel-1 staffing control (rejects direct edits to private/corporate slots).
    pub fn set_state_slot_target_jobs(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
        target_jobs: Option<u32>,
        immediate_buyout: bool,
    ) -> Result<PlanetSnapshot, String> {
        let ruling = self
            .world
            .query::<&Planet>()
            .iter()
            .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
            .ok_or_else(|| format!("找不到星球：{planet_id}"))?;
        self.set_slot_target_jobs_as(
            BuildingOwner::State { polity_id: ruling },
            planet_id,
            tile_index,
            slot,
            target_jobs,
            immediate_buyout,
        )
    }

    /// Unified owner/player control over a slot's target staffing (`None` restores 100% capacity).
    /// Routes as the slot's current owner; use `set_slot_target_jobs_as` or `set_state_slot_target_jobs`
    /// when enforcing cross-entity Channel-1 caller identity!
    pub fn set_slot_target_jobs(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
        target_jobs: Option<u32>,
        immediate_buyout: bool,
    ) -> Result<PlanetSnapshot, String> {
        let owner = self
            .world
            .query::<(&Planet, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (p, pt))| {
                (p.id == planet_id && tile_index < pt.tiles.len())
                    .then(|| pt.slot_owner(tile_index, slot))
            })
            .unwrap_or(BuildingOwner::Unowned);
        self.set_slot_target_jobs_as(
            owner,
            planet_id,
            tile_index,
            slot,
            target_jobs,
            immediate_buyout,
        )
    }

    /// Channel-1 owner-authenticated staffing control (`immediate_buyout` requires 1-month severance).
    pub fn set_slot_target_jobs_as(
        &mut self,
        actor: BuildingOwner,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
        target_jobs: Option<u32>,
        immediate_buyout: bool,
    ) -> Result<PlanetSnapshot, String> {
        let cash = self.goods.cash_id();
        let c_idx = usize::from(cash.0);

        let (ruling, sec_id, owner, is_state_public, has_ws, capped_target, cur_emp) = {
            let mut info = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let Some(tid) = pt.tiles[tile_index].slots.get(slot) else {
                    return Err("该建筑槽当前没有建筑。".to_string());
                };
                let slot_owner = pt.slot_owner(tile_index, slot);
                Self::check_channel1_slot_authority(actor, slot_owner)?;
                let sec_raw = pt.tiles[tile_index].sector_id;
                if sec_raw == u16::MAX {
                    return Err("该地块未归属任何实控辖区。".to_string());
                }
                let def = self.buildings.get(tid);
                let req = def.slot_jobs_required(slot);
                let capped = target_jobs.map(|n| n.min(req)).unwrap_or(u32::MAX);
                let cur: u32 = pt.slot_contracts[tile_index][slot.as_index()]
                    .iter()
                    .map(|c| c.count)
                    .sum();
                info = Some((
                    planet.ruling_polity,
                    SectorId(sec_raw),
                    slot_owner,
                    def.is_state_core_or_public(),
                    pt.workshops[tile_index][slot.as_index()].is_some(),
                    capped,
                    cur,
                ));
                break;
            }
            info.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        let effective_cap = if capped_target == u32::MAX {
            u32::MAX
        } else {
            capped_target
        };

        if immediate_buyout && cur_emp > effective_cap {
            let need_cut = cur_emp - effective_cap;
            let mut preview_sev = 0i64;
            let mut rem = need_cut;
            let mut ws_avail = 0i64;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if let Some(ws) = pt.workshops[tile_index][slot.as_index()].as_ref() {
                    ws_avail = ws.inv.avail(cash);
                }
                for c in pt.slot_contracts[tile_index][slot.as_index()].iter().rev() {
                    if rem == 0 {
                        break;
                    }
                    let take = c.count.min(rem);
                    if take > 0 {
                        let cut_bill = if take == c.count {
                            c.monthly_bill_cents
                        } else {
                            (c.monthly_bill_cents * i64::from(take)) / i64::from(c.count)
                        };
                        preview_sev += cut_bill.max(0);
                        rem -= take;
                    }
                }
                break;
            }

            let non_ws_sink = if is_state_public {
                CashSink::Polity(ruling)
            } else {
                match owner {
                    BuildingOwner::Enterprise { enterprise_id } => {
                        CashSink::Enterprise(enterprise_id)
                    }
                    BuildingOwner::Trust { trust_id } => CashSink::Trust(trust_id),
                    BuildingOwner::LocalPrivate { home_sector } => CashSink::Capital(home_sector),
                    _ => CashSink::Polity(ruling),
                }
            };
            let payer_avail = if has_ws {
                ws_avail
            } else {
                match non_ws_sink {
                    CashSink::Polity(pid) => self.polity_cash_cents(pid),
                    CashSink::Capital(sid) => self.sector_pool_cents(planet_id, sid),
                    CashSink::Enterprise(eid) => self.enterprise_cash_cents(planet_id, eid),
                    CashSink::Trust(tid) => self.trust_cash_cents(planet_id, tid),
                }
            };
            if payer_avail < preview_sev {
                return Err(format!(
                    "资金不足以支付提前解雇补偿金：需 {:.2} 资金（当前可用 {:.2}），可改用不勾选立即买断等待合同到期缩编。",
                    preview_sev as f64 / CASH_SCALE as f64,
                    payer_avail as f64 / CASH_SCALE as f64,
                ));
            }

            let mut payouts: Vec<(PopId, u32, i64)> = Vec::new();
            let mut rem_cut = need_cut;
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id != planet_id {
                    continue;
                }
                pt.slot_target_jobs[tile_index][slot.as_index()] = capped_target;
                let contracts = &mut pt.slot_contracts[tile_index][slot.as_index()];
                for c in contracts.iter_mut().rev() {
                    if rem_cut == 0 {
                        break;
                    }
                    let take = c.count.min(rem_cut);
                    if take > 0 {
                        let cut_bill = if take == c.count {
                            c.monthly_bill_cents
                        } else {
                            (c.monthly_bill_cents * i64::from(take)) / i64::from(c.count)
                        };
                        c.count -= take;
                        c.monthly_bill_cents -= cut_bill;
                        rem_cut -= take;
                        payouts.push((c.pop_id, take, cut_bill.max(0)));
                    }
                }
                contracts.retain(|c| c.count > 0);
                if has_ws {
                    if let Some(ws) = pt.workshops[tile_index][slot.as_index()].as_mut() {
                        ws.inv.available[c_idx] -= preview_sev;
                    }
                }
                break;
            }

            if !has_ws && preview_sev > 0 {
                self.adjust_cash(planet_id, non_ws_sink, -preview_sev);
            }

            let def_pool = self.pop_catalog.default_pool_id();
            let mut ref_ids = BTreeSet::new();
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id == planet_id {
                    ref_ids = pt.referenced_pop_ids_in_sector(sec_id);
                    break;
                }
            }
            for (_, (sec, mkt, pops)) in self
                .world
                .query_mut::<(&Sector, &mut SectorMarket, &mut SectorPops)>()
            {
                if sec.planet_id == planet_id && sec.id == sec_id {
                    for (pop_id, heads, sev_cents) in payouts {
                        if let Some(gi) = pops.index_by_id(pop_id) {
                            if sev_cents > 0 {
                                pops.groups[gi].inv.ensure_len(self.goods.len());
                                pops.groups[gi].inv.available[c_idx] += sev_cents;
                            }
                            let r_i = usize::from(pops.groups[gi].pop_type.0);
                            if let Some(wb) = mkt.wage_books.get_mut(r_i) {
                                wb.last_laid_off += heads;
                            }
                        }
                        pops.release_to_between_jobs(&self.goods, pop_id, heads, 3);
                    }
                    pops.prune_empty_groups(&self.goods, def_pool, &ref_ids);
                    break;
                }
            }
        } else {
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id == planet_id {
                    pt.slot_target_jobs[tile_index][slot.as_index()] = capped_target;
                    break;
                }
            }
        }

        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    /// Unified owner/player control over whether a slot redeems its wage obligations (`true`)
    /// or intentionally withholds/defaults (`false`, accruing `UnpaidWageClaim` / arrears).
    pub fn set_slot_wage_redemption(
        &mut self,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
        honor_wages: bool,
    ) -> Result<PlanetSnapshot, String> {
        let owner = self
            .world
            .query::<(&Planet, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (p, pt))| {
                (p.id == planet_id && tile_index < pt.tiles.len())
                    .then(|| pt.slot_owner(tile_index, slot))
            })
            .unwrap_or(BuildingOwner::Unowned);
        self.set_slot_wage_redemption_as(owner, planet_id, tile_index, slot, honor_wages)
    }

    /// Channel-1 owner-authenticated wage redemption toggle.
    pub fn set_slot_wage_redemption_as(
        &mut self,
        actor: BuildingOwner,
        planet_id: &str,
        tile_index: usize,
        slot: TileSlotKey,
        honor_wages: bool,
    ) -> Result<PlanetSnapshot, String> {
        let mut updated = false;
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            if tile_index >= pt.tiles.len() {
                return Err(format!("地块序号 {tile_index} 超出范围。"));
            }
            if pt.tiles[tile_index].slots.get(slot).is_none() {
                return Err("该建筑槽当前没有建筑。".to_string());
            }
            let slot_owner = pt.slot_owner(tile_index, slot);
            Self::check_channel1_slot_authority(actor, slot_owner)?;
            pt.slot_pay_wages[tile_index][slot.as_index()] = honor_wages;
            updated = true;
            break;
        }
        if !updated {
            return Err(format!("找不到星球：{planet_id}"));
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    /// Conserved pioneer migration when founding a core/outpost in a sparse or newly claimed `Sector`:
    /// moves surplus `Unowned` workers together with their exact pro-rata cash & pantry goods from
    /// same-polity sectors (`audit.rs` money, goods, and total headcount remain 100% conserved).
    pub(crate) fn migrate_pioneers_if_frontier(
        &mut self,
        actor_polity: PolityId,
        planet_id: &str,
        dst_sector: SectorId,
        target_pop: u32,
    ) {
        let cur_pop = self
            .world
            .query::<(&Sector, &SectorPops)>()
            .iter()
            .find_map(|(_, (s, p))| {
                (s.planet_id == planet_id && s.id == dst_sector).then(|| p.total_population())
            })
            .unwrap_or(0);
        if cur_pop >= target_pop {
            return;
        }
        let mut needed = target_pop - cur_pop;
        let owned_secs = self.polity_tree_sectors_on_planet(actor_polity, planet_id);
        let div_roles = self.pop_catalog.dividend_recipients();

        let mut donors: Vec<(SectorId, u32)> = self
            .world
            .query::<(&Sector, &SectorPops)>()
            .iter()
            .filter_map(|(_, (s, p))| {
                if s.planet_id != planet_id
                    || s.id == dst_sector
                    || !owned_secs.contains(&s.id.0)
                {
                    return None;
                }
                let free_workers: u32 = p
                    .groups
                    .iter()
                    .filter(|g| {
                        g.employer == BuildingOwner::Unowned
                            && !div_roles.contains(&g.pop_type)
                            && !self.pop_catalog.get(g.pop_type).is_vagrant_pool
                    })
                    .map(|g| g.count)
                    .sum();
                free_workers
                    .checked_sub(40)
                    .filter(|&n| n > 0)
                    .map(|n| (s.id, n))
            })
            .collect();
        donors.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let def_pool = self.pop_catalog.default_pool_id();
        let good_len = self.goods.len();

        for (src_sid, max_give) in donors {
            if needed == 0 {
                break;
            }
            let want_from_src = needed.min(max_give);
            if want_from_src == 0 {
                continue;
            }
            let mut src_pops_opt: Option<SectorPops> = None;
            let mut dst_pops_opt: Option<SectorPops> = None;
            for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                if sec.planet_id != planet_id {
                    continue;
                }
                if sec.id == src_sid {
                    src_pops_opt = Some(std::mem::take(pops));
                } else if sec.id == dst_sector {
                    dst_pops_opt = Some(std::mem::take(pops));
                }
            }
            if let (Some(mut src_pops), Some(mut dst_pops)) = (src_pops_opt.take(), dst_pops_opt.take()) {
                let mut rem = want_from_src;
                // Prefer `default_pool` first, then other `Unowned` non-dividend worker pools.
                let mut cand_indices: Vec<usize> = (0..src_pops.groups.len())
                    .filter(|&gi| {
                        let g = &src_pops.groups[gi];
                        g.count > 0
                            && g.employer == BuildingOwner::Unowned
                            && !div_roles.contains(&g.pop_type)
                            && !self.pop_catalog.get(g.pop_type).is_vagrant_pool
                    })
                    .collect();
                cand_indices.sort_by_key(|&gi| {
                    (
                        src_pops.groups[gi].pop_type != def_pool,
                        std::cmp::Reverse(src_pops.groups[gi].count),
                    )
                });

                for gi in cand_indices {
                    if rem == 0 {
                        break;
                    }
                    let src_cnt = src_pops.groups[gi].count;
                    if src_cnt == 0 {
                        continue;
                    }
                    let moved = rem.min(src_cnt);
                    let role = src_pops.groups[gi].pop_type;
                    let cid = src_pops.groups[gi].culture_id;
                    let dst_gi =
                        dst_pops.ensure_group_index(&self.goods, role, cid, BuildingOwner::Unowned);
                    src_pops.groups[gi].count -= moved;
                    dst_pops.groups[dst_gi].count += moved;
                    dst_pops.groups[dst_gi].sync_capacity();
                    src_pops.groups[gi].inv.ensure_len(good_len);
                    dst_pops.groups[dst_gi].inv.ensure_len(good_len);

                    for g_i in 0..good_len {
                        let gid = GoodId(g_i as u16);
                        let src_avail = src_pops.groups[gi].inv.available[g_i];
                        if src_avail <= 0 {
                            continue;
                        }
                        let raw_share = if moved == src_cnt {
                            src_avail
                        } else {
                            ((i128::from(src_avail) * i128::from(moved)) / i128::from(src_cnt))
                                as i64
                        };
                        if raw_share <= 0 {
                            continue;
                        }
                        if self.goods.get(gid).unit_volume == 0 {
                            src_pops.groups[gi].inv.available[g_i] -= raw_share;
                            dst_pops.groups[dst_gi].inv.available[g_i] += raw_share;
                        } else {
                            let accepted =
                                dst_pops.groups[dst_gi].inv.produce_capped(&self.goods, gid, raw_share);
                            src_pops.groups[gi].inv.available[g_i] -= accepted;
                        }
                    }
                    src_pops.groups[gi].sync_capacity();
                    rem -= moved;
                    needed -= moved;
                }

                for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                    if sec.planet_id != planet_id {
                        continue;
                    }
                    if sec.id == src_sid {
                        *pops = src_pops;
                        break;
                    }
                }
                for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                    if sec.planet_id != planet_id {
                        continue;
                    }
                    if sec.id == dst_sector {
                        *pops = dst_pops;
                        break;
                    }
                }
            } else {
                if let Some(sp) = src_pops_opt {
                    for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                        if sec.planet_id == planet_id && sec.id == src_sid {
                            *pops = sp;
                            break;
                        }
                    }
                }
                if let Some(dp) = dst_pops_opt {
                    for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                        if sec.planet_id == planet_id && sec.id == dst_sector {
                            *pops = dp;
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Immediately staffs a newly built state core / public slot from local `Unowned` labor in `sector_id`
    /// so its `base:admin` quota and hub baseline take effect right away without waiting for month-end.
    pub(crate) fn staff_state_slot_immediately(
        &mut self,
        actor_polity: PolityId,
        planet_id: &str,
        sector_id: SectorId,
        tile_index: usize,
        slot: TileSlotKey,
        def: &BuildingDef,
    ) {
        let scale = def.scale_for_slot(slot);
        let owner = BuildingOwner::State {
            polity_id: actor_polity,
        };
        let def_pool = self.pop_catalog.default_pool_id();
        let good_len = self.goods.len();
        let mut new_contracts: Vec<(PopId, u32, i64)> = Vec::new();

        for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.planet_id != planet_id || sec.id != sector_id {
                continue;
            }
            for (role_str, base_req) in &def.jobs_by_role {
                let mut rem = *base_req * scale;
                if rem == 0 {
                    continue;
                }
                let role_id = def.resolve_role_id(&self.pop_catalog, role_str);
                let wage_per_worker_cents =
                    (self.pop_catalog.get(role_id).base_wage_per_100 * CASH_SCALE / 100).max(15);

                // 1) Exact role match in `Unowned`, then 2) promote from `default_pool` `Unowned`.
                let source_roles: [PopTypeId; 2] = [role_id, def_pool];
                for src_role in source_roles {
                    if rem == 0 {
                        break;
                    }
                    let Some(src_gi) = pops.find_index(src_role, 0, BuildingOwner::Unowned) else {
                        continue;
                    };
                    let src_cnt = pops.groups[src_gi].count;
                    if src_cnt == 0 {
                        continue;
                    }
                    let take = rem.min(src_cnt);
                    let dst_gi = pops.ensure_group_index(&self.goods, role_id, 0, owner);
                    let dst_pid = pops.groups[dst_gi].id;
                    pops.groups[src_gi].count -= take;
                    pops.groups[dst_gi].count += take;
                    pops.groups[dst_gi].sync_capacity();
                    pops.groups[src_gi].inv.ensure_len(good_len);
                    pops.groups[dst_gi].inv.ensure_len(good_len);

                    for g_i in 0..good_len {
                        let gid = GoodId(g_i as u16);
                        let src_avail = pops.groups[src_gi].inv.available[g_i];
                        if src_avail <= 0 {
                            continue;
                        }
                        let raw_share = if take == src_cnt {
                            src_avail
                        } else {
                            ((i128::from(src_avail) * i128::from(take)) / i128::from(src_cnt))
                                as i64
                        };
                        if raw_share <= 0 {
                            continue;
                        }
                        if self.goods.get(gid).unit_volume == 0 {
                            pops.groups[src_gi].inv.available[g_i] -= raw_share;
                            pops.groups[dst_gi].inv.available[g_i] += raw_share;
                        } else {
                            let accepted =
                                pops.groups[dst_gi].inv.produce_capped(&self.goods, gid, raw_share);
                            pops.groups[src_gi].inv.available[g_i] -= accepted;
                        }
                    }
                    pops.groups[src_gi].sync_capacity();

                    let bill = i64::from(take) * wage_per_worker_cents;
                    new_contracts.push((dst_pid, take, bill));
                    rem -= take;
                    if role_id == def_pool {
                        break;
                    }
                }
            }
            break;
        }

        if !new_contracts.is_empty() {
            for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                if planet.id == planet_id && tile_index < pt.tiles.len() {
                    for (pid, cnt, bill) in new_contracts {
                        pt.add_slot_contract(tile_index, slot, pid, cnt, bill);
                    }
                    break;
                }
            }
        }
    }
}
