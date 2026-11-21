//! Read-Only View Projection & Snapshot Assembly (`只读视图投影与快照装配层`).

use crate::building::{BuildingCatalog, BuildingDef, BuildingTag, TileSlotKey, SECONDARY_SLOT_COUNT};
use crate::calendar::{DAYS_PER_MONTH, DAYS_PER_YEAR, SUBTICKS_PER_DAY};
use crate::economy::{
    compute_primary_staple_demand, BuildingOwner, BuildingOwnerSnapshot, Enterprise,
    EnterpriseSnapshot, GoodInfoSnapshot, Polity, PolitySnapshot, PopGroupSnapshot, PopTypeCatalog,
    PopTypeDef, RoleWageSnapshot, Sector, SectorCapital, SectorGoodBookSnapshot, SectorId,
    SectorMarket, SectorPops, SectorSnapshot, BASE_UNINCORPORATED_PRIVATE_CAP,
    ENTERPRISE_FOUNDING_MIN_PRIVATE, ENTERPRISE_OUTPUT_BONUS_PERMILLE,
};
use crate::market::{self, cents_to_credits_round, GoodCatalog, GoodId, CASH_SCALE};
use crate::planet::{
    evaluate_planet_economy, Appearance, Planet, PlanetTiles, Position2d, SectorSimView,
};
use crate::polity_ops::compute_polity_admin_budgets;
use crate::protocol::{
    GameSnapshot, OccupiedTileSnapshot, PlacedBuildingSnapshot, PlanetSnapshot,
    RegionSummarySnapshot, ResourceStockSnapshot, ScenarioPreset, TimeSnapshot,
};
use crate::surface::{self, PlanetSurfaceSnapshot, TileTag};
use crate::Game;
use hecs::World;
use std::collections::BTreeMap;

impl Game {
    pub fn time(&self) -> TimeSnapshot {
        let elapsed_days = self.elapsed_subticks / SUBTICKS_PER_DAY;
        let subtick = (self.elapsed_subticks % SUBTICKS_PER_DAY) as u8;
        let elapsed_years = (elapsed_days / DAYS_PER_YEAR) as i32;
        let day_of_year = elapsed_days % DAYS_PER_YEAR;
        let month = ((day_of_year / DAYS_PER_MONTH) + 1) as u8;
        let day = ((day_of_year % DAYS_PER_MONTH) + 1) as u8;

        let raw_year = self.calendar.start_year + elapsed_years;
        let year = if self.calendar.start_year < 0 && raw_year >= 0 {
            raw_year + 1
        } else {
            raw_year
        };

        let formatted = if year < 0 {
            format!("前{:04}.{:02}.{:02}", -year, month, day)
        } else {
            format!("{:04}.{:02}.{:02}", year, month, day)
        };

        TimeSnapshot {
            scenario: self.scenario,
            epoch_name: self.calendar.epoch_name.clone(),
            year,
            month,
            day,
            subtick,
            formatted,
            elapsed_days,
            speed: self.speed,
            active_combats: self.active_combat_count(),
        }
    }

    pub fn default_planet_id(&self) -> Option<String> {
        self.world
            .query::<&Planet>()
            .iter()
            .map(|(_, planet)| planet.id.as_str())
            .min()
            .map(str::to_owned)
    }

    pub fn planet_surface_by_id(&self, target_id: &str) -> Option<PlanetSurfaceSnapshot> {
        self.world
            .query::<(&Planet, &Appearance, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (planet, appearance, pt))| {
                (planet.id == target_id).then(|| {
                    let cells = if pt.grid_cell_target == 0 {
                        surface::DEFAULT_GRID_CELLS
                    } else {
                        pt.grid_cell_target
                    };
                    surface::generate_planet_surface_with_cells(&planet.id, appearance.seed, cells)
                })
            })
    }

    pub fn polities(&self) -> Vec<PolitySnapshot> {
        let budgets = compute_polity_admin_budgets(
            &self.world,
            &self.goods,
            &self.buildings,
            &self.pop_catalog,
        );
        let mut list: Vec<PolitySnapshot> = self
            .world
            .query::<&Polity>()
            .iter()
            .map(|(_, p)| {
                let (used, cap) = budgets.get(&p.id).copied().unwrap_or((0, p.base_admin_capacity));
                p.to_snapshot_with_admin(&self.goods, used, cap)
            })
            .collect();
        list.sort_by_key(|p| p.id);
        list
    }

    pub fn planet_by_id(&self, target_id: &str) -> Option<PlanetSnapshot> {
        self.world
            .query::<(&Planet, &Position2d, &Appearance, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (planet, position, appearance, planet_tiles))| {
                (planet.id == target_id).then(|| {
                    build_planet_snapshot(
                        self.scenario,
                        &self.world,
                        &self.buildings,
                        &self.pop_catalog,
                        &self.goods,
                        planet,
                        position,
                        appearance,
                        planet_tiles,
                    )
                })
            })
    }

    pub fn planets(&self) -> Vec<PlanetSnapshot> {
        let mut planets: Vec<_> = self
            .world
            .query::<(&Planet, &Position2d, &Appearance, &PlanetTiles)>()
            .iter()
            .map(|(_, (planet, position, appearance, planet_tiles))| {
                build_planet_snapshot(
                    self.scenario,
                    &self.world,
                    &self.buildings,
                    &self.pop_catalog,
                    &self.goods,
                    planet,
                    position,
                    appearance,
                    planet_tiles,
                )
            })
            .collect();
        planets.sort_by(|a, b| a.id.cmp(&b.id));
        planets
    }

    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot {
            time: self.time(),
            polities: self.polities(),
            planets: self.planets(),
            pending_events: self.pending_events.clone(),
        }
    }
}

fn employer_kind_and_label(
    employer: BuildingOwner,
    current_sector: SectorId,
    sector_names: &BTreeMap<u16, String>,
    enterprises: &[Enterprise],
) -> (String, String) {
    match employer {
        BuildingOwner::Unowned => ("unowned".into(), String::new()),
        BuildingOwner::State { .. } => ("state".into(), "国有".into()),
        BuildingOwner::LocalPrivate { home_sector } => {
            if home_sector == current_sector {
                ("local_private".into(), "民间私产".into())
            } else {
                let short = sector_names
                    .get(&home_sector.0)
                    .and_then(|s| s.split(" · ").next())
                    .unwrap_or("外区");
                ("local_private".into(), format!("{short}区私产"))
            }
        }
        BuildingOwner::Enterprise { enterprise_id } => {
            let ent_name = enterprises
                .get(usize::from(enterprise_id.0))
                .map(|e| e.name.as_str())
                .unwrap_or("本土企业");
            ("enterprise".into(), ent_name.to_string())
        }
        BuildingOwner::Trust { trust_id } => {
            ("trust".into(), format!("托拉斯#{}", trust_id.0 + 1))
        }
    }
}

fn describe_slot_owner(
    owner: BuildingOwner,
    def: &BuildingDef,
    slot: TileSlotKey,
    sector_names: &BTreeMap<u16, String>,
    enterprises: &[Enterprise],
) -> BuildingOwnerSnapshot {
    let can_priv = def.is_privatizable_industry() && matches!(owner, BuildingOwner::State { .. });
    let can_nat = def.is_privatizable_industry()
        && matches!(
            owner,
            BuildingOwner::LocalPrivate { .. } | BuildingOwner::Enterprise { .. }
        );
    let priv_price = (def.slot_build_cost(slot) * 80) / 100;
    let nat_cost = def.slot_build_cost(slot);

    match owner {
        BuildingOwner::State { .. } | BuildingOwner::Unowned => BuildingOwnerSnapshot {
            kind: "state".into(),
            label: "国有".into(),
            home_sector_id: None,
            enterprise_id: None,
            can_privatize: can_priv,
            privatize_price: priv_price,
            can_nationalize: false,
            nationalize_cost: 0,
        },
        BuildingOwner::LocalPrivate { home_sector } => {
            let short = sector_names
                .get(&home_sector.0)
                .and_then(|s| s.split(" · ").next())
                .unwrap_or("本地");
            BuildingOwnerSnapshot {
                kind: "local_private".into(),
                label: format!("民间私产（{short}区商业街）"),
                home_sector_id: Some(home_sector.0),
                enterprise_id: None,
                can_privatize: false,
                privatize_price: 0,
                can_nationalize: can_nat,
                nationalize_cost: nat_cost,
            }
        }
        BuildingOwner::Enterprise { enterprise_id } => {
            let ent_name = enterprises
                .get(usize::from(enterprise_id.0))
                .map(|e| e.name.as_str())
                .unwrap_or("本土企业");
            BuildingOwnerSnapshot {
                kind: "enterprise".into(),
                label: ent_name.to_string(),
                home_sector_id: enterprises
                    .get(usize::from(enterprise_id.0))
                    .map(|e| e.hq_sector.0),
                enterprise_id: Some(enterprise_id.0),
                can_privatize: false,
                privatize_price: 0,
                can_nationalize: can_nat,
                nationalize_cost: nat_cost,
            }
        }
        BuildingOwner::Trust { trust_id } => BuildingOwnerSnapshot {
            kind: "trust".into(),
            label: format!("星际托拉斯 #{}", trust_id.0 + 1),
            home_sector_id: None,
            enterprise_id: None,
            can_privatize: false,
            privatize_price: 0,
            can_nationalize: true,
            nationalize_cost: nat_cost,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn build_planet_snapshot(
    scenario: ScenarioPreset,
    world: &World,
    buildings: &BuildingCatalog,
    pop_catalog: &PopTypeCatalog,
    goods: &GoodCatalog,
    planet: &Planet,
    position: &Position2d,
    appearance: &Appearance,
    planet_tiles: &PlanetTiles,
) -> PlanetSnapshot {
    let admin_budgets = compute_polity_admin_budgets(world, goods, buildings, pop_catalog);
    let (ruling_used, ruling_cap) = admin_budgets
        .get(&planet.ruling_polity)
        .copied()
        .unwrap_or((0, 0));
    let polity_snap = world
        .query::<&Polity>()
        .iter()
        .find_map(|(_, pol)| {
            (pol.id == planet.ruling_polity)
                .then(|| pol.to_snapshot_with_admin(goods, ruling_used, ruling_cap))
        })
        .unwrap_or_else(|| Polity::earth_default(goods).to_snapshot(goods));

    let mut raw_sectors: Vec<(Sector, SectorMarket, SectorCapital, SectorPops)> = world
        .query::<(&Sector, &SectorMarket, &SectorCapital, &SectorPops)>()
        .iter()
        .filter(|(_, (s, _, _, _))| s.planet_id == planet.id)
        .map(|(_, (s, m, c, p))| (s.clone(), m.clone(), c.clone(), p.clone()))
        .collect();
    raw_sectors.sort_by_key(|(s, _, _, _)| s.id);

    let sec_views: Vec<SectorSimView> = raw_sectors
        .iter()
        .map(|(s, _, _, p)| SectorSimView {
            sector_id: s.id,
            pops: p.clone(),
        })
        .collect();

    let eval = evaluate_planet_economy(buildings, pop_catalog, planet_tiles, &sec_views);

    let mut sector_names = BTreeMap::new();
    for (s, _, _, _) in &raw_sectors {
        sector_names.insert(s.id.0, s.name.clone());
    }

    // Physical goods held anywhere on the planet (workshops, state depots, Pop pantries).
    // `monthly_delta` = last month's production - consumption (spoilage excluded).
    let ruling_vault = world
        .query::<&Polity>()
        .iter()
        .find_map(|(_, pol)| (pol.id == planet.ruling_polity).then(|| pol.vault.clone()));
    let mut stockpile: BTreeMap<String, i64> = BTreeMap::new();
    let mut resources = Vec::new();
    let mut good_catalog = Vec::new();
    for (g_i, gdef) in goods.defs().iter().enumerate() {
        if gdef.is_currency {
            continue;
        }
        good_catalog.push(GoodInfoSnapshot {
            id: gdef.id.clone(),
            name: gdef.name.clone(),
            unit_volume: gdef.unit_volume,
            retention_permille: gdef.retention_permille,
            transport_channel: gdef.transport_channel,
            genesis_price_cents: gdef.genesis_price_cents,
            is_flow: gdef.retention_permille == 0 || gdef.non_market,
        });
        if gdef.non_market {
            continue;
        }
        let gid = GoodId(g_i as u16);
        let mut amount = ruling_vault.as_ref().map(|v| v.total(gid)).unwrap_or(0);
        for row in &planet_tiles.workshops {
            for ws in row.iter().flatten() {
                amount += ws.inv.total(gid);
            }
        }
        for ent in &planet_tiles.enterprises {
            amount += ent.vault.total(gid);
        }
        for tr in &planet_tiles.trusts {
            amount += tr.vault.total(gid);
        }
        for (s, _, c, p) in &raw_sectors {
            amount += s.state_depot.total(gid) + c.vault.total(gid);
            amount += p.groups.iter().map(|g| g.inv.total(gid)).sum::<i64>();
        }
        let monthly_delta = if goods.primary_staple().map(|(gid_s, _)| gid_s) == Some(gid) {
            raw_sectors
                .iter()
                .map(|(_, m, _, _)| m.last_production + m.last_subsistence_output - m.last_consumption)
                .sum()
        } else {
            0
        };
        stockpile.insert(gdef.id.clone(), amount);
        resources.push(ResourceStockSnapshot {
            id: gdef.id.clone(),
            name: gdef.name.clone(),
            amount,
            monthly_delta,
        });
    }
    let primary_staple_id = goods
        .primary_staple()
        .map(|(gid, _)| gid)
        .or_else(|| goods.tradable_good_ids().into_iter().next())
        .unwrap_or(GoodId(0));
    let primary_staple_def = goods.get(primary_staple_id);
    let primary_staple_id_str = primary_staple_def.id.clone();
    let primary_staple_name_str = primary_staple_def.name.clone();

    let pop_catalog_snap = pop_catalog
        .defs()
        .iter()
        .map(PopTypeDef::to_snapshot)
        .collect();
    let building_catalog = buildings.defs().iter().map(BuildingDef::to_snapshot).collect();

    let mut total_population = 0u32;
    let mut monthly_population_growth = 0u32;
    let mut total_jobs_required = 0u32;
    let mut total_jobs_filled = 0u32;

    let default_pool_id = pop_catalog.default_pool_id();
    let vag_pool_id = pop_catalog.vagrant_pool_id();
    let has_vag_pool = pop_catalog.has_dedicated_vagrant_pool();
    let mut sectors_snap: Vec<SectorSnapshot> = Vec::with_capacity(raw_sectors.len());
    let mut region_sector_map: BTreeMap<u16, usize> = BTreeMap::new();

    for (s_idx, (sec, market, cap, pops)) in raw_sectors.iter().enumerate() {
        let stats = &eval.sector_stats[s_idx];
        let growth = pops.monthly_growth(sec.habitability_sum_permille, stats.has_core);
        let pop_total = pops.total_population();

        total_population += pop_total;
        monthly_population_growth += growth;
        total_jobs_required += stats.jobs_required;
        total_jobs_filled += stats.jobs_filled;

        let (cbd_count, cbd_hosted, cbd_capacity, ag_private) =
            planet_tiles.sector_cbd_hosting_stats(buildings, sec.id);

        let mut pop_groups_snap: Vec<PopGroupSnapshot> = pops
            .groups
            .iter()
            .enumerate()
            .filter(|&(gi, g)| {
                let (c_emp, _) = eval.pop_contracted[s_idx].get(gi).copied().unwrap_or((0, 0));
                g.count > 0
                    || c_emp > 0
                    || g.inv.total(goods.cash_id()) > 0
                    || g.inv.total(primary_staple_id) > 0
                    || (g.culture_id == 0
                        && g.employer == BuildingOwner::Unowned
                        && g.pop_type == default_pool_id)
            })
            .map(|(gi, g)| {
                let def = pop_catalog.get(g.pop_type);
                let (c_emp, c_bill) = eval.pop_contracted[s_idx].get(gi).copied().unwrap_or((0, 0));
                let employed = if def.is_self_subsistence() && g.employer == BuildingOwner::Unowned {
                    g.count
                } else if def.is_vagrant_pool || g.employer == BuildingOwner::Unowned {
                    0
                } else {
                    c_emp.min(g.count)
                };
                let contract_wage_per_100_cents = if c_emp > 0 {
                    (c_bill * 100) / i64::from(c_emp)
                } else {
                    0
                };
                let (employer_kind, mut employer_label) = employer_kind_and_label(
                    g.employer,
                    sec.id,
                    &sector_names,
                    &planet_tiles.enterprises,
                );
                if g.employer == BuildingOwner::Unowned
                    && !def.is_self_subsistence()
                    && !def.is_vagrant_pool
                {
                    employer_label = "待业".into();
                }
                let mut name = def.name.clone();
                if !employer_label.is_empty() {
                    name.push_str(" · ");
                    name.push_str(&employer_label);
                }
                if g.culture_id > 0 {
                    name.push_str(&format!("·族群{}", g.culture_id + 1));
                }
                PopGroupSnapshot {
                    pop_id: g.id.0,
                    id: def.id.clone(),
                    culture_id: g.culture_id,
                    employer_kind,
                    employer_label,
                    name,
                    count: g.count,
                    employed,
                    contract_wage_per_100_cents,
                    savings: cents_to_credits_round(g.inv.total(goods.cash_id())),
                    pantry_apples: g.inv.total(primary_staple_id),
                    pantry_capacity: g.inv.capacity,
                    satisfaction: g.satisfaction,
                    last_income: cents_to_credits_round(g.last_income_cents),
                    food_met_percent: (g.last_food_met_permille + 5) / 10,
                }
            })
            .collect();
        let employer_order = |k: &str| match k {
            "unowned" => 0u8,
            "state" => 1,
            "local_private" => 2,
            "enterprise" => 3,
            _ => 4,
        };
        pop_groups_snap.sort_by(|a, b| {
            pop_catalog
                .id_of(&a.id)
                .cmp(&pop_catalog.id_of(&b.id))
                .then_with(|| employer_order(&a.employer_kind).cmp(&employer_order(&b.employer_kind)))
                .then_with(|| a.employer_label.cmp(&b.employer_label))
                .then_with(|| a.culture_id.cmp(&b.culture_id))
                .then_with(|| a.pop_id.cmp(&b.pop_id))
        });

        let wage_quotes: Vec<RoleWageSnapshot> = pop_catalog
            .defs()
            .iter()
            .enumerate()
            .filter(|(_, d)| d.base_wage_per_100 > 0)
            .map(|(i, d)| {
                let wb = market.wage_books.get(i);
                let last_c = wb
                    .map(|b| b.last_wage_cents)
                    .unwrap_or(d.base_wage_per_100 * CASH_SCALE / 100);
                let ma_c = wb
                    .map(|b| b.ma_wage_cents)
                    .unwrap_or(d.base_wage_per_100 * CASH_SCALE / 100);
                RoleWageSnapshot {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    wage_per_100_cents: last_c * 100,
                    ma_wage_per_100_cents: ma_c * 100,
                    last_hired: wb.map(|b| b.last_hired).unwrap_or(0),
                    last_laid_off: wb.map(|b| b.last_laid_off).unwrap_or(0),
                }
            })
            .collect();

        let vagrant_count = if has_vag_pool {
            pops.count_of(vag_pool_id)
        } else {
            0
        };
        let unemployed_count: u32 = pops
            .groups
            .iter()
            .filter(|g| {
                let d = pop_catalog.get(g.pop_type);
                g.employer == BuildingOwner::Unowned
                    && !d.is_self_subsistence()
                    && !d.receives_dividends
            })
            .map(|g| g.count)
            .sum();

        // Count active solvent vacancies (excluding overstocked/bankrupt slots that voluntarily paused hiring).
        let mut active_vacancies = 0u32;
        for tile in &planet_tiles.tiles {
            if tile.sector_id != sec.id.0 {
                continue;
            }
            let t_idx = usize::from(tile.index);
            for (sk, tid) in tile.slots.iter_occupied() {
                if !planet_tiles.slot_honors_wages(t_idx, sk) {
                    continue;
                }
                let def = buildings.get(tid);
                let req = planet_tiles.effective_slot_target_jobs(t_idx, sk, def);
                let emp = eval.slot_employed[t_idx][sk.as_index()];
                if emp >= req {
                    continue;
                }
                let vac = req - emp;
                if let Some(ws) = planet_tiles.workshops[t_idx][sk.as_index()].as_ref() {
                    let fill_pm = (ws.inv.used_volume(goods) * 1000) / ws.inv.capacity.max(1);
                    if !ws.had_wage_arrears
                        && ws.total_unpaid_claims_cents() == 0
                        && fill_pm < 900
                        && ws.inv.avail(goods.cash_id()) > 0
                    {
                        active_vacancies += vac;
                    }
                } else if def.is_state_core_or_public() {
                    if polity_snap.treasury_cents > 0 {
                        active_vacancies += vac;
                    }
                } else {
                    let solvent = match planet_tiles.slot_owner(t_idx, sk) {
                        BuildingOwner::Enterprise { enterprise_id } => planet_tiles
                            .enterprises
                            .get(usize::from(enterprise_id.0))
                            .map(|e| e.cash_cents(goods) > 0)
                            .unwrap_or(false),
                        BuildingOwner::Trust { trust_id } => planet_tiles
                            .trusts
                            .get(usize::from(trust_id.0))
                            .map(|t| t.cash_cents(goods) > 0)
                            .unwrap_or(false),
                        _ => cap.pool_cents(goods) > 0,
                    };
                    if solvent {
                        active_vacancies += vac;
                    }
                }
            }
        }

        let staple_book = market
            .book(primary_staple_id)
            .unwrap_or_else(|| market.apple_book());
        let market_books: Vec<SectorGoodBookSnapshot> = goods
            .tradable_good_ids()
            .into_iter()
            .map(|gid| {
                let gdef = goods.get(gid);
                let order_book = market
                    .closing_depth(gid)
                    .cloned()
                    .or_else(|| market.book(gid).map(|b| b.to_snapshot(5)))
                    .unwrap_or_else(|| {
                        market::SectorOrderBook::new(gid, gdef.genesis_price_cents).to_snapshot(5)
                    });
                SectorGoodBookSnapshot {
                    good_id: gdef.id.clone(),
                    good_name: gdef.name.clone(),
                    depot_stock: sec.state_depot.total(gid),
                    order_book,
                }
            })
            .collect();
        let idle_pool_count = if pop_catalog.get(default_pool_id).is_self_subsistence() {
            pops.count_of(default_pool_id)
        } else {
            0
        };
        let shown_cbd_cap = cbd_capacity.max(cbd_hosted).max(BASE_UNINCORPORATED_PRIVATE_CAP);
        let dom_priv_cnt = planet_tiles
            .dominant_private_industry_in_sector(buildings, sec.id, None)
            .map(|(_, c)| c)
            .unwrap_or(ag_private);

        region_sector_map.insert(sec.geo_region_id, sectors_snap.len());
        sectors_snap.push(SectorSnapshot {
            id: sec.id.0,
            geo_region_id: sec.geo_region_id,
            owner_polity_id: sec.owner_polity.0,
            name: sec.name.clone(),
            allow_private_land_use: sec.allow_private_land_use,
            population: pop_total,
            monthly_growth: growth,
            jobs_required: stats.jobs_required,
            jobs_filled: stats.jobs_filled,
            free_labor: pop_total.saturating_sub(stats.jobs_filled),
            idle_pool_count,
            unemployed_count,
            vagrant_count,
            active_vacancies,
            average_satisfaction: pops.average_satisfaction(),
            apple_price: cents_to_credits_round(staple_book.last_price_cents),
            apple_price_cents: staple_book.last_price_cents,
            ma_apple_price_cents: staple_book.ma_price_cents,
            order_book: market
                .closing_depth(primary_staple_id)
                .cloned()
                .unwrap_or_else(|| staple_book.to_snapshot(5)),
            market_books,
            wage_quotes,
            depot_apples: sec.state_depot.total(primary_staple_id),
            depot_capacity: sec.state_depot.capacity,
            depot_policy: sec.depot_policy,
            depot_policy_label: sec.depot_policy.label().to_string(),
            local_apple_stockpile: sec.state_depot.total(primary_staple_id),
            apple_supply: market.last_production + market.last_subsistence_output,
            apple_demand: compute_primary_staple_demand(goods, pop_total),
            net_trade_in: staple_book.last_net_import,
            investment_pool: cents_to_credits_round(cap.pool_cents(goods)),
            last_dividends: cents_to_credits_round(cap.last_dividends_cents),
            cbd_count,
            cbd_hosted,
            cbd_capacity: shown_cbd_cap,
            unincorporated_ag_private: dom_priv_cnt,
            can_found_enterprise: usize::from(dom_priv_cnt) >= ENTERPRISE_FOUNDING_MIN_PRIVATE,
            pops: pop_groups_snap,
        });
    }

    let enterprises_snap: Vec<EnterpriseSnapshot> = planet_tiles
        .enterprises
        .iter()
        .enumerate()
        .map(|(e_i, ent)| EnterpriseSnapshot {
            id: ent.id.0,
            name: ent.name.clone(),
            industry_tag: ent.industry_tag.clone(),
            hq_sector_id: ent.hq_sector.0,
            campus_tile_index: ent.campus_tile_index,
            hosted_count: planet_tiles.enterprise_hosted_count(buildings, ent.id),
            hosting_capacity: ent.hosting_capacity,
            output_bonus_percent: if eval.enterprise_active.get(e_i).copied().unwrap_or(false) {
                (ENTERPRISE_OUTPUT_BONUS_PERMILLE / 10) as u16
            } else {
                0
            },
            treasury: cents_to_credits_round(ent.cash_cents(goods)),
            credit_rating: ent.credit_rating,
            last_revenue: cents_to_credits_round(ent.last_revenue_cents),
            last_profit: cents_to_credits_round(ent.last_profit_cents),
        })
        .collect();

    let mut region_summaries: Vec<RegionSummarySnapshot> = planet_tiles
        .regions
        .iter()
        .map(|r| {
            if let Some(&sec_vec_idx) = region_sector_map.get(&r.id) {
                let s = &sectors_snap[sec_vec_idx];
                RegionSummarySnapshot {
                    region_id: r.id,
                    sector_id: Some(s.id),
                    claimed_land_tiles: 0,
                    population: s.population,
                    monthly_growth: s.monthly_growth,
                    jobs_required: s.jobs_required,
                    jobs_filled: s.jobs_filled,
                    free_labor: s.free_labor,
                    primary_used: 0,
                    primary_total: r.tile_count,
                    land_primary_used: 0,
                    land_primary_total: r.land_tile_count,
                    secondary_used: 0,
                    secondary_total: r.tile_count.saturating_mul(SECONDARY_SLOT_COUNT as u16),
                    unlocked_secondary_empty: 0,
                    core_tile_index: None,
                    core_building_name: None,
                    building_counts: BTreeMap::new(),
                }
            } else {
                RegionSummarySnapshot {
                    region_id: r.id,
                    sector_id: None,
                    claimed_land_tiles: 0,
                    population: 0,
                    monthly_growth: 0,
                    jobs_required: 0,
                    jobs_filled: 0,
                    free_labor: 0,
                    primary_used: 0,
                    primary_total: r.tile_count,
                    land_primary_used: 0,
                    land_primary_total: r.land_tile_count,
                    secondary_used: 0,
                    secondary_total: r.tile_count.saturating_mul(SECONDARY_SLOT_COUNT as u16),
                    unlocked_secondary_empty: 0,
                    core_tile_index: None,
                    core_building_name: None,
                    building_counts: BTreeMap::new(),
                }
            }
        })
        .collect();

    let mut capital_tile_index: Option<usize> = None;
    let mut total_building_count = 0usize;
    let mut occupied_tiles = Vec::new();
    let mut tile_sectors = Vec::with_capacity(planet_tiles.tiles.len());

    for tile in &planet_tiles.tiles {
        let tile_idx = usize::from(tile.index);
        let claimed = tile.sector_id != u16::MAX;
        tile_sectors.push(claimed.then_some(tile.sector_id));
        if let Some(summary) = region_summaries.get_mut(usize::from(tile.region_id)) {
            if claimed && tile.tags.has(TileTag::Land) {
                summary.claimed_land_tiles += 1;
            }
            if let Some(tid) = tile.slots.primary {
                summary.primary_used += 1;
                if tile.tags.has(TileTag::Land) {
                    summary.land_primary_used += 1;
                }
                let def = buildings.get(tid);
                *summary.building_counts.entry(def.id.clone()).or_insert(0) += 1;
                if def.tags.has(BuildingTag::PlanetaryCapital) && capital_tile_index.is_none() {
                    capital_tile_index = Some(tile_idx);
                }
                if def.tags.has(BuildingTag::RegionalCore) && summary.core_tile_index.is_none() {
                    summary.core_tile_index = Some(tile_idx);
                    summary.core_building_name = Some(def.name.clone());
                }
            }
            for &sec in &tile.slots.secondary {
                if let Some(tid) = sec {
                    summary.secondary_used += 1;
                    let def = buildings.get(tid);
                    *summary.building_counts.entry(def.id.clone()).or_insert(0) += 1;
                } else if claimed && tile.tags.has(TileTag::Land) {
                    summary.unlocked_secondary_empty += 1;
                }
            }
        }

        if !tile.slots.is_empty() {
            total_building_count += tile.slots.used_count();
            let slot_emp = eval.slot_employed[tile_idx];
            let slot_bonus = eval.slot_bonus_permille[tile_idx];

            let build_slot_snap = |sk: TileSlotKey| -> Option<PlacedBuildingSnapshot> {
                tile.slots.get(sk).map(|tid| {
                    let def = buildings.get(tid);
                    let owner = describe_slot_owner(
                        planet_tiles.slot_owner(tile_idx, sk),
                        def,
                        sk,
                        &sector_names,
                        &planet_tiles.enterprises,
                    );
                    def.to_placed_snapshot(
                        sk,
                        slot_emp[sk.as_index()],
                        slot_bonus[sk.as_index()],
                        owner,
                    )
                })
            };

            occupied_tiles.push(OccupiedTileSnapshot {
                index: tile_idx,
                region_id: tile.region_id,
                sector_id: (tile.sector_id != u16::MAX).then_some(tile.sector_id),
                tags: tile.tags,
                habitability: tile.tags.habitability_percent(),
                primary: build_slot_snap(TileSlotKey::Primary),
                secondary: [
                    build_slot_snap(TileSlotKey::Secondary0),
                    build_slot_snap(TileSlotKey::Secondary1),
                    build_slot_snap(TileSlotKey::Secondary2),
                ],
            });
        }
    }

    PlanetSnapshot {
        scenario,
        id: planet.id.clone(),
        name: planet.name.clone(),
        radius_km: planet.radius_km,
        position: *position,
        appearance: *appearance,
        polity: polity_snap,
        primary_staple_id: primary_staple_id_str,
        primary_staple_name: primary_staple_name_str,
        good_catalog,
        pop_catalog: pop_catalog_snap,
        stockpile,
        resources,
        building_catalog,
        tile_count: planet_tiles.tiles.len(),
        land_tile_count: planet_tiles.land_tile_count,
        sea_tile_count: planet_tiles.sea_tile_count,
        region_count: planet_tiles.regions.len(),
        starter_tile_index: planet_tiles.starter_tile_index,
        capital_tile_index,
        total_population,
        monthly_population_growth,
        total_jobs_required,
        total_jobs_filled,
        total_building_count,
        sectors: sectors_snap,
        enterprises: enterprises_snap,
        region_summaries,
        tile_sectors,
        occupied_tiles,
    }
}
