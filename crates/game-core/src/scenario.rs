//! Hardcoded Scenario Bootstrap Layer (`开局剧本层` — 暂时集中存放地球初始世界、人口与建筑摆位预设).

use crate::building::{
    BuildingCatalog, BuildingDef, BuildingTag, SlotRule, TileSlotKey, BUILDING_APPLE_ORCHARD,
    BUILDING_COLONIAL_OUTPOST, BUILDING_COMMERCIAL_STREET, BUILDING_PLANETARY_CAPITAL,
    BUILDING_REGIONAL_CORE,
};
use crate::economy::{
    self, BuildingOwner, DepotPolicy, Polity, PolityId, PopTypeCatalog, PopTypeId, Sector,
    SectorCapital, SectorId, SectorMarket, SectorPops, Workshop,
};
use crate::market::{compute_per_100_demand, GoodCatalog, Inventory, CASH_SCALE};
use crate::planet::{Appearance, Planet, PlanetTiles, Position2d};
use crate::protocol::ScenarioPreset;
use crate::surface::TileTag;
use hecs::World;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

// ponytail: 临时默认地球开局初始化（把开局摆建筑、各区初始人口与家庭储蓄统一收拢在此，待后续剧本预设与 on_game_start 起源事件替换；本体表 PopTypeDef 与空白地表生成保持干净）。
fn temp_earth_role_preset(
    role_id: &str,
    is_default_pool: bool,
    is_vagrant_pool: bool,
    has_modern_utilities: bool,
) -> (u32, i64) {
    if is_vagrant_pool {
        return (15, 0);
    }
    match role_id {
        "base:laborer" if is_default_pool => {
            if has_modern_utilities {
                (520, 210)
            } else {
                (350, 210)
            }
        }
        "base:laborer" => (350, 50),
        "base:engineer" => {
            if has_modern_utilities {
                (135, 160)
            } else {
                (40, 120)
            }
        }
        "base:academic" => {
            if has_modern_utilities {
                (45, 160)
            } else {
                (40, 120)
            }
        }
        "base:bureaucrat" => {
            if has_modern_utilities {
                (115, 150)
            } else {
                (105, 150)
            }
        }
        "base:soldier" => {
            if has_modern_utilities {
                (45, 120)
            } else {
                (40, 100)
            }
        }
        "base:capitalist" => {
            if has_modern_utilities {
                (20, 300)
            } else {
                (15, 260)
            }
        }
        _ if is_default_pool => (0, 40),
        _ => (25, 200),
    }
}

fn apply_temp_earth_start(
    ruling_polity: PolityId,
    goods: &GoodCatalog,
    buildings: &BuildingCatalog,
    catalog: &PopTypeCatalog,
    planet_tiles: &mut PlanetTiles,
    sector_meta: &BTreeMap<u16, (u16, String, u32)>,
) -> BTreeMap<u16, SectorPops> {
    let starter_tile_index = planet_tiles.starter_tile_index;
    let starter_sector_u16 = planet_tiles
        .tiles
        .get(starter_tile_index)
        .map(|t| t.sector_id)
        .unwrap_or(0);
    let starter_sector = SectorId(starter_sector_u16);

    let cap_def = buildings
        .by_id(BUILDING_PLANETARY_CAPITAL)
        .expect("内置行星首府定义必须有效");
    let orchard_def = buildings
        .by_id(BUILDING_APPLE_ORCHARD)
        .ok()
        .or_else(|| {
            buildings
                .defs()
                .iter()
                .find(|d| d.tags.has(BuildingTag::Agriculture) && d.slot_rule == SlotRule::Any)
        })
        .expect("建筑目录必须至少包含一种农业产粮建筑");
    let cbd_def = buildings
        .by_id(BUILDING_COMMERCIAL_STREET)
        .expect("内置商业街定义必须有效");
    let pwr_def_opt = buildings
        .defs()
        .iter()
        .find(|d| d.tags.has(BuildingTag::Power) && d.monthly_inputs.is_empty());
    let has_modern_utilities = pwr_def_opt.is_some();

    if let Some(starter) = planet_tiles.tiles.get_mut(starter_tile_index) {
        starter.slots.primary = Some(cap_def.type_id);
        starter.slots.secondary[0] = Some(orchard_def.type_id);
        starter.slots.secondary[1] = Some(cbd_def.type_id);
        planet_tiles.slot_owners[starter_tile_index][TileSlotKey::Primary.as_index()] =
            BuildingOwner::State {
                polity_id: ruling_polity,
            };
        planet_tiles.slot_owners[starter_tile_index][TileSlotKey::Secondary0.as_index()] =
            BuildingOwner::State {
                polity_id: ruling_polity,
            };
        planet_tiles.slot_owners[starter_tile_index][TileSlotKey::Secondary1.as_index()] =
            BuildingOwner::LocalPrivate {
                home_sector: starter_sector,
            };
        let mut starter_farm_ws = Workshop::new(goods, 1, 60 * CASH_SCALE, 10);
        for (in_str, amt) in &orchard_def.monthly_inputs {
            if let Some(in_gid) = goods.id_of(in_str) {
                starter_farm_ws.inv.produce_capped(goods, in_gid, *amt);
            }
        }
        planet_tiles.workshops[starter_tile_index][TileSlotKey::Secondary0.as_index()] =
            Some(starter_farm_ws);
        if cbd_def.is_privatizable_industry() {
            let mut cbd_ws = Workshop::new(goods, 1, 45 * CASH_SCALE, 0);
            for (in_str, amt) in &cbd_def.monthly_inputs {
                if let Some(in_gid) = goods.id_of(in_str) {
                    cbd_ws.inv.produce_capped(goods, in_gid, *amt);
                }
            }
            planet_tiles.workshops[starter_tile_index][TileSlotKey::Secondary1.as_index()] =
                Some(cbd_ws);
        }
        if let Some(pwr_def) = pwr_def_opt {
            starter.slots.secondary[2] = Some(pwr_def.type_id);
            planet_tiles.slot_owners[starter_tile_index][TileSlotKey::Secondary2.as_index()] =
                BuildingOwner::LocalPrivate {
                    home_sector: starter_sector,
                };
            planet_tiles.workshops[starter_tile_index][TileSlotKey::Secondary2.as_index()] =
                Some(Workshop::new(goods, 1, 60 * CASH_SCALE, 0));
        }
    }

    let starter_land_tiles: Vec<usize> = {
        let mut v: Vec<usize> = planet_tiles
            .tiles
            .iter()
            .filter(|t| {
                usize::from(t.index) != starter_tile_index
                    && t.sector_id == starter_sector_u16
                    && t.tags.has(TileTag::Land)
            })
            .map(|t| usize::from(t.index))
            .collect();
        v.sort_by_key(|&idx| {
            (
                std::cmp::Reverse(planet_tiles.tiles[idx].tags.habitability_permille()),
                planet_tiles.tiles[idx].index,
            )
        });
        v
    };

    if let Some(&farm_idx) = starter_land_tiles.first() {
        planet_tiles.tiles[farm_idx].slots.primary = Some(orchard_def.type_id);
        planet_tiles.slot_owners[farm_idx][TileSlotKey::Primary.as_index()] =
            BuildingOwner::LocalPrivate {
                home_sector: starter_sector,
            };
        let mut farm_ws = Workshop::new(goods, 2, 160 * CASH_SCALE, 20);
        for (in_str, amt) in &orchard_def.monthly_inputs {
            if let Some(in_gid) = goods.id_of(in_str) {
                farm_ws.inv.produce_capped(goods, in_gid, *amt * 2);
            }
        }
        planet_tiles.workshops[farm_idx][TileSlotKey::Primary.as_index()] = Some(farm_ws);

        if let Some(water_def) = buildings.defs().iter().find(|d| {
            d.tags.has(BuildingTag::Water)
                && d.can_use_slot(TileSlotKey::Secondary0)
                && d.matches_tags(planet_tiles.tiles[farm_idx].tags)
        }) {
            planet_tiles.tiles[farm_idx].slots.secondary[0] = Some(water_def.type_id);
            planet_tiles.slot_owners[farm_idx][TileSlotKey::Secondary0.as_index()] =
                BuildingOwner::LocalPrivate {
                    home_sector: starter_sector,
                };
            let mut water_ws = Workshop::new(goods, 1, 75 * CASH_SCALE, 0);
            for (in_str, amt) in &water_def.monthly_inputs {
                if let Some(in_gid) = goods.id_of(in_str) {
                    water_ws.inv.produce_capped(goods, in_gid, *amt);
                }
            }
            planet_tiles.workshops[farm_idx][TileSlotKey::Secondary0.as_index()] = Some(water_ws);
        }
    }

    let default_pool = catalog.default_pool_id();
    let default_is_sub = catalog.get(default_pool).is_self_subsistence();
    let vag_opt = catalog
        .has_dedicated_vagrant_pool()
        .then(|| catalog.vagrant_pool_id());
    let staple_opt = goods.primary_staple();
    let staple_for = |cnt: u32| -> i64 {
        staple_opt
            .map(|(_, per_100)| compute_per_100_demand(cnt, per_100))
            .unwrap_or(0)
    };

    let starter_all: u32 = catalog
        .defs()
        .iter()
        .map(|d| {
            temp_earth_role_preset(
                &d.id,
                d.is_default_pool,
                d.is_vagrant_pool,
                has_modern_utilities,
            )
            .0
        })
        .sum();
    let starter_non_default: u32 = catalog
        .defs()
        .iter()
        .filter(|d| !d.is_default_pool)
        .map(|d| {
            temp_earth_role_preset(
                &d.id,
                d.is_default_pool,
                d.is_vagrant_pool,
                has_modern_utilities,
            )
            .0
        })
        .sum();
    let default_sav_base = temp_earth_role_preset(
        &catalog.get(default_pool).id,
        true,
        false,
        has_modern_utilities,
    )
    .1;

    let mut sector_pops: BTreeMap<u16, SectorPops> = BTreeMap::new();
    for (&sec_u16, &(_, _, hab_permille)) in sector_meta {
        let is_cap = sec_u16 == starter_sector_u16;
        let mut specs: Vec<(PopTypeId, u32, i64, i64)> = Vec::with_capacity(2);
        if is_cap {
            let total_target = if default_is_sub {
                let raw = 620 + (hab_permille * 38) / 1000;
                (((raw + 5) / 10) * 10).max(starter_non_default + 120)
            } else {
                starter_all
            };
            let vag_count = if vag_opt.is_some() {
                15.min(total_target)
            } else {
                0
            };
            let sub_count = total_target.saturating_sub(vag_count);
            specs.push((default_pool, sub_count, default_sav_base, staple_for(sub_count)));
            if let Some(vpid) = vag_opt {
                if vag_count > 0 {
                    specs.push((vpid, vag_count, 0, staple_for(vag_count)));
                }
            }
        } else {
            let raw = 95 + (hab_permille * 20) / 1000;
            let rural_min = if default_is_sub { 100 } else { 80 };
            let sub_count = (((raw + 5) / 10) * 10).max(rural_min);
            let starter_sav = if default_is_sub {
                default_sav_base
            } else {
                (default_sav_base * i64::from(sub_count) / 140).max(120)
            };
            let starter_food = if default_is_sub {
                staple_for(sub_count)
            } else {
                staple_for(sub_count) * 3
            };
            specs.push((default_pool, sub_count, starter_sav, starter_food));
        }
        sector_pops.insert(
            sec_u16,
            SectorPops::from_unowned_groups(goods, default_pool, vag_opt, &specs),
        );
    }

    let cash_id = goods.cash_id();
    let mut slots_by_sector: BTreeMap<u16, Vec<(usize, TileSlotKey, &BuildingDef, u16)>> =
        BTreeMap::new();
    for tile in &planet_tiles.tiles {
        if tile.sector_id == u16::MAX {
            continue;
        }
        let t_idx = usize::from(tile.index);
        let hab = tile.tags.habitability_permille();
        for (sk, tid) in tile.slots.iter_occupied() {
            slots_by_sector
                .entry(tile.sector_id)
                .or_default()
                .push((t_idx, sk, buildings.get(tid), hab));
        }
    }

    for (sec_u16, mut jobs) in slots_by_sector {
        let Some(pops) = sector_pops.get_mut(&sec_u16) else {
            continue;
        };
        jobs.sort_by(|a, b| {
            a.2.job_priority
                .cmp(&b.2.job_priority)
                .then_with(|| b.3.cmp(&a.3))
                .then_with(|| a.1.as_index().cmp(&b.1.as_index()))
                .then_with(|| a.0.cmp(&b.0))
        });

        let mut role_total_req = vec![0u32; catalog.len()];
        for &(_, sk, def, _) in &jobs {
            let scale = def.scale_for_slot(sk);
            for (role_str, base_req) in &def.jobs_by_role {
                let pid = def.resolve_role_id(catalog, role_str);
                role_total_req[usize::from(pid.0)] += *base_req * scale;
            }
        }

        for (t_idx, sk, def, _) in jobs {
            let owner = planet_tiles.slot_owner(t_idx, sk);
            let scale = def.scale_for_slot(sk);
            for (role_str, base_req) in &def.jobs_by_role {
                let req = *base_req * scale;
                if req == 0 {
                    continue;
                }
                let role_id = def.resolve_role_id(catalog, role_str);
                let r_def = catalog.get(role_id);
                let (_, role_sav_cr) = temp_earth_role_preset(
                    &r_def.id,
                    r_def.is_default_pool,
                    r_def.is_vagrant_pool,
                    has_modern_utilities,
                );
                let wage_per_worker_cents = r_def.base_wage_per_100 * CASH_SCALE / 100;

                let Some(src_idx) = pops.find_index(default_pool, 0, BuildingOwner::Unowned) else {
                    continue;
                };
                let avail = pops.groups[src_idx].count;
                let take = req.min(avail);
                if take == 0 {
                    continue;
                }
                let src_before = pops.groups[src_idx].count;
                let dst_idx = pops.ensure_group_index(goods, role_id, 0, owner);
                let dst_id = pops.groups[dst_idx].id;
                pops.groups[src_idx].count -= take;
                pops.groups[src_idx].sync_capacity();
                pops.groups[dst_idx].count += take;
                pops.groups[dst_idx].sync_capacity();

                if role_id == default_pool && !r_def.is_self_subsistence() {
                    let src_cash = pops.groups[src_idx].inv.available[usize::from(cash_id.0)];
                    let carry_cash = if take == src_before {
                        src_cash
                    } else {
                        ((i128::from(src_cash) * i128::from(take)) / i128::from(src_before.max(1)))
                            as i64
                    };
                    pops.groups[src_idx].inv.available[usize::from(cash_id.0)] -= carry_cash;
                    pops.groups[dst_idx].inv.available[usize::from(cash_id.0)] += carry_cash;
                } else {
                    let tot_role = role_total_req[usize::from(role_id.0)].max(1);
                    let add_cash_cents =
                        (role_sav_cr * CASH_SCALE * i64::from(take)) / i64::from(tot_role);
                    pops.groups[dst_idx].inv.available[usize::from(cash_id.0)] += add_cash_cents;
                }
                if let Some((sid, per_100)) = staple_opt {
                    let add_food = compute_per_100_demand(take, per_100);
                    pops.groups[dst_idx].inv.produce_capped(goods, sid, add_food);
                }

                let bill = i64::from(take) * wage_per_worker_cents;
                planet_tiles.add_slot_contract(t_idx, sk, dst_id, take, bill);
            }
        }
        for (idx, r_def) in catalog.defs().iter().enumerate() {
            let role_id = PopTypeId(idx as u16);
            if r_def.is_default_pool || r_def.is_vagrant_pool {
                continue;
            }
            let (cap_target, role_sav_cr) = temp_earth_role_preset(
                &r_def.id,
                r_def.is_default_pool,
                r_def.is_vagrant_pool,
                has_modern_utilities,
            );
            let used = role_total_req[idx];
            if used > 0 && cap_target > used {
                let extra_want = cap_target - used;
                let Some(src_idx) = pops.find_index(default_pool, 0, BuildingOwner::Unowned) else {
                    break;
                };
                let take = extra_want.min(pops.groups[src_idx].count);
                if take > 0 {
                    let dst_idx =
                        pops.ensure_group_index(goods, role_id, 0, BuildingOwner::Unowned);
                    pops.groups[src_idx].count -= take;
                    pops.groups[src_idx].sync_capacity();
                    pops.groups[dst_idx].count += take;
                    pops.groups[dst_idx].sync_capacity();

                    let denom = cap_target.max(1);
                    let add_cash_cents =
                        (role_sav_cr * CASH_SCALE * i64::from(take)) / i64::from(denom);
                    pops.groups[dst_idx].inv.available[usize::from(cash_id.0)] += add_cash_cents;
                    if let Some((sid, per_100)) = staple_opt {
                        let add_food = compute_per_100_demand(take, per_100);
                        pops.groups[dst_idx].inv.produce_capped(goods, sid, add_food);
                    }
                }
            }
        }
        if let Some(sub_idx) = pops.find_index(default_pool, 0, BuildingOwner::Unowned) {
            if let Some((sid, per_100)) = staple_opt {
                let target_food = compute_per_100_demand(pops.groups[sub_idx].count, per_100);
                pops.groups[sub_idx].inv.available[usize::from(sid.0)] =
                    target_food.min(pops.groups[sub_idx].inv.capacity);
            }
        }
    }

    sector_pops
}

/// Hardcoded initial Earth world bootstrap (isolated from engine mechanisms).
pub(crate) fn seed_earth_scenario_world(
    goods: &GoodCatalog,
    buildings: &BuildingCatalog,
    pop_catalog: &PopTypeCatalog,
    earth_grid_cells: usize,
) -> World {
        let mut world = World::new();

        let earth_polity = Polity::earth_default(&goods);
        let earth_polity_id = earth_polity.id;
        world.spawn((earth_polity,));

        let earth_id = "base:earth";
        let earth_seed = 42u32;
        let mut earth_tiles =
            PlanetTiles::from_planet_seed_with_cells(earth_id, earth_seed, earth_grid_cells);

        let starter_sector_u16 = earth_tiles
            .tiles
            .get(earth_tiles.starter_tile_index)
            .map(|t| t.sector_id)
            .unwrap_or(0);

        let mut sector_meta: BTreeMap<u16, (u16, String, u32)> = BTreeMap::new();
        for tile in &earth_tiles.tiles {
            if tile.sector_id == u16::MAX {
                continue;
            }
            let reg_name = earth_tiles
                .regions
                .get(usize::from(tile.region_id))
                .map(|r| r.name.clone())
                .unwrap_or_else(|| format!("{:02} · 陆地辖区", tile.sector_id + 1));
            let entry = sector_meta
                .entry(tile.sector_id)
                .or_insert((tile.region_id, reg_name, 0));
            if tile.tags.has(TileTag::Land) {
                entry.2 += u32::from(tile.tags.habitability_permille());
            }
        }

        let mut sector_pops_map = apply_temp_earth_start(
            earth_polity_id,
            &goods,
            &buildings,
            &pop_catalog,
            &mut earth_tiles,
            &sector_meta,
        );

        for (sec_u16, (geo_region_id, name, hab_permille)) in sector_meta {
            let is_cap = sec_u16 == starter_sector_u16;

            let sector = Sector {
                id: SectorId(sec_u16),
                planet_id: earth_id.into(),
                geo_region_id,
                owner_polity: earth_polity_id,
                admin_polity: earth_polity_id,
                foreign_investment_rights: BTreeSet::new(),
                name,
                allow_private_land_use: true,
                habitability_sum_permille: hab_permille,
                is_coastal: earth_tiles
                    .sector_coastal
                    .get(usize::from(sec_u16))
                    .copied()
                    .unwrap_or(false),
                state_depot: Inventory::new(goods.len(), economy::BASE_SECTOR_DEPOT_CAPACITY),
                depot_policy: DepotPolicy::Idle,
            };
            let market = SectorMarket::new(&goods, &pop_catalog);
            let capital = SectorCapital::new(&goods, if is_cap { 320 } else { 140 });
            let pops = sector_pops_map.remove(&sec_u16).unwrap();
            world.spawn((sector, market, capital, pops));
        }

        world.spawn((
            Planet {
                id: earth_id.into(),
                name: "地球".into(),
                radius_km: 6371.0,
                ruling_polity: earth_polity_id,
            },
            Position2d { x: 0.0, y: 0.0 },
            Appearance {
                ocean_rgb: [44, 101, 139],
                land_rgb: [115, 145, 125],
                seed: earth_seed,
            },
            earth_tiles,
        ));
        world
}

pub(crate) fn seed_scenario_world(
    scenario: ScenarioPreset,
    goods: &GoodCatalog,
    buildings: &BuildingCatalog,
    pop_catalog: &PopTypeCatalog,
    grid_cells: usize,
) -> World {
    match scenario {
        ScenarioPreset::Earth => seed_earth_scenario_world(goods, buildings, pop_catalog, grid_cells),
        ScenarioPreset::Colony => {
            seed_colony_scenario_world(goods, buildings, pop_catalog, grid_cells)
        }
    }
}

/// Colonial frontier start (`殖民地开局`):
/// - Procedural habitable world (`base:colony`, 12 land macro-regions + 4 oceans, auto-marked `core_site` hubs);
/// - Only the landing `CoreSite` tile + 8 contiguous adjacent land tiles (`9` land tiles total) start claimed
///   into `SectorId(0)` under `星际拓荒公署`; all other ~330+ land tiles remain unclaimed wilderness (`u16::MAX`);
/// - Landing foothold starts with a self-sustaining pioneer outpost & farm (`殖民据点` + `1x 现代农场`),
///   staffed by a ~208-person pioneer expedition with starter working capital, rations & savings.
pub(crate) fn seed_colony_scenario_world(
    goods: &GoodCatalog,
    buildings: &BuildingCatalog,
    pop_catalog: &PopTypeCatalog,
    grid_cells: usize,
) -> World {
    let mut world = World::new();

    let colony_polity = Polity::colony_default(goods);
    let ruling_id = colony_polity.id;
    world.spawn((colony_polity,));

    let colony_id = "base:colony";
    let colony_seed = 2026_0419u32;
    let mut pt = PlanetTiles::from_unclaimed_planet_seed_with_cells(colony_id, colony_seed, grid_cells);

    let starter_idx = pt.starter_tile_index;
    let start_region = pt
        .tiles
        .get(starter_idx)
        .map(|t| t.region_id)
        .unwrap_or(0);

    // BFS-claim a compact, contiguous 9-tile foothold around `starter_idx` within `start_region`.
    let target_foothold = 9usize;
    let mut claimed: Vec<usize> = Vec::with_capacity(target_foothold);
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    if starter_idx < pt.tiles.len() && pt.tiles[starter_idx].tags.has(TileTag::Land) {
        visited.insert(starter_idx);
        queue.push_back(starter_idx);
    }
    while let Some(cur) = queue.pop_front() {
        if claimed.len() >= target_foothold {
            break;
        }
        claimed.push(cur);
        let mut nbs: Vec<usize> = pt.tile_neighbors[cur]
            .iter()
            .map(|&u| usize::from(u))
            .filter(|&nb| {
                nb < pt.tiles.len()
                    && !visited.contains(&nb)
                    && pt.tiles[nb].region_id == start_region
                    && pt.tiles[nb].tags.has(TileTag::Land)
            })
            .collect();
        nbs.sort_by_key(|&nb| std::cmp::Reverse(pt.tiles[nb].tags.habitability_permille()));
        for nb in nbs {
            if visited.insert(nb) {
                queue.push_back(nb);
            }
        }
    }

    let mut hab_sum_permille = 0u32;
    let mut is_coastal = false;
    for &idx in &claimed {
        pt.tiles[idx].sector_id = 0;
        hab_sum_permille += u32::from(pt.tiles[idx].tags.habitability_permille());
        if pt.tiles[idx].tags.has(TileTag::Coastal) {
            is_coastal = true;
            let nbs = pt.tile_neighbors[idx].clone();
            for nb_u16 in nbs {
                let nb = usize::from(nb_u16);
                if nb < pt.tiles.len()
                    && pt.tiles[nb].region_id == start_region
                    && pt.tiles[nb].tags.has(TileTag::CoastalSea)
                {
                    pt.tiles[nb].sector_id = 0;
                }
            }
        }
    }
    pt.refresh_sector_topology(1);

    let outpost_def = buildings
        .by_id(BUILDING_COLONIAL_OUTPOST)
        .or_else(|_| buildings.by_id(BUILDING_REGIONAL_CORE))
        .expect("建筑目录必须包含殖民据点或大区核心");
    let farm_def = buildings
        .by_id(BUILDING_APPLE_ORCHARD)
        .ok()
        .or_else(|| {
            buildings
                .defs()
                .iter()
                .find(|d| d.tags.has(BuildingTag::Agriculture) && d.slot_rule == SlotRule::Any)
        })
        .expect("建筑目录必须包含农业建筑");

    if starter_idx < pt.tiles.len() {
        pt.tiles[starter_idx].slots.primary = Some(outpost_def.type_id);
        pt.slot_owners[starter_idx][TileSlotKey::Primary.as_index()] = BuildingOwner::State {
            polity_id: ruling_id,
        };

        pt.tiles[starter_idx].slots.secondary[0] = Some(farm_def.type_id);
        pt.slot_owners[starter_idx][TileSlotKey::Secondary0.as_index()] =
            BuildingOwner::LocalPrivate {
                home_sector: SectorId(0),
            };
        let mut farm_ws = Workshop::new(goods, 1, 180 * CASH_SCALE, 10);
        for (in_str, amt) in &farm_def.monthly_inputs {
            if let Some(in_gid) = goods.id_of(in_str) {
                farm_ws.inv.produce_capped(goods, in_gid, *amt * 2);
            }
        }
        pt.workshops[starter_idx][TileSlotKey::Secondary0.as_index()] = Some(farm_ws);
    }

    let default_pool = pop_catalog.default_pool_id();
    let vag_opt = pop_catalog
        .has_dedicated_vagrant_pool()
        .then(|| pop_catalog.vagrant_pool_id());
    let staple_opt = goods.primary_staple();
    let cash_id = goods.cash_id();

    let free_pioneers = 49u32;
    let free_food = staple_opt
        .map(|(_, per_100)| compute_per_100_demand(free_pioneers, per_100) * 2)
        .unwrap_or(0);
    let mut pops = SectorPops::from_unowned_groups(
        goods,
        default_pool,
        vag_opt,
        &[(default_pool, free_pioneers, 160, free_food)],
    );
    // Seed a compact pioneer specialist/entrepreneur reserve (`34 + 6 + 4 = 44 > 40` for frontier migration).
    for (role_str, extra_cnt, sav_cr) in [
        ("base:engineer", 6u32, 70i64),
        ("base:bureaucrat", 4u32, 45i64),
        ("base:capitalist", 4u32, 160i64),
    ] {
        if let Some(rid) = pop_catalog.id_of(role_str) {
            if rid != default_pool {
                let gi = pops.ensure_group_index(goods, rid, 0, BuildingOwner::Unowned);
                pops.groups[gi].count += extra_cnt;
                pops.groups[gi].sync_capacity();
                pops.groups[gi].inv.available[usize::from(cash_id.0)] += sav_cr * CASH_SCALE;
                if let Some((sid, per_100)) = staple_opt {
                    let fd = compute_per_100_demand(extra_cnt, per_100);
                    pops.groups[gi].inv.produce_capped(goods, sid, fd);
                }
            }
        }
    }

    // Lock initial `StaffContract`s for every placed starter slot in `claimed`.
    for &t_idx in &claimed {
        for sk in TileSlotKey::ALL {
            let Some(tid) = pt.tiles[t_idx].slots.get(sk) else {
                continue;
            };
            let bdef = buildings.get(tid);
            let owner = pt.slot_owners[t_idx][sk.as_index()];
            let scale = bdef.scale_for_slot(sk);
            for (role_str, base_req) in &bdef.jobs_by_role {
                let req = *base_req * scale;
                if req == 0 {
                    continue;
                }
                let rid = bdef.resolve_role_id(pop_catalog, role_str);
                let wage_per_worker_cents =
                    (pop_catalog.get(rid).base_wage_per_100 * CASH_SCALE / 100).max(15);
                let gi = pops.ensure_group_index(goods, rid, 0, owner);
                let pid = pops.groups[gi].id;
                pops.groups[gi].count += req;
                pops.groups[gi].sync_capacity();
                pops.groups[gi].inv.available[usize::from(cash_id.0)] +=
                    i64::from(req) * wage_per_worker_cents * 3;
                if let Some((sid, per_100)) = staple_opt {
                    let fd = compute_per_100_demand(req, per_100);
                    pops.groups[gi].inv.produce_capped(goods, sid, fd);
                }
                let bill = i64::from(req) * wage_per_worker_cents;
                pt.add_slot_contract(t_idx, sk, pid, req, bill);
            }
        }
    }

    let base_reg_name = pt
        .regions
        .get(usize::from(start_region))
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "01 · 先遣着陆区".into());
    let sector = Sector {
        id: SectorId(0),
        planet_id: colony_id.into(),
        geo_region_id: start_region,
        owner_polity: ruling_id,
        admin_polity: ruling_id,
        foreign_investment_rights: BTreeSet::new(),
        name: base_reg_name,
        allow_private_land_use: true,
        habitability_sum_permille: hab_sum_permille,
        is_coastal,
        state_depot: Inventory::new(goods.len(), economy::BASE_SECTOR_DEPOT_CAPACITY),
        depot_policy: DepotPolicy::Idle,
    };
    let market = SectorMarket::new(goods, pop_catalog);
    let capital = SectorCapital::new(goods, 180);
    world.spawn((sector, market, capital, pops));

    world.spawn((
        Planet {
            id: colony_id.into(),
            name: "新泰拉殖民星".into(),
            radius_km: 6120.0,
            ruling_polity: ruling_id,
        },
        Position2d { x: 0.0, y: 0.0 },
        Appearance {
            ocean_rgb: [36, 92, 134],
            land_rgb: [108, 146, 118],
            seed: colony_seed,
        },
        pt,
    ));

    world
}
