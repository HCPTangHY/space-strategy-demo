//! Planet & Sector Spatial Container + Objective Economy Evaluation (`星球与大区容器及客观产出求值层`).

use crate::building::{
    scale_output_by_staffing, BuildingCatalog, BuildingDef, BuildingTag, BuildingTags,
    TileSlotKey, TileSlots, SLOTS_PER_TILE,
};
use crate::economy::{
    self, BilateralSupplyContract, BuildingOwner, Enterprise, EnterpriseId, PolityId, PopId,
    PopTypeCatalog, SectorId, SectorPops, SectorTradeLink, StaffContract, Trust, TrustId,
    Workshop, ENTERPRISE_OUTPUT_BONUS_PERMILLE,
};
use crate::market::{GoodCatalog, LogisticsMatrix, CASH_SCALE};
use crate::surface::{self, SurfaceRegion, TileTag, TileTags};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub struct Planet {
    pub id: String,
    pub name: String,
    pub radius_km: f64,
    pub ruling_polity: PolityId,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Position2d {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Appearance {
    pub ocean_rgb: [u8; 3],
    pub land_rgb: [u8; 3],
    pub seed: u32,
}

/// Compact planetary Voronoi tile state in ECS (`24 bytes` total per tile!).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub index: u16,
    pub region_id: u16,
    pub sector_id: u16,
    pub elevation: u8,
    pub tags: TileTags,
    pub slots: TileSlots,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanetTiles {
    pub grid_cell_target: usize,
    pub regions: Vec<SurfaceRegion>,
    pub land_tile_count: usize,
    pub sea_tile_count: usize,
    pub starter_tile_index: usize,
    pub tiles: Vec<Tile>,
    pub slot_owners: Vec<[BuildingOwner; SLOTS_PER_TILE]>,
    /// Owner/player target staffing cap per slot (`u32::MAX` = 100% of `def.slot_jobs_required`).
    pub slot_target_jobs: Vec<[u32; SLOTS_PER_TILE]>,
    /// Owner/player wage redemption switch per slot (`true` = honor wages, `false` = withhold/default).
    pub slot_pay_wages: Vec<[bool; SLOTS_PER_TILE]>,
    /// Persistent locked labor contracts per building slot (`(pop_id, count, monthly_bill_cents)`).
    pub slot_contracts: Vec<[Vec<StaffContract>; SLOTS_PER_TILE]>,
    /// Independent per-slot economic agents: only productive buildings own a `Workshop`
    /// (its own capacity-bounded `Inventory` holding stock + `base:cash`).
    pub workshops: Vec<[Option<Workshop>; SLOTS_PER_TILE]>,
    pub tile_neighbors: Vec<Vec<u16>>,
    pub trade_links: Vec<SectorTradeLink>,
    pub sector_coastal: Vec<bool>,
    /// Static planetary natural-geography transport matrix (`land_terrain_mult`, `sea_steps`, `air_steps`).
    pub geo_topology: economy::GeoTransportTopology,
    /// Precomputed `S × S` freight & monthly capacity matrix (overland relay + coastal shipping lanes).
    pub logistics: LogisticsMatrix,
    pub enterprises: Vec<Enterprise>,
    pub trusts: Vec<Trust>,
    pub bilateral_contracts: Vec<BilateralSupplyContract>,
    pub next_contract_id: u32,
}

/// Creates the per-slot `Workshop` for productive buildings (`None` for hubs / commercial / campus),
/// seeded with 1 month of full-staffing working capital (`base:cash`) carved out of construction expenditure.
pub fn make_workshop(
    goods: &GoodCatalog,
    pop_catalog: &PopTypeCatalog,
    def: &BuildingDef,
    slot: TileSlotKey,
) -> Option<Workshop> {
    def.is_privatizable_industry().then(|| {
        let scale = i64::from(def.scale_for_slot(slot));
        let wage_seed = def.slot_monthly_wage_bill_cents(pop_catalog, slot);
        let input_seed: i64 = def
            .monthly_inputs
            .iter()
            .filter_map(|(gid_str, amt)| {
                goods
                    .id_of(gid_str)
                    .map(|gid| *amt * scale * goods.get(gid).genesis_price_cents)
            })
            .sum();
        let max_seed = def.slot_build_cost(slot) * CASH_SCALE;
        let seed_cash = (wage_seed + (input_seed * 3) / 2).min(max_seed.max(wage_seed));
        Workshop::new(goods, def.scale_for_slot(slot), seed_cash, 0)
    })
}

impl PlanetTiles {
    /// Generates a pure natural, uninhabited planet surface (`TileSlots::default()`, zero buildings/workshops).
    pub fn from_planet_seed(planet_id: &str, seed: u32) -> Self {
        Self::from_planet_seed_with_cells(planet_id, seed, surface::DEFAULT_GRID_CELLS)
    }

    pub fn from_planet_seed_with_cells(
        planet_id: &str,
        seed: u32,
        target_grid_cells: usize,
    ) -> Self {
        let generated =
            surface::generate_sim_surface_with_cells(planet_id, seed, target_grid_cells);
        let starter_tile_index = generated.starter_tile_index;

        let mut region_to_sector = vec![u16::MAX; generated.regions.len()];
        let mut next_sector: u16 = 0;
        for reg in &generated.regions {
            if !reg.is_ocean && reg.land_tile_count > 0 {
                region_to_sector[usize::from(reg.id)] = next_sector;
                next_sector += 1;
            }
        }

        let tiles: Vec<Tile> = generated
            .tiles
            .into_iter()
            .enumerate()
            .map(|(index, geo)| Tile {
                index: index as u16,
                region_id: geo.region_id,
                sector_id: region_to_sector
                    .get(usize::from(geo.region_id))
                    .copied()
                    .unwrap_or(u16::MAX),
                elevation: geo.elevation,
                tags: geo.tags,
                slots: TileSlots::default(),
            })
            .collect();

        let slot_owners = vec![[BuildingOwner::Unowned; SLOTS_PER_TILE]; tiles.len()];
        let slot_target_jobs = vec![[u32::MAX; SLOTS_PER_TILE]; tiles.len()];
        let slot_pay_wages = vec![[true; SLOTS_PER_TILE]; tiles.len()];
        let slot_contracts: Vec<[Vec<StaffContract>; SLOTS_PER_TILE]> = (0..tiles.len())
            .map(|_| std::array::from_fn(|_| Vec::new()))
            .collect();
        let workshops: Vec<[Option<Workshop>; SLOTS_PER_TILE]> =
            (0..tiles.len()).map(|_| [None, None, None, None]).collect();

        let sector_count = usize::from(next_sector);
        let tile_regions: Vec<u16> = tiles.iter().map(|t| t.region_id).collect();
        let tile_tags: Vec<TileTags> = tiles.iter().map(|t| t.tags).collect();
        let geo_topology = economy::GeoTransportTopology::compute(
            &generated.regions,
            &tile_regions,
            &tile_tags,
            &generated.tile_neighbors,
        );

        let mut pt = Self {
            grid_cell_target: target_grid_cells,
            regions: generated.regions,
            land_tile_count: generated.land_tile_count,
            sea_tile_count: generated.sea_tile_count,
            starter_tile_index,
            tiles,
            slot_owners,
            slot_target_jobs,
            slot_pay_wages,
            slot_contracts,
            workshops,
            tile_neighbors: generated.tile_neighbors,
            trade_links: Vec::new(),
            sector_coastal: Vec::new(),
            geo_topology,
            logistics: LogisticsMatrix::default(),
            enterprises: Vec::new(),
            trusts: Vec::new(),
            bilateral_contracts: Vec::new(),
            next_contract_id: 1,
        };
        pt.refresh_sector_topology(sector_count);
        pt
    }

    /// Generates an uninhabited planet where ALL tiles start as unclaimed wilderness (`sector_id = u16::MAX`),
    /// while precomputing the permanent `R × R` natural `geo_topology` once at world creation.
    pub fn from_unclaimed_planet_seed_with_cells(
        planet_id: &str,
        seed: u32,
        target_grid_cells: usize,
    ) -> Self {
        let mut pt = Self::from_planet_seed_with_cells(planet_id, seed, target_grid_cells);
        for tile in &mut pt.tiles {
            tile.sector_id = u16::MAX;
        }
        pt.refresh_sector_topology(0);
        pt
    }

    /// Recomputes `trade_links`, `sector_coastal`, and `logistics` for the active `Sector`s
    /// without ever touching the static planetary `geo_topology`.
    pub fn refresh_sector_topology(&mut self, sector_count: usize) {
        let tile_sectors: Vec<u16> = self.tiles.iter().map(|t| t.sector_id).collect();
        let tile_tags: Vec<TileTags> = self.tiles.iter().map(|t| t.tags).collect();
        let trade_links = economy::derive_sector_trade_links(
            &tile_sectors,
            &tile_tags,
            &self.tile_neighbors,
            sector_count,
        );
        let mut sector_coastal = vec![false; sector_count];
        for t in &self.tiles {
            let s = usize::from(t.sector_id);
            if s < sector_count && (t.tags.has(TileTag::Coastal) || t.tags.has(TileTag::CoastalSea))
            {
                sector_coastal[s] = true;
            }
        }
        let edges: Vec<(usize, usize, i64, i64)> = trade_links
            .iter()
            .map(|l| {
                (
                    usize::from(l.sector_a.0),
                    usize::from(l.sector_b.0),
                    l.monthly_capacity,
                    l.transport_fee,
                )
            })
            .collect();
        self.logistics = LogisticsMatrix::from_links(sector_count, &edges, &sector_coastal);
        self.trade_links = trade_links;
        self.sector_coastal = sector_coastal;
    }

    pub fn referenced_pop_ids_in_sector(&self, sector_id: SectorId) -> BTreeSet<PopId> {
        let mut set = BTreeSet::new();
        for tile in &self.tiles {
            if tile.sector_id != sector_id.0 {
                continue;
            }
            let t = usize::from(tile.index);
            for sk in TileSlotKey::ALL {
                for c in &self.slot_contracts[t][sk.as_index()] {
                    if c.count > 0 {
                        set.insert(c.pop_id);
                    }
                }
            }
        }
        set
    }

    /// Appends or merges a `(pop_id, count, monthly_bill_cents)` contract onto `(tile_index, slot)`.
    pub fn add_slot_contract(
        &mut self,
        tile_index: usize,
        slot: TileSlotKey,
        pop_id: PopId,
        count: u32,
        monthly_bill_cents: i64,
    ) {
        if count == 0 || tile_index >= self.slot_contracts.len() {
            return;
        }
        let list = &mut self.slot_contracts[tile_index][slot.as_index()];
        if let Some(existing) = list.iter_mut().find(|c| c.pop_id == pop_id) {
            existing.count += count;
            existing.monthly_bill_cents += monthly_bill_cents;
        } else {
            list.push(StaffContract {
                pop_id,
                count,
                monthly_bill_cents,
            });
        }
    }

    pub fn slot_owner(&self, tile_index: usize, slot: TileSlotKey) -> BuildingOwner {
        self.slot_owners
            .get(tile_index)
            .map(|arr| arr[slot.as_index()])
            .unwrap_or(BuildingOwner::Unowned)
    }

    pub fn set_slot_owner(&mut self, tile_index: usize, slot: TileSlotKey, owner: BuildingOwner) {
        if let Some(arr) = self.slot_owners.get_mut(tile_index) {
            arr[slot.as_index()] = owner;
        }
    }

    #[inline]
    pub fn effective_slot_target_jobs(
        &self,
        tile_index: usize,
        slot: TileSlotKey,
        def: &BuildingDef,
    ) -> u32 {
        let req = def.slot_jobs_required(slot);
        self.slot_target_jobs
            .get(tile_index)
            .map(|arr| arr[slot.as_index()].min(req))
            .unwrap_or(req)
    }

    #[inline]
    pub fn slot_honors_wages(&self, tile_index: usize, slot: TileSlotKey) -> bool {
        self.slot_pay_wages
            .get(tile_index)
            .map(|arr| arr[slot.as_index()])
            .unwrap_or(true)
    }

    pub fn reset_slot_directives(&mut self, tile_index: usize, slot: TileSlotKey) {
        if let Some(arr) = self.slot_target_jobs.get_mut(tile_index) {
            arr[slot.as_index()] = u32::MAX;
        }
        if let Some(arr) = self.slot_pay_wages.get_mut(tile_index) {
            arr[slot.as_index()] = true;
        }
    }

    /// Resolves where a building's liquidated cash should flow back to (strictly preserving
    /// three-vault separation between `Polity`, `SectorCapital`, `Enterprise`, and `Trust`).
    pub(crate) fn owner_cash_sink(&self, owner: BuildingOwner, ruling: PolityId, _fallback: SectorId) -> CashSink {
        match owner {
            BuildingOwner::LocalPrivate { home_sector } => CashSink::Capital(home_sector),
            BuildingOwner::Enterprise { enterprise_id } => CashSink::Enterprise(enterprise_id),
            BuildingOwner::Trust { trust_id } => CashSink::Trust(trust_id),
            _ => CashSink::Polity(ruling),
        }
    }

    pub fn check_uniqueness(
        &self,
        buildings: &BuildingCatalog,
        target_tile_index: usize,
        replacing_slot: Option<TileSlotKey>,
        def: &BuildingDef,
    ) -> Result<(), String> {
        let target_region = self
            .tiles
            .get(target_tile_index)
            .map(|t| t.region_id)
            .ok_or_else(|| format!("地块序号 {target_tile_index} 超出范围。"))?;

        if def.unique_planet_tags == BuildingTags::EMPTY
            && def.unique_region_tags == BuildingTags::EMPTY
            && def.unique_tile_tags == BuildingTags::EMPTY
        {
            return Ok(());
        }

        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                if t_idx == target_tile_index && replacing_slot == Some(slot_key) {
                    continue;
                }
                let existing = buildings.get(tid);
                if let Some(conflict) = existing.tags.first_overlap(def.unique_planet_tags) {
                    return Err(format!(
                        "全星球仅限建造 1 座「{}」（地块 #{:04} 已存在「{}」）。",
                        conflict.display_name(),
                        t_idx + 1,
                        existing.name
                    ));
                }
                if tile.region_id == target_region {
                    if let Some(conflict) = existing.tags.first_overlap(def.unique_region_tags) {
                        return Err(format!(
                            "每个大区仅限拥有 1 座「{}」（本大区地块 #{:04} 已有「{}」）。",
                            conflict.display_name(),
                            t_idx + 1,
                            existing.name
                        ));
                    }
                }
                if t_idx == target_tile_index {
                    if let Some(conflict) = existing.tags.first_overlap(def.unique_tile_tags) {
                        return Err(format!(
                            "同一地块仅限建造 1 座「{}」（已有「{}」）。",
                            conflict.display_name(),
                            existing.name
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn sector_cbd_hosting_stats(
        &self,
        buildings: &BuildingCatalog,
        sector_id: SectorId,
    ) -> (u16, u16, u16, u16) {
        let mut cbd_count = 0u16;
        let mut cbd_capacity = 0u16;
        let mut cbd_hosted = 0u16;
        let mut ag_private = 0u16;

        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                let def = buildings.get(tid);
                let owner = self.slot_owner(t_idx, slot_key);
                if tile.sector_id == sector_id.0 && def.tags.has(BuildingTag::Commercial) {
                    cbd_count += 1;
                    cbd_capacity += def.hosting_capacity;
                }
                if def.is_privatizable_industry() && !def.tags.has(BuildingTag::Commercial) {
                    if let BuildingOwner::LocalPrivate { home_sector } = owner {
                        if home_sector == sector_id {
                            cbd_hosted += 1;
                            if def.tags.has(BuildingTag::Agriculture) {
                                ag_private += 1;
                            }
                        }
                    }
                }
            }
        }
        (cbd_count, cbd_hosted, cbd_capacity, ag_private)
    }

    /// Finds the dominant industry tag among unincorporated `LocalPrivate` workshops in `sector_id`.
    pub fn dominant_private_industry_in_sector(
        &self,
        buildings: &BuildingCatalog,
        sector_id: SectorId,
        ignore_slot: Option<(usize, TileSlotKey)>,
    ) -> Option<(BuildingTag, u16)> {
        let mut counts: BTreeMap<&'static str, (BuildingTag, u16)> = BTreeMap::new();
        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                if ignore_slot == Some((t_idx, slot_key)) {
                    continue;
                }
                let def = buildings.get(tid);
                if !def.is_privatizable_industry() || def.tags.has(BuildingTag::Commercial) {
                    continue;
                }
                if matches!(
                    self.slot_owner(t_idx, slot_key),
                    BuildingOwner::LocalPrivate { home_sector } if home_sector == sector_id
                ) {
                    for tag in def.tags.iter() {
                        if matches!(
                            tag,
                            BuildingTag::PlanetaryCapital
                                | BuildingTag::RegionalCore
                                | BuildingTag::CorporateCampus
                                | BuildingTag::Commercial
                        ) {
                            continue;
                        }
                        let entry = counts.entry(tag.as_str()).or_insert((tag, 0));
                        entry.1 = entry.1.saturating_add(1);
                    }
                }
            }
        }
        counts
            .into_values()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.as_str().cmp(b.0.as_str())))
    }

    pub fn enterprise_hosted_count(
        &self,
        buildings: &BuildingCatalog,
        ent_id: EnterpriseId,
    ) -> u16 {
        let mut count = 0u16;
        for tile in &self.tiles {
            let t_idx = usize::from(tile.index);
            for (slot_key, tid) in tile.slots.iter_occupied() {
                let def = buildings.get(tid);
                if def.is_privatizable_industry()
                    && matches!(
                        self.slot_owner(t_idx, slot_key),
                        BuildingOwner::Enterprise { enterprise_id } if enterprise_id == ent_id
                    )
                {
                    count += 1;
                }
            }
        }
        count
    }
}

/// Destination of liquidated or transferred cash outside the monthly clearing cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CashSink {
    Polity(PolityId),
    Capital(SectorId),
    Enterprise(EnterpriseId),
    Trust(TrustId),
}

pub(crate) struct SectorSimView {
    pub(crate) sector_id: SectorId,
    pub(crate) pops: SectorPops,
}

pub(crate) struct SectorLaborStats {
    pub(crate) jobs_required: u32,
    pub(crate) jobs_filled: u32,
    pub(crate) employed_by_type: Vec<u32>,
    pub(crate) required_by_type: Vec<u32>,
    pub(crate) has_core: bool,
    pub(crate) building_apple_output: i64,
}

pub(crate) struct PlanetSimEvaluation {
    pub(crate) slot_employed: Vec<[u32; SLOTS_PER_TILE]>,
    pub(crate) slot_bonus_permille: Vec<[i64; SLOTS_PER_TILE]>,
    pub(crate) enterprise_active: Vec<bool>,
    pub(crate) sector_stats: Vec<SectorLaborStats>,
    /// Exact contracted `(headcount, monthly_bill_cents)` per `(sector_vec_idx, group_idx)`.
    pub(crate) pop_contracted: Vec<Vec<(u32, i64)>>,
}

/// Pure evaluation of existing locked `slot_contracts` & nominal output (no re-auction, no cash movement).
pub(crate) fn evaluate_planet_economy<'a>(
    buildings: &'a BuildingCatalog,
    catalog: &PopTypeCatalog,
    planet_tiles: &PlanetTiles,
    sectors: &[SectorSimView],
) -> PlanetSimEvaluation {
    let type_count = catalog.len();
    let sector_count = sectors.len();
    let tile_count = planet_tiles.tiles.len();
    let mut slot_employed = vec![[0u32; SLOTS_PER_TILE]; tile_count];
    let mut slot_bonus_permille = vec![[0i64; SLOTS_PER_TILE]; tile_count];
    let mut pop_contracted: Vec<Vec<(u32, i64)>> = sectors
        .iter()
        .map(|v| vec![(0u32, 0i64); v.pops.groups.len()])
        .collect();

    let mut sector_buckets: Vec<Vec<(usize, TileSlotKey, &'a BuildingDef)>> =
        vec![Vec::new(); sector_count];
    let mut sector_stats: Vec<SectorLaborStats> = (0..sector_count)
        .map(|_| SectorLaborStats {
            jobs_required: 0,
            jobs_filled: 0,
            employed_by_type: vec![0; type_count],
            required_by_type: vec![0; type_count],
            has_core: false,
            building_apple_output: 0,
        })
        .collect();

    for tile in &planet_tiles.tiles {
        let s_idx = usize::from(tile.sector_id);
        if s_idx >= sector_count {
            continue;
        }
        let t_idx = usize::from(tile.index);
        for (slot_key, tid) in tile.slots.iter_occupied() {
            let def = buildings.get(tid);
            if def.tags.has(BuildingTag::RegionalCore) {
                sector_stats[s_idx].has_core = true;
            }
            let scale = def.scale_for_slot(slot_key);
            for (role_str, base_req) in &def.jobs_by_role {
                let req = *base_req * scale;
                let pid = def.resolve_role_id(catalog, role_str);
                sector_stats[s_idx].jobs_required += req;
                sector_stats[s_idx].required_by_type[usize::from(pid.0)] += req;
            }

            let mut filled_sum = 0u32;
            for c in &planet_tiles.slot_contracts[t_idx][slot_key.as_index()] {
                if c.count == 0 {
                    continue;
                }
                if let Some(gi) = sectors[s_idx].pops.index_by_id(c.pop_id) {
                    let p_i = usize::from(sectors[s_idx].pops.groups[gi].pop_type.0);
                    if p_i < type_count {
                        sector_stats[s_idx].employed_by_type[p_i] += c.count;
                    }
                    pop_contracted[s_idx][gi].0 += c.count;
                    pop_contracted[s_idx][gi].1 += c.monthly_bill_cents;
                    filled_sum += c.count;
                }
            }
            slot_employed[t_idx][slot_key.as_index()] = filled_sum;
            sector_stats[s_idx].jobs_filled += filled_sum;
            sector_buckets[s_idx].push((t_idx, slot_key, def));
        }
    }

    let mut enterprise_active = vec![false; planet_tiles.enterprises.len()];
    for (e_idx, ent) in planet_tiles.enterprises.iter().enumerate() {
        if let Some(tile) = planet_tiles.tiles.get(ent.campus_tile_index) {
            if let Some(tid) = tile.slots.primary {
                let def = buildings.get(tid);
                if def.tags.has(BuildingTag::CorporateCampus) {
                    let req = def.slot_jobs_required(TileSlotKey::Primary);
                    let emp = slot_employed[ent.campus_tile_index][TileSlotKey::Primary.as_index()];
                    if req == 0 || emp * 2 >= req {
                        enterprise_active[e_idx] = true;
                    }
                }
            }
        }
    }

    for (s_idx, jobs) in sector_buckets.iter().enumerate() {
        for &(t_idx, slot_key, def) in jobs {
            let owner = planet_tiles.slot_owner(t_idx, slot_key);
            let bonus = match owner {
                BuildingOwner::Enterprise { enterprise_id }
                    if enterprise_active
                        .get(usize::from(enterprise_id.0))
                        .copied()
                        .unwrap_or(false) =>
                {
                    ENTERPRISE_OUTPUT_BONUS_PERMILLE
                }
                _ => 0,
            };
            slot_bonus_permille[t_idx][slot_key.as_index()] = bonus;
            sector_stats[s_idx].building_apple_output += slot_primary_output(
                def,
                slot_key,
                slot_employed[t_idx][slot_key.as_index()],
                bonus,
            );
        }
    }

    PlanetSimEvaluation {
        slot_employed,
        slot_bonus_permille,
        enterprise_active,
        sector_stats,
        pop_contracted,
    }
}


/// Effective monthly primary output units of one slot after scale, enterprise bonus and staffing.
pub(crate) fn slot_primary_output(
    def: &BuildingDef,
    slot: TileSlotKey,
    employed: u32,
    bonus_permille: i64,
) -> i64 {
    let scale = def.scale_for_slot(slot);
    let req = def.slot_jobs_required(slot);
    def.monthly_output
        .first()
        .map(|&(_, unit_delta)| {
            let base_out = unit_delta * i64::from(scale);
            let with_bonus = (base_out * (1000 + bonus_permille) + 500) / 1000;
            scale_output_by_staffing(with_bonus, employed, req)
        })
        .unwrap_or(0)
}
