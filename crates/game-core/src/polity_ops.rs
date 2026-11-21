//! Polity, Sovereign Hierarchy, Fiscal Vaults & Macro Policies (`多级政权、财政金库与宏观政策层`).

use crate::building::{scale_output_by_staffing, BuildingCatalog};
use crate::economy::{
    self, BuildingOwner, CounterpartyId, DepotPolicy, EnterpriseId, MonetaryPolicy, Polity,
    PolityId, PolitySnapshot, PopTypeCatalog, Sector, SectorCapital, SectorId, SectorMarket,
    SectorPops, TrustId,
};
use crate::market::{GoodCatalog, Inventory, GOOD_ADMIN};
use crate::planet::{CashSink, Planet, PlanetTiles};
use crate::protocol::PlanetSnapshot;
use crate::rules::PendingEvent;
use crate::surface::TileTag;
use crate::Game;
use hecs::World;
use std::collections::{BTreeMap, BTreeSet};

/// Pure accounting of `(admin_used, admin_capacity)` for every `Polity` in `world`:
/// - `admin_used` = number of claimed land tiles (`TileTag::Land`) governed under `sec.admin_polity == polity_id`;
/// - `admin_capacity` = `polity.base_admin_capacity` + staffed `base:admin` output of state buildings owned by `polity_id`.
pub(crate) fn compute_polity_admin_budgets(
    world: &World,
    goods: &GoodCatalog,
    buildings: &BuildingCatalog,
    _pop_catalog: &PopTypeCatalog,
) -> BTreeMap<PolityId, (i64, i64)> {
    let has_admin_good = goods.id_of(GOOD_ADMIN).is_some();
    let mut budgets: BTreeMap<PolityId, (i64, i64)> = BTreeMap::new();
    for (_, pol) in world.query::<&Polity>().iter() {
        let base_cap = if has_admin_good {
            pol.base_admin_capacity
        } else {
            pol.base_admin_capacity.max(2_000)
        };
        budgets.insert(pol.id, (0, base_cap));
    }

    let mut sec_admin_map: BTreeMap<(String, u16), PolityId> = BTreeMap::new();
    let mut sec_pops_query = world.query::<(&Sector, &SectorPops)>();
    let mut sec_pops_map: BTreeMap<(String, u16), &SectorPops> = BTreeMap::new();
    for (_, (sec, pops)) in sec_pops_query.iter() {
        sec_admin_map.insert((sec.planet_id.clone(), sec.id.0), sec.admin_polity);
        sec_pops_map.insert((sec.planet_id.clone(), sec.id.0), pops);
    }

    for (_, (planet, pt)) in world.query::<(&Planet, &PlanetTiles)>().iter() {
        for tile in &pt.tiles {
            if tile.sector_id == u16::MAX {
                continue;
            }
            if tile.tags.has(TileTag::Land) {
                let admin_pid = sec_admin_map
                    .get(&(planet.id.clone(), tile.sector_id))
                    .copied()
                    .unwrap_or(planet.ruling_polity);
                if let Some(entry) = budgets.get_mut(&admin_pid) {
                    entry.0 += 1;
                }
            }
            if !has_admin_good {
                continue;
            }
            let t_idx = usize::from(tile.index);
            let spops_opt = sec_pops_map.get(&(planet.id.clone(), tile.sector_id)).copied();
            for (sk, tid) in tile.slots.iter_occupied() {
                let BuildingOwner::State { polity_id } = pt.slot_owner(t_idx, sk) else {
                    continue;
                };
                let def = buildings.get(tid);
                let admin_base: i64 = def
                    .monthly_output
                    .iter()
                    .filter(|(gid, _)| gid == GOOD_ADMIN)
                    .map(|&(_, amt)| amt)
                    .sum();
                if admin_base <= 0 {
                    continue;
                }
                let scale = i64::from(def.scale_for_slot(sk));
                let req = def.slot_jobs_required(sk);
                let emp: u32 = pt.slot_contracts[t_idx][sk.as_index()]
                    .iter()
                    .filter(|c| {
                        c.count > 0
                            && spops_opt
                                .map(|sp| sp.index_by_id(c.pop_id).is_some())
                                .unwrap_or(true)
                    })
                    .map(|c| c.count)
                    .sum();
                let contrib = scale_output_by_staffing(admin_base * scale, emp, req);
                if let Some(entry) = budgets.get_mut(&polity_id) {
                    entry.1 += contrib;
                }
            }
        }
    }
    budgets
}

impl Game {
    /// Sets which named entity (`Polity` / `Enterprise` / `Trust`) is controlled by a human player
    /// so bilateral contract & debt obligation events queue into `pending_events` for manual choice.
    pub fn set_controlled_actor(&mut self, actor: Option<CounterpartyId>) {
        self.controlled_actor = actor;
    }

    #[inline]
    pub fn controlled_actor(&self) -> Option<CounterpartyId> {
        self.controlled_actor
    }

    #[inline]
    pub fn pending_events(&self) -> &[PendingEvent] {
        &self.pending_events
    }

    /// Snapshot of any `Polity` in `world` by `PolityId` (central, planetary, sector, or foreign).
    pub fn polity_snapshot(&self, polity: PolityId) -> Option<PolitySnapshot> {
        let budgets = compute_polity_admin_budgets(
            &self.world,
            &self.goods,
            &self.buildings,
            &self.pop_catalog,
        );
        let (used, cap) = budgets.get(&polity).copied().unwrap_or((0, 0));
        self.world
            .query::<&Polity>()
            .iter()
            .find_map(|(_, pol)| {
                (pol.id == polity).then(|| pol.to_snapshot_with_admin(&self.goods, used, cap))
            })
    }

    pub fn polity_admin_usage(&self, polity: PolityId) -> (i64, i64) {
        compute_polity_admin_budgets(
            &self.world,
            &self.goods,
            &self.buildings,
            &self.pop_catalog,
        )
        .get(&polity)
        .copied()
        .unwrap_or((0, 0))
    }

    /// True if `ancestor == target` or `target` walks up `parent_polity` links to `ancestor`.
    pub fn is_sovereign_ancestor(&self, ancestor: PolityId, target: PolityId) -> bool {
        let mut cur = Some(target);
        for _ in 0..16 {
            let Some(pid) = cur else {
                return false;
            };
            if pid == ancestor {
                return true;
            }
            cur = self
                .world
                .query::<&Polity>()
                .iter()
                .find_map(|(_, p)| (p.id == pid).then_some(p.parent_polity))
                .flatten();
        }
        false
    }

    /// Checks whether `actor_polity` holds direct construction / investment rights in `sector_id`:
    /// automatically true within its own sovereign `Polity` tree, or via `foreign_investment_rights`.
    pub fn has_sector_investment_right(
        &self,
        actor_polity: PolityId,
        planet_id: &str,
        sector_id: SectorId,
    ) -> bool {
        self.world
            .query::<&Sector>()
            .iter()
            .find_map(|(_, sec)| {
                (sec.planet_id == planet_id && sec.id == sector_id).then(|| {
                    self.is_sovereign_ancestor(actor_polity, sec.admin_polity)
                        || self.is_sovereign_ancestor(actor_polity, sec.owner_polity)
                        || sec.foreign_investment_rights.contains(&actor_polity)
                })
            })
            .unwrap_or(false)
    }

    /// Spawns a new homogeneous `Polity` (`中央 / 星区 / 星球 / 大区地方政府` or external sovereign).
    /// When `funded_by` is provided, `initial_cash_cents` is transferred atomically out of `funded_by`'s vault!
    pub fn create_polity(
        &mut self,
        name: String,
        parent_polity: Option<PolityId>,
        tax_rate_permille: u16,
        upward_tax_share_permille: u16,
        initial_cash_cents: i64,
        funded_by: Option<PolityId>,
    ) -> Result<PolityId, String> {
        if initial_cash_cents < 0 || upward_tax_share_permille > 1_000 || tax_rate_permille > 1_000 {
            return Err("政权税率与分成千分比必须在 0..=1000 之间，初始资金不能为负。".into());
        }
        if let Some(p_id) = parent_polity {
            let exists = self.world.query::<&Polity>().iter().any(|(_, p)| p.id == p_id);
            if !exists {
                return Err(format!("找不到上级政权 #{}.", p_id.0));
            }
        }
        if let Some(src_id) = funded_by {
            if self.polity_cash_cents(src_id) < initial_cash_cents {
                return Err("拨款政权金库现金不足以设立新政权。".into());
            }
            let c = usize::from(self.goods.cash_id().0);
            for (_, pol) in self.world.query_mut::<&mut Polity>() {
                if pol.id == src_id {
                    pol.vault.ensure_len(self.goods.len());
                    pol.vault.available[c] -= initial_cash_cents;
                    break;
                }
            }
        }
        let next_id = self
            .world
            .query::<&Polity>()
            .iter()
            .map(|(_, p)| p.id.0 + 1)
            .max()
            .unwrap_or(0);
        let pid = PolityId(next_id);
        let cash_id = self.goods.cash_id();
        let mut vault = Inventory::new(self.goods.len(), i64::MAX / 8);
        vault.available[usize::from(cash_id.0)] = initial_cash_cents;
        self.world.spawn((Polity {
            id: pid,
            name,
            parent_polity,
            upward_tax_share_permille,
            currency_good: self.goods.get(cash_id).id.clone(),
            vault,
            monetary_policy: MonetaryPolicy::Tight,
            tax_rate_permille,
            import_tariff_cents_per_vol: 0,
            embargoed_polities: BTreeSet::new(),
            credit_rating: 100,
            base_admin_capacity: 12,
            last_tax_income_cents: 0,
            last_state_enterprise_net_cents: 0,
            last_freight_revenue_cents: 0,
            last_seigniorage_cents: 0,
            last_admin_upkeep_cents: 0,
            last_net_cashflow_cents: 0,
        },));
        Ok(pid)
    }

    /// Assigns `sector_id`'s immediate governing `admin_polity` (either delegating to a subordinate
    /// regional/planetary `Polity`, or elevating back to central direct administration `直辖`).
    pub fn set_sector_admin_polity(
        &mut self,
        actor_polity: PolityId,
        planet_id: &str,
        sector_id: SectorId,
        new_admin: PolityId,
    ) -> Result<(), String> {
        let new_in_tree = self.is_sovereign_ancestor(actor_polity, new_admin);
        if !new_in_tree {
            return Err("仅可将辖区交由本主权树内的政权管辖或收归直辖。".into());
        }
        let cur_owner = self
            .world
            .query::<&Sector>()
            .iter()
            .find_map(|(_, s)| (s.planet_id == planet_id && s.id == sector_id).then_some(s.owner_polity))
            .ok_or_else(|| format!("找不到辖区 #{}", sector_id.0))?;
        if !self.is_sovereign_ancestor(actor_polity, cur_owner) {
            return Err("当前政权对该辖区无主权管辖权。".into());
        }
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == sector_id {
                sec.admin_polity = new_admin;
                return Ok(());
            }
        }
        Err(format!("找不到辖区 #{}", sector_id.0))
    }

    /// Claims a single unclaimed land tile (`sector_id == u16::MAX`) into the planet's ruling `Polity`
    /// (or active controlled `Polity`), enforcing `base:admin` direct-rule quota and territorial contiguity,
    /// and automatically merging into or spawning `Sector(geo_region_id, polity)`.
    pub fn claim_tile(&mut self, planet_id: &str, tile_index: usize) -> Result<PlanetSnapshot, String> {
        let actor = self.resolve_planet_actor_polity(planet_id)?;
        self.claim_tile_as(actor, planet_id, tile_index)
    }

    pub fn claim_tile_as(
        &mut self,
        actor_polity: PolityId,
        planet_id: &str,
        tile_index: usize,
    ) -> Result<PlanetSnapshot, String> {
        self.claim_tile_internal(actor_polity, planet_id, tile_index)?;
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    /// Claims up to `max_count` contiguous unclaimed land tiles inside `region_id` (preferring paths
    /// toward `CoreSite` hub candidates and high-habitability land).
    pub fn claim_region_frontier(
        &mut self,
        planet_id: &str,
        region_id: u16,
        max_count: usize,
    ) -> Result<PlanetSnapshot, String> {
        let actor = self.resolve_planet_actor_polity(planet_id)?;
        let limit = max_count.clamp(1, 32);
        let mut claimed_any = 0usize;
        let mut last_err: Option<String> = None;

        for _ in 0..limit {
            let (used, cap) = self.polity_admin_usage(actor);
            if used >= cap {
                if claimed_any == 0 {
                    return Err(format!(
                        "直辖行政管理力已满（{used}/{cap}），请先建造或扩编殖民据点 / 大区核心。"
                    ));
                }
                break;
            }
            let owned_sectors = self.polity_tree_sectors_on_planet(actor, planet_id);
            let next_cand = {
                let mut best: Option<(usize, i32)> = None;
                for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                    if planet.id != planet_id {
                        continue;
                    }
                    let core_targets: Vec<[f32; 2]> = pt
                        .tiles
                        .iter()
                        .filter(|t| t.region_id == region_id && t.tags.has(TileTag::CoreSite))
                        .map(|t| pt.regions.get(usize::from(t.region_id)).map(|r| r.center).unwrap_or([600.0, 320.0]))
                        .collect();
                    let _ = core_targets;
                    for t in &pt.tiles {
                        let idx = usize::from(t.index);
                        if t.region_id != region_id
                            || t.sector_id != u16::MAX
                            || !t.tags.has(TileTag::Land)
                        {
                            continue;
                        }
                        if !is_tile_contiguous_to_owned(pt, idx, &owned_sectors) {
                            continue;
                        }
                        let mut score = i32::from(t.tags.habitability_permille());
                        if t.tags.has(TileTag::CoreSite) {
                            score += 5_000;
                        } else if pt.tile_neighbors[idx]
                            .iter()
                            .any(|&nb| pt.tiles[usize::from(nb)].tags.has(TileTag::CoreSite))
                        {
                            score += 900;
                        }
                        if best.map(|(_, bs)| score > bs).unwrap_or(true) {
                            best = Some((idx, score));
                        }
                    }
                    break;
                }
                best.map(|(idx, _)| idx)
            };
            let Some(cand_idx) = next_cand else {
                break;
            };
            match self.claim_tile_internal(actor, planet_id, cand_idx) {
                Ok(()) => claimed_any += 1,
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            }
        }

        if claimed_any == 0 {
            return Err(last_err.unwrap_or_else(|| {
                "本大区当前没有与已控领土接壤（或隔近海海峡相望）的无主陆地格可框入。".into()
            }));
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub(crate) fn resolve_planet_actor_polity(&self, planet_id: &str) -> Result<PolityId, String> {
        let ruling = self
            .world
            .query::<&Planet>()
            .iter()
            .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
            .ok_or_else(|| format!("找不到星球：{planet_id}"))?;
        Ok(match self.controlled_actor {
            Some(CounterpartyId::Polity { polity_id }) => polity_id,
            _ => ruling,
        })
    }

    pub(crate) fn polity_tree_sectors_on_planet(
        &self,
        actor_polity: PolityId,
        planet_id: &str,
    ) -> BTreeSet<u16> {
        self.world
            .query::<&Sector>()
            .iter()
            .filter_map(|(_, s)| {
                (s.planet_id == planet_id
                    && (self.is_sovereign_ancestor(actor_polity, s.owner_polity)
                        || self.is_sovereign_ancestor(actor_polity, s.admin_polity)))
                .then_some(s.id.0)
            })
            .collect()
    }

    fn claim_tile_internal(
        &mut self,
        actor_polity: PolityId,
        planet_id: &str,
        tile_index: usize,
    ) -> Result<(), String> {
        let pol_name = self
            .world
            .query::<&Polity>()
            .iter()
            .find_map(|(_, p)| (p.id == actor_polity).then(|| p.name.clone()))
            .ok_or_else(|| format!("找不到政权 #{}", actor_polity.0))?;

        let (used, cap) = self.polity_admin_usage(actor_polity);
        if used >= cap {
            return Err(format!(
                "「{pol_name}」直辖行政管理力已满（{used}/{cap}），请先建造或扩编殖民据点 / 大区核心。"
            ));
        }

        let owned_sectors = self.polity_tree_sectors_on_planet(actor_polity, planet_id);
        let (geo_reg, reg_name, tile_hab, tile_is_coastal) = {
            let mut info = None;
            for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
                if planet.id != planet_id {
                    continue;
                }
                if tile_index >= pt.tiles.len() {
                    return Err(format!("地块序号 {tile_index} 超出范围。"));
                }
                let t = &pt.tiles[tile_index];
                if !t.tags.has(TileTag::Land) {
                    return Err("仅陆地地块需要消耗行政管理力框入政区（近海大陆架随沿海陆地自动划入）。".into());
                }
                if t.sector_id != u16::MAX {
                    return Err("该地块已纳入实控辖区，无需重复框入。".into());
                }
                if !is_tile_contiguous_to_owned(pt, tile_index, &owned_sectors) {
                    return Err("仅可连片框入与当前已控领土陆路接壤（或隔近海大陆架相望）的无主陆地格。".into());
                }
                let rname = pt
                    .regions
                    .get(usize::from(t.region_id))
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| format!("{:02} · 拓荒辖区", t.region_id + 1));
                info = Some((
                    t.region_id,
                    rname,
                    u32::from(t.tags.habitability_permille()),
                    t.tags.has(TileTag::Coastal),
                ));
                break;
            }
            info.ok_or_else(|| format!("找不到星球：{planet_id}"))?
        };

        // Find existing `Sector(geo_reg, actor_polity)` or allocate a dense new `SectorId`.
        let mut existing_sec_id: Option<SectorId> = None;
        let mut other_polity_in_reg = false;
        let mut planet_sec_count: usize = 0;
        for (_, sec) in self.world.query::<&Sector>().iter() {
            if sec.planet_id != planet_id {
                continue;
            }
            planet_sec_count = planet_sec_count.max(usize::from(sec.id.0) + 1);
            if sec.geo_region_id == geo_reg {
                if sec.owner_polity == actor_polity {
                    existing_sec_id = Some(sec.id);
                } else {
                    other_polity_in_reg = true;
                }
            }
        }

        let target_sec_id = if let Some(sid) = existing_sec_id {
            for (_, sec) in self.world.query_mut::<&mut Sector>() {
                if sec.planet_id == planet_id && sec.id == sid {
                    sec.habitability_sum_permille += tile_hab;
                    if tile_is_coastal {
                        sec.is_coastal = true;
                    }
                    break;
                }
            }
            sid
        } else {
            let new_sid = SectorId(planet_sec_count as u16);
            planet_sec_count += 1;
            let sec_name = if other_polity_in_reg {
                format!("{reg_name}（{pol_name}）")
            } else {
                reg_name
            };
            let default_pool = self.pop_catalog.default_pool_id();
            let vag_opt = self
                .pop_catalog
                .has_dedicated_vagrant_pool()
                .then(|| self.pop_catalog.vagrant_pool_id());
            let sector = Sector {
                id: new_sid,
                planet_id: planet_id.to_string(),
                geo_region_id: geo_reg,
                owner_polity: actor_polity,
                admin_polity: actor_polity,
                foreign_investment_rights: BTreeSet::new(),
                name: sec_name,
                allow_private_land_use: true,
                habitability_sum_permille: tile_hab,
                is_coastal: tile_is_coastal,
                state_depot: Inventory::new(self.goods.len(), economy::BASE_SECTOR_DEPOT_CAPACITY),
                depot_policy: DepotPolicy::Idle,
            };
            let market = SectorMarket::new(&self.goods, &self.pop_catalog);
            let capital = SectorCapital::new(&self.goods, 0);
            let pops = SectorPops::from_unowned_groups(&self.goods, default_pool, vag_opt, &[]);
            self.world.spawn((sector, market, capital, pops));
            new_sid
        };

        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id != planet_id {
                continue;
            }
            pt.tiles[tile_index].sector_id = target_sec_id.0;
            // Automatically claim adjacent unclaimed continental-shelf (`CoastalSea`) water cells in the same region at 0 admin cost.
            if tile_is_coastal {
                let nbs = pt.tile_neighbors[tile_index].clone();
                for nb_u16 in nbs {
                    let nb = usize::from(nb_u16);
                    if nb < pt.tiles.len()
                        && pt.tiles[nb].sector_id == u16::MAX
                        && pt.tiles[nb].region_id == geo_reg
                        && pt.tiles[nb].tags.has(TileTag::CoastalSea)
                    {
                        pt.tiles[nb].sector_id = target_sec_id.0;
                    }
                }
            }
            pt.refresh_sector_topology(planet_sec_count);
            break;
        }
        Ok(())
    }

    /// Grants foreign direct investment rights in `sector_id` to `investor_polity` (`外资准入协定`).
    pub fn grant_investment_right(
        &mut self,
        host_polity: PolityId,
        planet_id: &str,
        sector_id: SectorId,
        investor_polity: PolityId,
    ) -> Result<(), String> {
        let sec_owner = self
            .world
            .query::<&Sector>()
            .iter()
            .find_map(|(_, s)| (s.planet_id == planet_id && s.id == sector_id).then_some(s.owner_polity))
            .ok_or_else(|| format!("找不到辖区 #{}", sector_id.0))?;
        if !self.is_sovereign_ancestor(host_polity, sec_owner) {
            return Err("仅东道主权政权可授予本辖区的外资准入权。".into());
        }
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == sector_id {
                sec.foreign_investment_rights.insert(investor_polity);
                return Ok(());
            }
        }
        Err(format!("找不到辖区 #{}", sector_id.0))
    }

    /// Revokes `investor_polity`'s future construction rights in `sector_id`.
    pub fn revoke_investment_right(
        &mut self,
        host_polity: PolityId,
        planet_id: &str,
        sector_id: SectorId,
        investor_polity: PolityId,
    ) -> Result<(), String> {
        let sec_owner = self
            .world
            .query::<&Sector>()
            .iter()
            .find_map(|(_, s)| (s.planet_id == planet_id && s.id == sector_id).then_some(s.owner_polity))
            .ok_or_else(|| format!("找不到辖区 #{}", sector_id.0))?;
        if !self.is_sovereign_ancestor(host_polity, sec_owner) {
            return Err("仅东道主权政权可收回本辖区的外资准入权。".into());
        }
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == sector_id {
                sec.foreign_investment_rights.remove(&investor_polity);
                return Ok(());
            }
        }
        Err(format!("找不到辖区 #{}", sector_id.0))
    }

    /// Channel-3 conserved fiscal grant / earmarked transfer between two `Polity` vaults
    /// (e.g. central government skip-level transfer directly to a sector `Polity`).
    pub fn transfer_polity_funds(
        &mut self,
        from_polity: PolityId,
        to_polity: PolityId,
        amount_cents: i64,
    ) -> Result<(), String> {
        if amount_cents <= 0 || from_polity == to_polity {
            return Err("转移支付金额必须大于 0，且收支政权不能相同。".into());
        }
        if self.polity_cash_cents(from_polity) < amount_cents {
            return Err("拨款政权金库可用现金不足。".into());
        }
        let to_exists = self
            .world
            .query::<&Polity>()
            .iter()
            .any(|(_, p)| p.id == to_polity);
        if !to_exists {
            return Err(format!("找不到接收政权 #{}.", to_polity.0));
        }
        let c = usize::from(self.goods.cash_id().0);
        for (_, pol) in self.world.query_mut::<&mut Polity>() {
            if pol.id == from_polity {
                pol.vault.ensure_len(self.goods.len());
                pol.vault.available[c] -= amount_cents;
            } else if pol.id == to_polity {
                pol.vault.ensure_len(self.goods.len());
                pol.vault.available[c] += amount_cents;
            }
        }
        Ok(())
    }

    pub(crate) fn polity_cash_cents(&self, polity: PolityId) -> i64 {
        self.world
            .query::<&Polity>()
            .iter()
            .find_map(|(_, pol)| (pol.id == polity).then(|| pol.cash_cents(&self.goods)))
            .unwrap_or(0)
    }

    pub(crate) fn sector_pool_cents(&self, planet_id: &str, sector: SectorId) -> i64 {
        self.world
            .query::<(&Sector, &SectorCapital)>()
            .iter()
            .find_map(|(_, (s, cap))| {
                (s.planet_id == planet_id && s.id == sector).then(|| cap.pool_cents(&self.goods))
            })
            .unwrap_or(0)
    }

    pub(crate) fn enterprise_cash_cents(&self, planet_id: &str, ent_id: EnterpriseId) -> i64 {
        self.world
            .query::<(&Planet, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (p, pt))| {
                (p.id == planet_id)
                    .then(|| pt.enterprises.get(usize::from(ent_id.0)))
                    .flatten()
                    .map(|e| e.cash_cents(&self.goods))
            })
            .unwrap_or(0)
    }

    pub(crate) fn trust_cash_cents(&self, planet_id: &str, trust_id: TrustId) -> i64 {
        self.world
            .query::<(&Planet, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (p, pt))| {
                (p.id == planet_id)
                    .then(|| pt.trusts.get(usize::from(trust_id.0)))
                    .flatten()
                    .map(|t| t.cash_cents(&self.goods))
            })
            .unwrap_or(0)
    }

    /// Adds (or with a negative amount removes) cash directly in a polity vault, sector capital pool,
    /// corporate `Enterprise.vault`, or `Trust.vault`.
    pub(crate) fn adjust_cash(&mut self, planet_id: &str, sink: CashSink, delta_cents: i64) {
        let c = usize::from(self.goods.cash_id().0);
        match sink {
            CashSink::Polity(pid) => {
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == pid {
                        pol.vault.ensure_len(self.goods.len());
                        pol.vault.available[c] += delta_cents;
                        break;
                    }
                }
            }
            CashSink::Capital(sid) => {
                for (_, (sec, cap)) in self.world.query_mut::<(&Sector, &mut SectorCapital)>() {
                    if sec.planet_id == planet_id && sec.id == sid {
                        cap.vault.ensure_len(self.goods.len());
                        cap.vault.available[c] += delta_cents;
                        break;
                    }
                }
            }
            CashSink::Enterprise(eid) => {
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id == planet_id {
                        if let Some(ent) = pt.enterprises.get_mut(usize::from(eid.0)) {
                            ent.vault.ensure_len(self.goods.len());
                            ent.vault.available[c] += delta_cents;
                        }
                        break;
                    }
                }
            }
            CashSink::Trust(tid) => {
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id == planet_id {
                        if let Some(tr) = pt.trusts.get_mut(usize::from(tid.0)) {
                            tr.vault.ensure_len(self.goods.len());
                            tr.vault.available[c] += delta_cents;
                        }
                        break;
                    }
                }
            }
        }
    }

    pub fn set_monetary_policy(
        &mut self,
        planet_id: &str,
        policy: MonetaryPolicy,
    ) -> Result<PlanetSnapshot, String> {
        let ruling = self
            .world
            .query::<&Planet>()
            .iter()
            .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
            .ok_or_else(|| format!("找不到星球：{planet_id}"))?;
        for (_, pol) in self.world.query_mut::<&mut Polity>() {
            if pol.id == ruling {
                pol.monetary_policy = policy;
            }
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn set_depot_policy(
        &mut self,
        planet_id: &str,
        sector_id: u16,
        policy: DepotPolicy,
    ) -> Result<PlanetSnapshot, String> {
        let mut found = false;
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == SectorId(sector_id) {
                sec.depot_policy = policy;
                found = true;
            }
        }
        if !found {
            return Err(format!("找不到辖区编号：{sector_id}"));
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }

    pub fn toggle_private_land_use(
        &mut self,
        planet_id: &str,
        sector_id: u16,
        allow: bool,
    ) -> Result<PlanetSnapshot, String> {
        let mut updated = false;
        for (_, sec) in self.world.query_mut::<&mut Sector>() {
            if sec.planet_id == planet_id && sec.id == SectorId(sector_id) {
                sec.allow_private_land_use = allow;
                updated = true;
                break;
            }
        }
        if !updated {
            return Err(format!("找不到辖区编号：{sector_id}"));
        }
        self.planet_by_id(planet_id)
            .ok_or_else(|| format!("无法生成星球快照：{planet_id}"))
    }
}

/// Checks whether `tile_index` is land-contiguous (or across a 1~2 cell `CoastalSea` strait)
/// to any land tile already controlled by `owned_sectors` (automatically true if `owned_sectors` owns 0 land tiles).
pub(crate) fn is_tile_contiguous_to_owned(
    pt: &PlanetTiles,
    tile_index: usize,
    owned_sectors: &BTreeSet<u16>,
) -> bool {
    let has_any_land = pt
        .tiles
        .iter()
        .any(|t| t.tags.has(TileTag::Land) && owned_sectors.contains(&t.sector_id));
    if !has_any_land {
        return true;
    }
    for &nb_u16 in &pt.tile_neighbors[tile_index] {
        let nb = usize::from(nb_u16);
        if nb < pt.tiles.len()
            && pt.tiles[nb].tags.has(TileTag::Land)
            && owned_sectors.contains(&pt.tiles[nb].sector_id)
        {
            return true;
        }
    }
    // Allow hopping across narrow continental-shelf straits (`Coastal -> CoastalSea (1..2 hops) -> Coastal`).
    if pt.tiles[tile_index].tags.has(TileTag::Coastal) {
        for &sea_u16 in &pt.tile_neighbors[tile_index] {
            let sea1 = usize::from(sea_u16);
            if sea1 >= pt.tiles.len() || !pt.tiles[sea1].tags.has(TileTag::CoastalSea) {
                continue;
            }
            for &nb2_u16 in &pt.tile_neighbors[sea1] {
                let nb2 = usize::from(nb2_u16);
                if nb2 >= pt.tiles.len() {
                    continue;
                }
                if pt.tiles[nb2].tags.has(TileTag::Land)
                    && owned_sectors.contains(&pt.tiles[nb2].sector_id)
                {
                    return true;
                }
                if pt.tiles[nb2].tags.has(TileTag::CoastalSea) {
                    for &nb3_u16 in &pt.tile_neighbors[nb2] {
                        let nb3 = usize::from(nb3_u16);
                        if nb3 < pt.tiles.len()
                            && pt.tiles[nb3].tags.has(TileTag::Land)
                            && owned_sectors.contains(&pt.tiles[nb3].sector_id)
                        {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}
