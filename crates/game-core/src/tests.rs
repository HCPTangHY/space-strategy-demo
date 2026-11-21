use super::*;
use std::collections::BTreeMap;
use std::num::NonZeroU16;
use std::time::Duration;

    #[test]
    fn initial_earth_has_polity_sectors_data_driven_pops_and_compact_tile() {
        assert!(
            std::mem::size_of::<Tile>() <= 24,
            "Tile struct should stay <= 24 bytes in ECS, got {}",
            std::mem::size_of::<Tile>()
        );

        let default_game = Game::default();
        let default_earth = &default_game.snapshot().planets[0];
        assert_eq!(default_earth.good_catalog.len(), 19);
        assert_eq!(default_earth.building_catalog.len(), 26);
        assert_eq!(default_earth.primary_staple_id, "base:agri_food");
        for sec in &default_earth.sectors {
            assert_eq!(sec.market_books.len(), 18);
        }

        let game = Game::with_legacy_apple_catalog();
        let snap = game.snapshot();
        assert_eq!(snap.polities.len(), 1);
        assert_eq!(snap.polities[0].treasury, 2_600);

        let earth = &snap.planets[0];
        assert_eq!(earth.id, "base:earth");
        assert_eq!(earth.sectors.len(), surface::EARTH_LAND_REGIONS);
        assert!(
            earth.sectors.iter().any(|s| s.name.contains("东亚"))
                && earth.sectors.iter().any(|s| s.name.contains("青藏高原"))
                && earth.sectors.iter().any(|s| s.name.contains("大洋区")),
            "Earth must feature the 13 configured macro-regions"
        );
        let starter_reg_id = default_game
            .planet_surface_by_id("base:earth")
            .unwrap()
            .tiles[default_earth.starter_tile_index]
            .region_id;
        assert_eq!(
            starter_reg_id, 0,
            "Earth default starter capital must sit in 01 · 东亚, got {starter_reg_id}"
        );
        assert_eq!(earth.pop_catalog.len(), 7);
        assert!(
            earth.pop_catalog.iter().all(|p| p.id != "base:subsistence"),
            "modern catalog must not contain thin-air subsistence pop"
        );
        assert_eq!(earth.occupied_tiles.len(), 2);
        assert_eq!(earth.total_building_count, 4);
        assert_eq!(earth.total_jobs_required, 540);
        assert_eq!(earth.total_jobs_filled, 540);

        // Verify unified single-count Pop invariant: sector population == sum of PopGroup counts (no dependents split!).
        for sec in &earth.sectors {
            let sum_counts: u32 = sec.pops.iter().map(|g| g.count).sum();
            assert_eq!(sec.population, sum_counts);
        }

        // Verify cylindrical horizontal wrap-around (`x = 0 <-> SURFACE_WIDTH`) in both tile neighbors & trade distances!
        let surf = default_game.planet_surface_by_id("base:earth").unwrap();
        let has_dateline_neighbor = surf.tiles.iter().any(|t| {
            t.center[0] > 1100.0
                && t.neighbors
                    .iter()
                    .any(|&nb| surf.tiles[usize::from(nb)].center[0] < 100.0)
        });
        assert!(
            has_dateline_neighbor,
            "东侧边缘地块（x > 1100）必须直接与西侧边缘地块（x < 100）跨日界线相邻"
        );
    }

    #[test]
    fn custom_pop_type_catalog_runs_purely_from_data() {
        let blank_planet = PlanetTiles::from_planet_seed("mod:blank_world", 7);
        assert!(
            blank_planet
                .tiles
                .iter()
                .all(|t| t.slots.iter_occupied().next().is_none()),
            "PlanetTiles::from_planet_seed 必须生成零建筑的纯自然空白星球"
        );

        let custom_defs = vec![
            PopTypeDef {
                id: "mod:freeholder".into(),
                name: "自由拓荒民".into(),
                base_wage_per_100: 0,
                subsistence_apple_per_100: 8,
                is_default_pool: true,
                is_vagrant_pool: false,
                receives_dividends: false,
                can_demote: false,
                demote_to: None,
                expected_sol: 3,
                redemption_tier: 3,
                reserve_months: 2,
                surplus_service_permille: 150,
                surplus_invest_permille: 0,
                promote_from: Vec::new(),
            },
            PopTypeDef {
                id: "base:laborer".into(),
                name: "合成劳工".into(),
                base_wage_per_100: 50,
                subsistence_apple_per_100: 0,
                is_default_pool: false,
                is_vagrant_pool: false,
                receives_dividends: false,
                can_demote: true,
                demote_to: None,
                expected_sol: 5,
                redemption_tier: 3,
                reserve_months: 2,
                surplus_service_permille: 200,
                surplus_invest_permille: 100,
                promote_from: vec!["mod:freeholder".into()],
            },
            PopTypeDef {
                id: "mod:guild_shareholder".into(),
                name: "行会股东".into(),
                base_wage_per_100: 0,
                subsistence_apple_per_100: 0,
                is_default_pool: false,
                is_vagrant_pool: false,
                receives_dividends: true,
                can_demote: false,
                demote_to: None,
                expected_sol: 15,
                redemption_tier: 2,
                reserve_months: 3,
                surplus_service_permille: 200,
                surplus_invest_permille: 400,
                promote_from: vec!["mod:freeholder".into()],
            },
        ];
        let catalog = PopTypeCatalog::from_defs(custom_defs).unwrap();
        let mut game = Game::with_pop_catalog(CalendarConfig::default(), catalog);
        let earth0 = game.planet_by_id("base:earth").unwrap();
        assert_eq!(earth0.pop_catalog.len(), 3);
        assert!(!earth0.sectors[0].pops.is_empty());
        assert_eq!(earth0.sectors[0].pops[0].name, "自由拓荒民");

        for _ in 0..30 {
            game.step_once();
        }
        let earth1 = game.planet_by_id("base:earth").unwrap();
        assert!(earth1.total_population > earth0.total_population);
    }

    #[test]
    fn primary_first_rule_2x_dominant_scale_and_redevelopment_work() {
        let mut game = Game::with_legacy_apple_catalog();
        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_region = surf.tiles[earth0.starter_tile_index].region_id;

        let other_region = surf
            .regions
            .iter()
            .find(|r| !r.is_ocean && r.id != starter_region)
            .unwrap()
            .id;
        let empty_land_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| t.region_id == other_region && t.tags.has(TileTag::Land))
                .unwrap()
                .index,
        );

        // 1. Prerequisite-bound secondary building (`商业街`, `required_primary_tags != EMPTY`)
        //    is STILL blocked on an empty primary slot, while general `Any` industry (`苹果园`)
        //    can be built directly in `Secondary0` at `1x` scale (`150 CR`)!
        assert!(game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_COMMERCIAL_STREET,
                Some(TileSlotKey::Secondary0),
            )
            .is_err());

        let treasury_before = game.planet_by_id("base:earth").unwrap().polity.treasury;
        let after_sec_first = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_APPLE_ORCHARD,
                Some(TileSlotKey::Secondary0),
            )
            .unwrap();
        assert_eq!(after_sec_first.polity.treasury, treasury_before - 150);

        let after_primary = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_APPLE_ORCHARD,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        assert_eq!(after_primary.polity.treasury, treasury_before - 150 - 300);

        // `商业街` still fails when `Primary` is `苹果园` (missing `regional_core` / `corporate_campus`).
        assert!(game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_COMMERCIAL_STREET,
                Some(TileSlotKey::Secondary1),
            )
            .is_err());

        // Independent secondary `苹果园` does NOT block demolishing `Primary`!
        game.demolish_on_tile("base:earth", empty_land_tile, TileSlotKey::Primary)
            .unwrap();

        let after_redev = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_REGIONAL_CORE,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        let redev_occ = after_redev
            .occupied_tiles
            .iter()
            .find(|t| t.index == empty_land_tile)
            .unwrap();
        assert_eq!(
            redev_occ.primary.as_ref().unwrap().id,
            BUILDING_REGIONAL_CORE
        );

        let with_cbd = game
            .build_on_tile(
                "base:earth",
                empty_land_tile,
                BUILDING_COMMERCIAL_STREET,
                Some(TileSlotKey::Secondary1),
            )
            .unwrap();
        let cbd_occ = with_cbd
            .occupied_tiles
            .iter()
            .find(|t| t.index == empty_land_tile)
            .unwrap();
        assert_eq!(
            cbd_occ.secondary[1].as_ref().unwrap().id,
            BUILDING_COMMERCIAL_STREET
        );
        // Demolishing `Primary` while prerequisite-bound `商业街` relies on it MUST be blocked!
        assert!(game
            .demolish_on_tile("base:earth", empty_land_tile, TileSlotKey::Primary)
            .is_err());
    }

    #[test]
    fn privatization_enterprise_campus_founding_and_nationalization_work() {
        let mut game = Game::with_legacy_apple_catalog();
        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let cap_region = surf.tiles[starter_idx].region_id;

        game.build_on_tile(
            "base:earth",
            starter_idx,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary2),
        )
        .unwrap();

        game.privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        for _ in 0..60 {
            game.step_once();
        }
        let after_priv2 = game
            .privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary2)
            .unwrap();

        let cap_sec_id = after_priv2.region_summaries[usize::from(cap_region)]
            .sector_id
            .unwrap();
        let cap_sec = after_priv2
            .sectors
            .iter()
            .find(|s| s.id == cap_sec_id)
            .unwrap();
        assert!(cap_sec.can_found_enterprise);

        let occupied_set: std::collections::HashSet<usize> = after_priv2
            .occupied_tiles
            .iter()
            .map(|t| t.index)
            .collect();
        let campus_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| {
                    t.region_id == cap_region
                        && t.tags.has(TileTag::Land)
                        && !occupied_set.contains(&usize::from(t.index))
                })
                .unwrap()
                .index,
        );

        let after_campus = game
            .build_on_tile(
                "base:earth",
                campus_tile,
                BUILDING_CORPORATE_CAMPUS,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        assert_eq!(after_campus.enterprises.len(), 1);
        assert!(after_campus.enterprises[0].hosted_count >= 3);

        let after_nat = game
            .nationalize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        let starter_occ = after_nat
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap();
        assert_eq!(starter_occ.secondary[0].as_ref().unwrap().owner.kind, "state");
    }

    #[test]
    fn monthly_cycle_and_calendar_work() {
        let mut game = Game::with_calendar(CalendarConfig::new("纪元", -1).unwrap());
        assert_eq!(game.time().formatted, "前0001.01.01");
        let initial_pop = game.planet_by_id("base:earth").unwrap().total_population;
        for _ in 0..360 {
            game.step_once();
        }
        assert_eq!(game.time().year, 1);
        assert_eq!(game.time().formatted, "0001.01.01");
        let after_year = game.planet_by_id("base:earth").unwrap();
        assert!(after_year.total_population > initial_pop);
        assert!(after_year.stockpile[&after_year.primary_staple_id] > 0);
    }

    #[test]
    fn monthly_settlement_conserves_cash_except_minting() {
        let mut game = Game::default();
        let ruling = PolityId(0);
        for month in 0..6 {
            let before = game.planet_money_supply_cents("base:earth");
            game.settle_planet_month("base:earth", ruling);
            let after = game.planet_money_supply_cents("base:earth");
            let minted = game
                .world
                .query::<&Polity>()
                .iter()
                .map(|(_, p)| p.last_seigniorage_cents)
                .sum::<i64>();
            assert_eq!(after, before + minted, "cash leaked in month {month}");
        }
        // No inventory may hold locked balances between settlements.
        for (_, (_, pops)) in game.world.query::<(&Sector, &SectorPops)>().iter() {
            for g in &pops.groups {
                assert!(g.inv.locked.iter().all(|&x| x == 0));
            }
        }
        let earth = game.planet_by_id("base:earth").unwrap();
        let traded: i64 = earth
            .sectors
            .iter()
            .map(|s| s.order_book.last_contract_volume + s.order_book.last_spot_volume)
            .sum();
        assert!(traded > 0, "apples should change hands on the order books");
    }

    #[test]
    fn multi_culture_pops_promotion_carries_savings_and_full_warehouse_clears() {
        let mut game = Game::with_legacy_apple_catalog();
        let goods = game.goods().clone();
        let pop_cat = game.pop_catalog().clone();
        let cash_id = goods.cash_id();
        let apple_id = goods.id_of(GOOD_APPLE).unwrap();
        let vag_id = pop_cat.id_of(POP_VAGRANT).unwrap();
        let lab_id = pop_cat.id_of(POP_LABORER).unwrap();

        // 1. Verify promotion carries proportional cash & food and preserves non-zero `culture_id`!
        let mut pops = SectorPops {
            groups: vec![PopGroup::with_identity(
                &goods,
                PopId(1),
                vag_id,
                2,
                BuildingOwner::Unowned,
                100,
                200,
                10,
            )],
            next_pop_id: 2,
            last_growth: 0,
        };
        let total_cash_before = pops.groups[0].inv.total(cash_id);
        let total_food_before = pops.groups[0].inv.total(apple_id);
        let moved = pops.promote_into(&goods, &pop_cat, lab_id, 40);
        assert_eq!(moved, 40);
        assert_eq!(pops.groups.len(), 2, "must spawn `(laborer, culture=2)` row dynamically");
        assert_eq!(pops.groups[1].pop_type, lab_id);
        assert_eq!(pops.groups[1].culture_id, 2);
        assert_eq!(pops.groups[0].count, 60);
        assert_eq!(pops.groups[1].count, 40);
        assert_eq!(
            pops.groups[1].inv.total(cash_id),
            (total_cash_before * 40) / 100,
            "promoted workers must carry 40% of savings"
        );
        assert_eq!(
            pops.groups[0].inv.total(cash_id) + pops.groups[1].inv.total(cash_id),
            total_cash_before
        );
        assert_eq!(
            pops.groups[0].inv.total(apple_id) + pops.groups[1].inv.total(apple_id),
            total_food_before
        );

        // 2. Inject a second culture row (`culture_id = 1`) into Capital Sector on Earth and verify
        //    both cultures coexist, get hired via Labor Order Book, and conserve cash across settlements!
        let cap_sec_u16 = game
            .planet_by_id("base:earth")
            .unwrap()
            .occupied_tiles[0]
            .sector_id
            .unwrap();
        for (_, (sec, spops)) in game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == SectorId(cap_sec_u16) {
                spops.push_group_with_culture(
                    &goods,
                    lab_id,
                    1,
                    BuildingOwner::Unowned,
                    80,
                    120,
                    4,
                );
                break;
            }
        }
        let snap_multi = game.planet_by_id("base:earth").unwrap();
        let cap_sec_snap = snap_multi
            .sectors
            .iter()
            .find(|s| s.id == cap_sec_u16)
            .unwrap();
        assert!(
            cap_sec_snap.pops.len() >= 10,
            "capital sector with employer-partitioned Pops + 2 cultures must expose >= 10 rows, got {}",
            cap_sec_snap.pops.len()
        );

        // 3. Verify newly built orchards receive 1 month of starter working capital (`60.00 CR`),
        //    and a 100% full orchard discounts below cost instead of spiking to `12.00 CR`!
        let ask_normal = settlement::compute_workshop_ask_cents(1_000, 300, 20, 120);
        assert_eq!(ask_normal, 1_000, "low-inventory orchard quotes at 100% MA, not 92%");
        let ask_full = settlement::compute_workshop_ask_cents(320, 300, 120, 120);
        assert!(
            ask_full < 300,
            "100% full orchard must discount below unit cost to clear stock, got {ask_full}"
        );

        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let empty_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| t.tags.has(TileTag::Land) && usize::from(t.index) != snap_multi.starter_tile_index)
                .unwrap()
                .index,
        );
        let starter_idx = snap_multi.starter_tile_index;
        game.build_on_tile(
            "base:earth",
            starter_idx,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary2),
        )
        .unwrap();
        for (_, (_p, pt)) in game.world.query::<(&Planet, &PlanetTiles)>().iter() {
            let ws = pt.workshops[starter_idx][TileSlotKey::Secondary2.as_index()]
                .as_ref()
                .unwrap();
            assert_eq!(
                ws.inv.avail(cash_id),
                60 * CASH_SCALE,
                "newly built 1x orchard must hold 60 CR working capital"
            );
        }
        let _ = empty_tile;

        let ruling = PolityId(0);
        for m in 0..4 {
            let before = game.planet_money_supply_cents("base:earth");
            game.settle_planet_month("base:earth", ruling);
            let after = game.planet_money_supply_cents("base:earth");
            let minted: i64 = game
                .world
                .query::<&Polity>()
                .iter()
                .map(|(_, p)| p.last_seigniorage_cents)
                .sum();
            assert_eq!(after, before + minted, "multi-culture settlement cash leak in month {m}");
        }
    }

    #[test]
    fn between_jobs_preserves_role_wallet_slides_to_vagrant_on_destitution_and_fixes_zombie_orchards() {
        let mut game = Game::with_legacy_apple_catalog();
        let ruling = PolityId(0);
        let cash_id = game.goods().cash_id();
        let apple_id = game.goods().id_of(market::GOOD_APPLE).unwrap();

        // 1. Starter capital sector separates State vs LocalPrivate vs `劳工 · 待业` (`Unowned`) + `流民`!
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let cap_sec_u16 = earth0
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .sector_id
            .unwrap();
        let cap0 = earth0.sectors.iter().find(|s| s.id == cap_sec_u16).unwrap();

        let state_lab = cap0
            .pops
            .iter()
            .find(|p| p.id == POP_LABORER && p.employer_kind == "state")
            .expect("must have State laborers");
        let priv_lab = cap0
            .pops
            .iter()
            .find(|p| p.id == POP_LABORER && p.employer_kind == "local_private")
            .expect("must have LocalPrivate laborers");
        let wait_lab = cap0
            .pops
            .iter()
            .find(|p| p.id == POP_LABORER && p.employer_kind == "unowned")
            .expect("must have between-jobs `劳工 · 待业`");
        assert_eq!(state_lab.count, 100);
        assert_eq!(priv_lab.count, 200);
        assert_eq!(wait_lab.count, 50);
        assert_eq!(wait_lab.name, "劳工 · 待业");
        assert_eq!(cap0.vagrant_count, 15, "capital starts with 15 destitute `流民`");

        // 2. Massive laborer layoffs + vagrants + cheap apples MUST NEVER drag Scholar wage down!
        let vag_pid = game.pop_catalog().id_of(POP_VAGRANT).unwrap();
        let goods_for_vag = game.goods().clone();
        for (_, (sec, mkt, spops)) in
            game.world.query_mut::<(&Sector, &mut SectorMarket, &mut SectorPops)>()
        {
            if sec.id == SectorId(cap_sec_u16) {
                let ab = mkt.apple_book_mut();
                ab.ma_price_cents = 320;
                ab.last_price_cents = 320;
                let gi = spops.ensure_group_index(&goods_for_vag, vag_pid, 0, BuildingOwner::Unowned);
                spops.groups[gi].count = 250; // 250 hungry vagrants!
                spops.groups[gi].last_food_met_permille = 300;
            }
        }
        game.settle_planet_month("base:earth", ruling);
        let earth1 = game.planet_by_id("base:earth").unwrap();
        let cap1 = earth1.sectors.iter().find(|s| s.id == cap_sec_u16).unwrap();
        let scholar_quote = cap1
            .wage_quotes
            .iter()
            .find(|q| q.id == POP_ACADEMIC)
            .unwrap();
        assert_eq!(
            scholar_quote.ma_wage_per_100_cents,
            130 * CASH_SCALE,
            "250 hungry vagrants & cheap apples must NEVER cut scholar nominal wage!"
        );
        let soldier_quote = cap1
            .wage_quotes
            .iter()
            .find(|q| q.id == POP_SOLDIER)
            .unwrap();
        assert_eq!(
            soldier_quote.ma_wage_per_100_cents,
            80 * CASH_SCALE,
            "zero soldier vacancies must NOT open labor book or depress soldier MA wage!"
        );
        let bureaucrat_quote = cap1
            .wage_quotes
            .iter()
            .find(|q| q.id == POP_BUREAUCRAT)
            .unwrap();
        assert_eq!(
            bureaucrat_quote.ma_wage_per_100_cents,
            90 * CASH_SCALE,
            "zero bureaucrat vacancies must NOT open labor book or depress bureaucrat MA wage!"
        );

        // 3. Privatize Secondary0 -> staff migrates to `local_private`; then simulate a jammed full
        //    warehouse (`120/120` apples) + `0` cash + wage arrears -> must lay off into `劳工 · 待业`!
        game.privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        for (_, (_p, pt)) in game.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[starter_idx][TileSlotKey::Secondary0.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 0;
                ws.inv.available[usize::from(apple_id.0)] = ws.inv.capacity; // 100% jammed full!
                ws.last_revenue_cents = 0;
                ws.had_wage_arrears = true;
            }
        }
        game.settle_planet_month("base:earth", ruling);
        let after_jam = game.planet_by_id("base:earth").unwrap();
        let sec0_b = after_jam
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .secondary[0]
            .as_ref()
            .unwrap();
        assert_eq!(
            sec0_b.jobs_filled, 0,
            "jammed unpaid zombie orchard must immediately release all 100 workers, got {}",
            sec0_b.jobs_filled
        );

        // 4. Verify `学者 · 待业` keeps own savings initially, and ONLY slides down `demote_to` -> `流民`
        //    once savings hit 0 and food is short!
        let acad_pid = game.pop_catalog().id_of(POP_ACADEMIC).unwrap();
        let goods_clone = game.goods().clone();
        for (_, (sec, spops)) in game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == SectorId(cap_sec_u16) {
                let gi =
                    spops.ensure_group_index(&goods_clone, acad_pid, 0, BuildingOwner::Unowned);
                spops.groups[gi].count = 16;
                spops.groups[gi].sync_capacity();
                spops.groups[gi].inv.available[usize::from(cash_id.0)] = 0; // broke!
                spops.groups[gi].inv.available[usize::from(apple_id.0)] = 0; // empty pantry!
            }
        }
        game.settle_planet_month("base:earth", ruling);
        let after_demote = game.planet_by_id("base:earth").unwrap();
        let cap_dem = after_demote
            .sectors
            .iter()
            .find(|s| s.id == cap_sec_u16)
            .unwrap();
        let rem_acad_wait = cap_dem
            .pops
            .iter()
            .find(|p| p.id == POP_ACADEMIC && p.employer_kind == "unowned")
            .map(|p| p.count)
            .unwrap_or(0);
        assert!(
            rem_acad_wait < 16,
            "destitute hungry `学者 · 待业` must demote along `demote_to`, remaining={rem_acad_wait}"
        );
    }

    #[test]
    fn policies_can_be_changed() {
        let mut game = Game::default();
        let snap = game
            .set_monetary_policy("base:earth", MonetaryPolicy::Loose)
            .unwrap();
        assert_eq!(snap.polity.monetary_policy, MonetaryPolicy::Loose);
        let sid = snap.sectors[0].id;
        let snap = game
            .set_depot_policy("base:earth", sid, DepotPolicy::Stockpile)
            .unwrap();
        assert_eq!(snap.sectors[0].depot_policy, DepotPolicy::Stockpile);
    }

    #[test]
    fn subticks_expand_only_during_combat_and_realign_when_combat_ends() {
        let mut game = Game::default();
        game.set_speed(GameSpeed::Speed2);
        game.step_once();
        assert_eq!(game.counters().combat_subticks, 0);
        assert_eq!(game.counters().daily_ticks, 1);

        let fleet = game.world.spawn((InCombat::default(),));
        for _ in 0..3 {
            game.step_once();
        }
        assert_eq!(game.time().subtick, 3);
        game.world.despawn(fleet).unwrap();

        let out = game.step_once();
        assert!(out.day_advanced);
        assert_eq!(game.time().subtick, 0);
    }

    #[test]
    fn stream_subscriber_receives_surface_once_and_sparse_planet_updates() {
        let runtime = GameRuntime::default();
        let (sub_id, rx) = runtime.subscribe(None).unwrap();

        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(first, StreamEvent::Init { .. }));
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(second, StreamEvent::PlanetSurface { surface: Some(_) }));
        let third = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (initial_count, starter_region_id) = match third {
            StreamEvent::PlanetView { planet: Some(p) } => {
                let reg = p
                    .occupied_tiles
                    .iter()
                    .find(|t| t.index == p.starter_tile_index)
                    .map(|t| t.region_id)
                    .unwrap_or(0);
                (p.total_building_count, reg)
            }
            other => panic!("expected initial PlanetView, got {other:?}"),
        };

        runtime
            .build_in_region_async(
                sub_id,
                "base:earth".into(),
                starter_region_id,
                "base:agro_farm".into(),
            )
            .unwrap();
        let built_evt = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match built_evt {
            StreamEvent::PlanetView { planet: Some(p) } => {
                assert_eq!(p.total_building_count, initial_count + 1);
            }
            other => panic!("expected PlanetView after build_in_region_async, got {other:?}"),
        }

        runtime.unsubscribe(sub_id);
    }

    #[test]
    fn spot_driven_ma_unlocks_contract_pool_and_private_investment_guards_cbd_payroll() {
        let goods = GoodCatalog::from_defs(default_good_defs()).unwrap();
        let apple = goods.id_of(market::GOOD_APPLE).unwrap();
        let cash = goods.cash_id();

        // 1. Simulate the exact glut from user screenshot: 470 apples offered at `6.80` (`680` cents)
        //    while `MA` starts at `8.40` (`840` cents) and buyers only want `30` apples!
        let mut seller_inv = Inventory::new(goods.len(), 600);
        seller_inv.available[usize::from(apple.0)] = 470;
        let mut buyer_inv = Inventory::new(goods.len(), 200);
        buyer_inv.available[usize::from(cash.0)] = 50_000;
        let mut vault_inv = Inventory::new(goods.len(), 0);
        let mut invs = vec![vault_inv.clone(), seller_inv, buyer_inv];
        let _ = &mut vault_inv;

        let mut books = vec![market::SectorOrderBook::new(apple, 840)];
        let logistics = LogisticsMatrix::from_links(1, &[], &[false]);
        market::clear_planetary_good_market(
            &goods,
            apple,
            &mut books,
            &logistics,
            &mut invs,
            0,
            vec![market::SellSubmission {
                sector_id: 0,
                goods_inv: 1,
                cash_inv: 1,
                ask_price_cents: 680,
                qty: 470,
                allow_contract_pool: true,
            }],
            vec![market::BuySubmission {
                sector_id: 0,
                goods_inv: 2,
                cash_inv: 2,
                bid_price_cents: 900,
                qty: 30,
                allow_contract_pool: true,
            }],
        );
        market::release_resting_orders(&goods, apple, &mut books, &mut invs);
        // Spot trades happen at `6.80`, and `MA` MUST step down below `8.00` on the very first month
        // (instead of staying self-locked near `8.40` from 70% contract-pool volume!).
        assert!(
            books[0].ma_price_cents <= 790,
            "spot-driven MA must step down toward 680 during a 470-apple glut, got {}",
            books[0].ma_price_cents
        );

        // 2. Private autonomous investment MUST skip glutted sectors and keep `商业街` Workshop solvent!
        let mut game = Game::with_legacy_apple_catalog();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        // Give `SectorCapital` modest cash (`80 CR`, below any build cost) and verify `商业街` Workshop stays solvent on its own `base:service` revenue!
        for (_, (_sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            cap.vault.available[usize::from(cash.0)] = 80 * CASH_SCALE;
        }
        let b_before = game.planet_by_id("base:earth").unwrap().total_building_count;
        game.run_monthly_political_economy();
        let after_guard = game.planet_by_id("base:earth").unwrap();
        assert_eq!(
            after_guard.total_building_count, b_before,
            "private capital must NOT build when cash is below build cost!"
        );
        let cbd_slot = after_guard
            .occupied_tiles
            .iter()
            .find(|t| t.index == earth0.starter_tile_index)
            .unwrap()
            .secondary[1]
            .as_ref()
            .unwrap();
        assert_eq!(
            cbd_slot.jobs_filled, 40,
            "CBD Workshop must retain all 40/40 staff (`15 资本家 + 25 官僚`) by selling local `base:service`!"
        );
    }

    #[test]
    fn decision_layer_severance_redemption_priority_and_spatial_fob_call_auction_work() {
        let goods = GoodCatalog::from_defs(default_good_defs()).unwrap();
        let apple = goods.id_of(market::GOOD_APPLE).unwrap();
        let cash = goods.cash_id();

        // 1. Spatial Call Auction: 1 bankrupt orchard selling 10 apples at `5.00` (`500` cents)
        //    alongside 90 normal apples at `10.00` (`1000` cents) against 80 local demand at `10.00`
        //    MUST receive the uniform local FOB clearing price `10.00` (not `5.00`!) and keep MA at `10.00`!
        let mut vault_inv = Inventory::new(goods.len(), 0);
        let mut bankrupt_inv = Inventory::new(goods.len(), 120);
        bankrupt_inv.available[usize::from(apple.0)] = 10;
        let mut normal_inv = Inventory::new(goods.len(), 200);
        normal_inv.available[usize::from(apple.0)] = 90;
        let mut buyer_inv = Inventory::new(goods.len(), 200);
        buyer_inv.available[usize::from(cash.0)] = 100_000;
        let _ = &mut vault_inv;

        let mut invs = vec![vault_inv.clone(), bankrupt_inv, normal_inv, buyer_inv];
        let mut books = vec![market::SectorOrderBook::new(apple, 1_000)];
        let logistics_1 = LogisticsMatrix::from_links(1, &[], &[false]);
        market::clear_planetary_good_market(
            &goods,
            apple,
            &mut books,
            &logistics_1,
            &mut invs,
            0,
            vec![
                market::SellSubmission {
                    sector_id: 0,
                    goods_inv: 1,
                    cash_inv: 1,
                    ask_price_cents: 500, // 50% MA liquidation floor
                    qty: 10,
                    allow_contract_pool: false,
                },
                market::SellSubmission {
                    sector_id: 0,
                    goods_inv: 2,
                    cash_inv: 2,
                    ask_price_cents: 1_000,
                    qty: 90,
                    allow_contract_pool: true,
                },
            ],
            vec![market::BuySubmission {
                sector_id: 0,
                goods_inv: 3,
                cash_inv: 3,
                bid_price_cents: 1_000,
                qty: 80,
                allow_contract_pool: true,
            }],
        );
        market::release_resting_orders(&goods, apple, &mut books, &mut invs);
        assert_eq!(
            invs[1].avail(apple),
            0,
            "50% liquidation ask must get first queue priority and sell all 10 apples"
        );
        assert_eq!(
            invs[1].avail(cash),
            10 * 1_000,
            "bankrupt seller must receive full uniform FOB clearing price 10.00 CR (10,000 cents)"
        );
        assert_eq!(books[0].last_price_cents, 1_000);
        assert_eq!(books[0].ma_price_cents, 1_000);

        // 2. Cross-sector Spatial Call Auction with freight & shipping capacity cap:
        //    Sector 0 has 100 surplus apples at `6.00` (`600`), route `0 -> 1` has freight `2.00` (`200`)
        //    and capacity `40`; Sector 1 buyers bid `10.00` (`1000`) for `60` apples while local Sector 1
        //    sellers ask `10.00` for `50` apples.
        let mut s0_seller = Inventory::new(goods.len(), 200);
        s0_seller.available[usize::from(apple.0)] = 100;
        let mut s1_seller = Inventory::new(goods.len(), 200);
        s1_seller.available[usize::from(apple.0)] = 50;
        let mut s1_buyer = Inventory::new(goods.len(), 200);
        s1_buyer.available[usize::from(cash.0)] = 100_000;
        let mut invs2 = vec![
            Inventory::new(goods.len(), 0),
            s0_seller,
            s1_seller,
            s1_buyer,
        ];
        let mut books2 = vec![
            market::SectorOrderBook::new(apple, 600),
            market::SectorOrderBook::new(apple, 1_000),
        ];
        let logistics_2 = LogisticsMatrix::from_links(2, &[(0, 1, 40, 2)], &[false, false]);
        market::clear_planetary_good_market(
            &goods,
            apple,
            &mut books2,
            &logistics_2,
            &mut invs2,
            0,
            vec![
                market::SellSubmission {
                    sector_id: 0,
                    goods_inv: 1,
                    cash_inv: 1,
                    ask_price_cents: 600,
                    qty: 100,
                    allow_contract_pool: false,
                },
                market::SellSubmission {
                    sector_id: 1,
                    goods_inv: 2,
                    cash_inv: 2,
                    ask_price_cents: 1_000,
                    qty: 50,
                    allow_contract_pool: false,
                },
            ],
            vec![market::BuySubmission {
                sector_id: 1,
                goods_inv: 3,
                cash_inv: 3,
                bid_price_cents: 1_000,
                qty: 60,
                allow_contract_pool: false,
            }],
        );
        market::release_resting_orders(&goods, apple, &mut books2, &mut invs2);
        assert_eq!(books2[0].last_price_cents, 600, "bottlenecked exporter clears at 6.00 FOB");
        assert_eq!(invs2[1].avail(cash), 40 * 600, "exporter sells 40 capped units at 6.00 FOB");
        assert_eq!(invs2[0].avail(cash), 40 * 200, "freight receiver collects 40 * 2.00 CR");
        assert_eq!(invs2[2].avail(cash), 20 * 1_000, "local Sector 1 seller fills remaining 20 at 10.00");

        // 3. `set_slot_target_jobs(..., immediate_buyout = true)` pays exact 1-month severance & conserves cash;
        //    and Polity priority redemption pays `军人` 100% first when treasury cash is tight!
        let mut game = Game::with_legacy_apple_catalog();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let money_before = game.planet_money_supply_cents("base:earth");
        let after_cut = game
            .set_slot_target_jobs(
                "base:earth",
                starter_idx,
                TileSlotKey::Secondary0,
                Some(60),
                true,
            )
            .unwrap();
        let money_after = game.planet_money_supply_cents("base:earth");
        assert_eq!(money_before, money_after, "early severance buyout must strictly conserve cash");
        let sec0_slot = after_cut
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .secondary[0]
            .as_ref()
            .unwrap();
        assert_eq!(sec0_slot.jobs_filled, 60, "immediate buyout must trim orchard staff to 60");

        // Drain Polity vault so it can cover 40 Soldiers (`40 * 0.80 = 32.00 CR`) in Planetary Capital
        // but NOT the full `80 官僚 + 40 学者 + 40 工程师 + 40 军人` (`212.00 CR`), with Tight 0% minting.
        game.set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        // Close Secondary0 orchard so state enterprise profit doesn't refill treasury mid-test, and zero tax.
        game.set_slot_target_jobs("base:earth", starter_idx, TileSlotKey::Secondary0, Some(0), true)
            .unwrap();
        for (_, pol) in game.world.query_mut::<&mut Polity>() {
            pol.tax_rate_permille = 0;
            pol.vault.available[usize::from(cash.0)] = 50 * CASH_SCALE; // 50.00 CR > 32.00 CR soldier bill!
        }
        let soldier_pid = game.pop_catalog().id_of(POP_SOLDIER).unwrap();
        let academic_pid = game.pop_catalog().id_of(POP_ACADEMIC).unwrap();
        game.settle_planet_month("base:earth", PolityId(0));
        let cap_sec_u16 = earth0
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .sector_id
            .unwrap();
        for (_, (sec, pops)) in game.world.query::<(&Sector, &SectorPops)>().iter() {
            if sec.id == SectorId(cap_sec_u16) {
                let soldier_g = pops
                    .groups
                    .iter()
                    .find(|g| {
                        g.pop_type == soldier_pid
                            && matches!(g.employer, BuildingOwner::State { .. })
                    })
                    .unwrap();
                let academic_g = pops
                    .groups
                    .iter()
                    .find(|g| {
                        g.pop_type == academic_pid
                            && matches!(g.employer, BuildingOwner::State { .. })
                    })
                    .unwrap();
                assert_eq!(
                    soldier_g.last_income_cents,
                    40 * 80,
                    "soldiers must receive 100% of their 32.00 CR military pay first!"
                );
                assert_eq!(
                    academic_g.last_income_cents,
                    0,
                    "lower-priority state roles absorb the treasury shortfall after soldiers & core admin"
                );
            }
        }

        // 4. Explicit wage withholding (`set_slot_wage_redemption(..., false)`) on a private secondary
        //    orchard records senior `UnpaidWageClaim`s; 3 months of unresolved insolvency liquidates
        //    apples on the order book and closes the bankrupt private slot while conserving cash!
        let mut game_b = Game::with_legacy_apple_catalog();
        game_b
            .set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        let snap_b = game_b.planet_by_id("base:earth").unwrap();
        let s_idx = snap_b.starter_tile_index;
        game_b
            .privatize_slot("base:earth", s_idx, TileSlotKey::Secondary0)
            .unwrap();
        for (_, (_p, pt)) in game_b.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[s_idx][TileSlotKey::Secondary0.as_index()].as_mut() {
                ws.inv.available[usize::from(cash.0)] = 0;
            }
        }
        game_b
            .set_slot_wage_redemption("base:earth", s_idx, TileSlotKey::Secondary0, false)
            .unwrap();
        for m in 0..3 {
            let m_before = game_b.planet_money_supply_cents("base:earth");
            game_b.settle_planet_month("base:earth", PolityId(0));
            let m_after = game_b.planet_money_supply_cents("base:earth");
            assert_eq!(m_before, m_after, "bankruptcy path must strictly conserve cash in month {m}");
        }
        let after_bankrupt = game_b.planet_by_id("base:earth").unwrap();
        let sec0_after = after_bankrupt
            .occupied_tiles
            .iter()
            .find(|t| t.index == s_idx)
            .unwrap()
            .secondary[0]
            .as_ref();
        assert!(
            sec0_after.is_none(),
            "3-month insolvent private secondary orchard must liquidate inventory and exit cleanly!"
        );
    }

    #[test]
    fn three_vault_symmetry_multi_good_trust_and_channel_boundaries_work() {
        let mut game = Game::with_legacy_apple_catalog();
        game.set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let cap_region = surf.tiles[starter_idx].region_id;
        let cap_sec_u16 = earth0.region_summaries[usize::from(cap_region)]
            .sector_id
            .unwrap();
        let cap_sec = SectorId(cap_sec_u16);
        let cash_id = game.goods().cash_id();

        // 1. Channel-1 ownership guard: Polity MUST NOT directly rewrite a private slot's target jobs!
        game.privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary0)
            .unwrap();
        assert!(
            game.set_state_slot_target_jobs(
                "base:earth",
                starter_idx,
                TileSlotKey::Secondary0,
                Some(20),
                false
            )
            .is_err(),
            "Channel-1 must reject direct Polity tampering with a LocalPrivate slot!"
        );

        // 2. Channel-3 boundary guard: `LocalPrivate` MUST be rejected from bilateral contracts!
        let apple_id = game.goods().id_of(GOOD_APPLE).unwrap();
        assert!(
            game.sign_bilateral_supply_contract(
                "base:earth",
                BuildingOwner::LocalPrivate {
                    home_sector: cap_sec
                },
                BuildingOwner::State {
                    polity_id: PolityId(0)
                },
                cap_sec,
                apple_id,
                10,
                900,
                6,
            )
            .is_err(),
            "Channel-3 must strictly reject LocalPrivate / SectorCapital!"
        );

        // 3. Found Corporate Campus (`Enterprise`), verify independent `Enterprise.vault` receives
        //    corporate profits while `SectorCapital` stays decoupled, AND test Channel-3 breach rating!
        game.build_on_tile(
            "base:earth",
            starter_idx,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary2),
        )
        .unwrap();
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == cap_sec {
                cap.vault.available[usize::from(cash_id.0)] = 400 * CASH_SCALE;
            }
        }
        game.privatize_slot("base:earth", starter_idx, TileSlotKey::Secondary2)
            .unwrap();
        let occ_set: std::collections::HashSet<usize> = game
            .planet_by_id("base:earth")
            .unwrap()
            .occupied_tiles
            .iter()
            .map(|t| t.index)
            .collect();
        let campus_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| {
                    t.region_id == cap_region
                        && t.tags.has(TileTag::Land)
                        && !occ_set.contains(&usize::from(t.index))
                })
                .unwrap()
                .index,
        );
        let after_campus = game
            .build_on_tile(
                "base:earth",
                campus_tile,
                BUILDING_CORPORATE_CAMPUS,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        assert_eq!(after_campus.enterprises.len(), 1);
        let ent_id = EnterpriseId(after_campus.enterprises[0].id);
        let ent_owner = BuildingOwner::Enterprise {
            enterprise_id: ent_id,
        };
        let state_owner = BuildingOwner::State {
            polity_id: PolityId(0),
        };

        // Give one corporate orchard excess cash so Step 7 sweeps profit to `Enterprise.vault` (NOT `SectorCapital`)!
        for (_, (_p, pt)) in game.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[starter_idx][TileSlotKey::Secondary0.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 600 * CASH_SCALE;
            }
        }
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == cap_sec {
                cap.vault.available[usize::from(cash_id.0)] = 100 * CASH_SCALE;
            }
        }
        let ent_cash_before = game.enterprise_cash_cents("base:earth", ent_id);
        let supply_before = game.planet_money_supply_cents("base:earth");
        game.settle_planet_month("base:earth", PolityId(0));
        let supply_after = game.planet_money_supply_cents("base:earth");
        assert_eq!(supply_before, supply_after, "three-vault sweep must strictly conserve cash");
        let ent_cash_after = game.enterprise_cash_cents("base:earth", ent_id);
        assert!(
            ent_cash_after - ent_cash_before >= 280 * CASH_SCALE,
            "Enterprise orchard's 500 CR windfall must sweep into Enterprise.vault ({ent_cash_before} -> {ent_cash_after})"
        );
        assert!(
            game.sector_pool_cents("base:earth", cap_sec) < 250 * CASH_SCALE,
            "SectorCapital NEVER receives Enterprise orchard's 500 CR profit sweep!"
        );

        // Channel-3 contract between `Enterprise` and `Polity`, then deliberate seller breach -> rating drops!
        let cid = game
            .sign_bilateral_supply_contract(
                "base:earth",
                ent_owner,
                state_owner,
                cap_sec,
                apple_id,
                5,
                900,
                3,
            )
            .unwrap();
        game.set_bilateral_contract_honoring("base:earth", ent_owner, cid, false)
            .unwrap();
        game.settle_planet_month("base:earth", PolityId(0));
        let snap_breach = game.planet_by_id("base:earth").unwrap();
        assert_eq!(
            snap_breach.enterprises[0].credit_rating,
            88,
            "deliberate Channel-3 breach must deduct 12 credit rating points"
        );

        // 4. `SectorCapital` overflow rule + cross-sector frontier pioneer settlement + auto-CBD!
        //    Pick an empty frontier land sector (`0` buildings), give Capital `SectorCapital` 1,800 CR:
        //    - First verify `settle_planet_month` overflows cash above `SECTOR_CAPITAL_OVERFLOW_CAP_CENTS` to capitalists;
        //    - Then give `SectorCapital` 680 CR and run `run_autonomous_private_investment`: verify a frontier
        //      sector with 0 buildings receives its very first `2x` `LocalPrivate { home_sector: frontier_sec }` farm!
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == cap_sec {
                cap.vault.available[usize::from(cash_id.0)] = 2_000 * CASH_SCALE;
            }
        }
        game.settle_planet_month("base:earth", PolityId(0));
        let capped_pool = game.sector_pool_cents("base:earth", cap_sec);
        let expected_dynamic_cap = BASE_SECTOR_CAPITAL_POOL_CAP_CENTS + 15 * 40 * CASH_SCALE;
        assert!(
            capped_pool <= expected_dynamic_cap + 50 * CASH_SCALE,
            "SectorCapital above dynamic service-scaled cap ({expected_dynamic_cap}) must overflow 100% as dividends, got {capped_pool}"
        );

        // Close remaining empty secondary slots in Capital so Capital `SectorCapital` spills over to frontier!
        let frontier_sec_u16 = snap_breach
            .sectors
            .iter()
            .find(|s| s.id != cap_sec_u16 && s.cbd_hosted == 0)
            .unwrap()
            .id;
        let frontier_sec = SectorId(frontier_sec_u16);
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == cap_sec {
                cap.vault.available[usize::from(cash_id.0)] = 700 * CASH_SCALE;
            }
        }
        // Temporarily disallow private land in all sectors EXCEPT `frontier_sec` to isolate frontier pioneer opening!
        let mut allow_map = BTreeMap::new();
        for s in &snap_breach.sectors {
            allow_map.insert(SectorId(s.id), s.id == frontier_sec_u16);
        }
        game.run_autonomous_private_investment("base:earth", &allow_map);
        let after_pioneer = game.planet_by_id("base:earth").unwrap();
        let frontier_snap = after_pioneer
            .sectors
            .iter()
            .find(|s| s.id == frontier_sec_u16)
            .unwrap();
        assert_eq!(
            frontier_snap.cbd_hosted, 1,
            "rich SectorCapital must pioneer a frontier sector's 1st local workshop stamped to `home_sector = frontier_sec`!"
        );

        // Build a `大区核心` hub in `frontier_sec`, fund `frontier_sec` `SectorCapital` with 450 CR,
        // and verify `SectorCapital` automatically constructs its own `商业街` (`cbd_count: 0 -> 1`)!
        let frontier_reg = frontier_snap.geo_region_id;
        game.build_in_region("base:earth", frontier_reg, BUILDING_REGIONAL_CORE)
            .unwrap();
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == frontier_sec {
                cap.vault.available[usize::from(cash_id.0)] = 450 * CASH_SCALE;
            }
        }
        game.run_autonomous_private_investment("base:earth", &allow_map);
        let after_auto_cbd = game.planet_by_id("base:earth").unwrap();
        let frontier_cbd = after_auto_cbd
            .sectors
            .iter()
            .find(|s| s.id == frontier_sec_u16)
            .unwrap()
            .cbd_count;
        assert_eq!(
            frontier_cbd, 1,
            "SectorCapital with an active local workshop + hub + sufficient cash must auto-fund 商业街!"
        );

        // 5. Dynamic multi-good market & `Trust` equity share mechanism:
        //    Charter a `Trust` (registers weightless `Telecom` share good -> auto-creates order books
        //    in all 12 sectors), transfer 50% shares to a PopGroup in `frontier_sec`, inject profit into
        //    `Trust.vault`, and verify pro-rata `Inventory`-scanned dividend payout & exact cash conservation!
        let m_pre_trust = game.planet_money_supply_cents("base:earth");
        let (trust_id, share_gid) = game
            .charter_trust(
                "base:earth",
                "寰宇农产托拉斯".into(),
                BuildingTag::Agriculture.as_str().into(),
                "trust:agro_share".into(),
                "农产托拉斯股票".into(),
                100,
                1_000,
                state_owner,
                300 * CASH_SCALE,
            )
            .unwrap();
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            m_pre_trust,
            "chartering a Trust must strictly conserve planetary cash"
        );
        // Every sector market must automatically hold an order book for `share_gid`!
        for (_, mkt) in game.world.query::<&SectorMarket>().iter() {
            assert!(
                mkt.book(share_gid).is_some(),
                "every SectorMarket must auto-spawn an order book for newly registered Trust shares"
            );
        }
        // Move 50 of Polity's 100 shares to `frontier_sec`'s first PopGroup (simulating secondary market purchase),
        // and simulate a profitable Trust workshop sweeping cash into `Trust.vault`.
        let s_idx_u = usize::from(share_gid.0);
        for (_, pol) in game.world.query_mut::<&mut Polity>() {
            pol.vault.available[s_idx_u] -= 50;
        }
        let goods_len = game.goods().len();
        let mut pop_cash_before = 0i64;
        for (_, (sec, pops)) in game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == frontier_sec {
                pops.groups[0].inv.ensure_len(goods_len);
                pops.groups[0].inv.available[s_idx_u] += 50;
                pop_cash_before = pops.groups[0].inv.avail(cash_id);
                break;
            }
        }
        // Assign one orchard to `Trust` with 400 CR excess cash so Step 7 sweeps into `Trust.vault` & Step 8C pays dividends!
        for (_, (_p, pt)) in game.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            pt.set_slot_owner(
                starter_idx,
                TileSlotKey::Secondary2,
                BuildingOwner::Trust { trust_id },
            );
            if let Some(ws) = pt.workshops[starter_idx][TileSlotKey::Secondary2.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 450 * CASH_SCALE;
            }
        }
        let m_before_div = game.planet_money_supply_cents("base:earth");
        game.settle_planet_month("base:earth", PolityId(0));
        let m_after_div = game.planet_money_supply_cents("base:earth");
        assert_eq!(m_before_div, m_after_div, "Trust dividend distribution must strictly conserve cash");

        let mut pop_income_seen = 0i64;
        for (_, (sec, pops)) in game.world.query::<(&Sector, &SectorPops)>().iter() {
            if sec.id == frontier_sec {
                pop_income_seen = pops
                    .groups
                    .iter()
                    .filter(|g| g.inv.total(share_gid) > 0)
                    .map(|g| g.last_income_cents)
                    .sum();
                break;
            }
        }
        let _ = pop_cash_before;
        assert!(
            pop_income_seen > 0,
            "PopGroup holding 50% of Trust shares in its Inventory must receive pro-rata dividends, got {pop_income_seen}"
        );
    }

    #[test]
    fn scale_independent_node_matrix_event_driven_contracts_and_data_only_catalogs_work() {
        // 1. Scale-independent `N × N` `NodeMatrix` across a 4-planet interstellar jumpgate network:
        //    - Planets 0, 1, 2 have orbital jumpgates (`HubCliqueRule`: cost 300 cents, cap 120);
        //    - Planet 3 is a frontier colony linked to Planet 2 via sublight shuttle (`(2, 3, cap 50, cost 150)`).
        //    - Verify Floyd multi-hop synthesis (`0 -> 3` = `300 + 150 = 450`, bottleneck cap `50`),
        //      generic zero-sum `compute_pairwise_flows`, AND `clear_spatial_good_market` with custom `MarketClearingRule`!
        let star_net = NodeMatrix::from_spec(&NodeNetworkSpec {
            node_count: 4,
            direct_edges_cents: vec![(2, 3, 50, 150)],
            hub_cliques: vec![HubCliqueRule {
                eligible_nodes: vec![true, true, true, false],
                unit_cost_cents: 300,
                monthly_capacity: 120,
            }],
            allow_multihop_relay: true,
        });
        assert_eq!(star_net.cost(0, 2), 300);
        assert_eq!(star_net.capacity(0, 2), 120);
        assert_eq!(star_net.cost(0, 3), 450, "jumpgate + shuttle multi-hop relay cost");
        assert_eq!(star_net.capacity(0, 3), 50, "multi-hop bottleneck capacity");

        let pressures = vec![1_000i64, 400, 400, 100];
        let max_out = vec![80i64, 0, 0, 0];
        let (flows, deltas) =
            star_net.compute_pairwise_flows(&pressures, Some(&max_out), |_s, _d, ps, pd, cost, _cap| {
                ((ps - pd - cost) / 10).max(0)
            });
        assert_eq!(deltas.iter().sum::<i64>(), 0, "pairwise operator must be strictly zero-sum");
        assert!(flows[0 * 4 + 1] > 0 && deltas[0] < 0);

        let mut custom_goods = GoodCatalog::from_defs(vec![
            GoodDef {
                id: GOOD_CASH.into(),
                name: "星际信用点".into(),
                is_currency: true,
                non_market: false,
                unit_volume: 0,
                retention_permille: 1000,
                transport_channel: TransportChannel::Telecom,
                genesis_price_cents: CASH_SCALE,
                staple_demand_per_100: 0,
                service_demand_per_100: 0,
                excess_satisfaction_bonus: 0,
                capital_pool_cap_cents_per_unit: 0,
                strategic_depot_target: 0,
                coupon_permille_per_month: 0,
                issuer_key: None,
                market_rule: MarketClearingRule::spot_security(),
            },
            GoodDef {
                id: "mod:synth_ration".into(),
                name: "合成口粮".into(),
                is_currency: false,
                non_market: false,
                unit_volume: 1,
                retention_permille: 990,
                transport_channel: TransportChannel::Physical,
                genesis_price_cents: 1_000,
                staple_demand_per_100: 4,
                service_demand_per_100: 0,
                excess_satisfaction_bonus: 0,
                capital_pool_cap_cents_per_unit: 0,
                strategic_depot_target: 60,
                coupon_permille_per_month: 0,
                issuer_key: None,
                market_rule: MarketClearingRule {
                    contract_pool_percent: 0,
                    pricing_mode: PricingMode::MakerQuote,
                    ma_inertia_weight: 1,
                },
            },
            GoodDef {
                id: "mod:biomass".into(),
                name: "原生生物质".into(),
                is_currency: false,
                non_market: false,
                unit_volume: 1,
                retention_permille: 1000,
                transport_channel: TransportChannel::Physical,
                genesis_price_cents: 400,
                staple_demand_per_100: 0,
                service_demand_per_100: 0,
                excess_satisfaction_bonus: 0,
                capital_pool_cap_cents_per_unit: 0,
                strategic_depot_target: 0,
                coupon_permille_per_month: 0,
                issuer_key: None,
                market_rule: MarketClearingRule::default(),
            },
        ])
        .unwrap();
        let cash_id = custom_goods.cash_id();
        let ration_id = custom_goods.id_of("mod:synth_ration").unwrap();

        let mut p0_seller = Inventory::new(custom_goods.len(), 200);
        p0_seller.available[usize::from(ration_id.0)] = 60;
        let mut p3_buyer = Inventory::new(custom_goods.len(), 200);
        p3_buyer.available[usize::from(cash_id.0)] = 100_000;
        let mut star_invs = vec![Inventory::new(custom_goods.len(), 0), p0_seller, p3_buyer];
        let mut star_books: Vec<NodeOrderBook> = (0..4)
            .map(|_| NodeOrderBook::new(ration_id, 1_000))
            .collect();
        clear_spatial_good_market(
            &custom_goods,
            ration_id,
            custom_goods.get(ration_id).market_rule,
            &mut star_books,
            &star_net,
            &mut star_invs,
            0,
            vec![market::SellSubmission {
                sector_id: 0,
                goods_inv: 1,
                cash_inv: 1,
                ask_price_cents: 700,
                qty: 60,
                allow_contract_pool: false,
            }],
            vec![market::BuySubmission {
                sector_id: 3,
                goods_inv: 2,
                cash_inv: 2,
                bid_price_cents: 1_300,
                qty: 60,
                allow_contract_pool: false,
            }],
        );
        market::release_resting_orders(&custom_goods, ration_id, &mut star_books, &mut star_invs);
        assert_eq!(star_invs[2].avail(ration_id), 50, "frontier planet 3 receives bottleneck-capped 50 units");
        assert_eq!(star_invs[1].avail(cash_id), 50 * 700, "MakerQuote pays seller exact 7.00 CR ask");
        assert_eq!(star_invs[0].avail(cash_id), 50 * 450, "relay carrier collects 50 * 4.50 CR freight");

        // 2. Pure data-driven `BuildingCatalog` + `GoodCatalog` WITHOUT `base:apple`!
        let mut b_defs = default_building_defs();
        b_defs[0].monthly_output = vec![("mod:biomass".into(), 25)]; // repurpose starter ag building to produce biomass
        let mut custom_buildings = BuildingCatalog::from_defs(b_defs).unwrap();
        custom_buildings
            .register_building(BuildingDef {
                type_id: BuildingTypeId(NonZeroU16::new(99).unwrap()),
                id: "mod:ration_vat".into(),
                name: "合成口粮培养车间".into(),
                slot_rule: SlotRule::Any,
                job_priority: JobPriority::Standard,
                build_cost: 160,
                hosting_capacity: 0,
                jobs_by_role: vec![(POP_LABORER.into(), 80)],
                tags: BuildingTags::EMPTY.with(BuildingTag::Agriculture),
                unique_planet_tags: BuildingTags::EMPTY,
                unique_region_tags: BuildingTags::EMPTY,
                unique_tile_tags: BuildingTags::EMPTY,
                required_primary_tags: BuildingTags::EMPTY,
                required_tags: TileTags::EMPTY.with(TileTag::Land),
                forbidden_tags: TileTags::EMPTY,
                monthly_inputs: vec![("mod:biomass".into(), 10)],
                monthly_output: vec![("mod:synth_ration".into(), 24)],
            })
            .unwrap();
        let _ = &mut custom_goods;
        let mut mod_game = Game::with_catalogs(
            CalendarConfig::default(),
            custom_goods,
            custom_buildings,
            PopTypeCatalog::default(),
            EventCatalog::default(),
        );
        mod_game
            .set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        let starter_tile = mod_game.planet_by_id("base:earth").unwrap().starter_tile_index;
        mod_game
            .build_on_tile(
                "base:earth",
                starter_tile,
                "mod:ration_vat",
                Some(TileSlotKey::Secondary2),
            )
            .unwrap();
        let m0 = mod_game.planet_money_supply_cents("base:earth");
        mod_game.settle_planet_month("base:earth", PolityId(0));
        let m1 = mod_game.planet_money_supply_cents("base:earth");
        assert_eq!(m0, m1, "custom multi-good catalog without base:apple must conserve cash!");

        // 3. Declarative `EventCatalog` player `PendingEvent` resolution + standardized coupon bond payout!
        let mut ev_game = Game::with_legacy_apple_catalog();
        ev_game
            .set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        let state_owner = BuildingOwner::State {
            polity_id: PolityId(0),
        };
        let bond_gid = ev_game
            .issue_standardized_claim(
                "base:earth",
                state_owner,
                "bond:treasury_5pct".into(),
                "五年期主权公债".into(),
                1_000, // 10.00 CR face
                50,    // 5.0% monthly coupon = 0.50 CR (50 cents) per bond
                20,
            )
            .unwrap();
        // Transfer 10 bonds from Polity vault to Sector 0's first PopGroup and verify automatic AI coupon payout!
        let b_idx = usize::from(bond_gid.0);
        for (_, pol) in ev_game.world.query_mut::<&mut Polity>() {
            pol.vault.available[b_idx] -= 10;
        }
        let g_len = ev_game.goods().len();
        for (_, (sec, pops)) in ev_game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == SectorId(0) {
                pops.groups[0].inv.ensure_len(g_len);
                pops.groups[0].inv.available[b_idx] += 10;
                break;
            }
        }
        let pre_coupon_money = ev_game.planet_money_supply_cents("base:earth");
        ev_game.settle_planet_month("base:earth", PolityId(0));
        assert_eq!(
            ev_game.planet_money_supply_cents("base:earth"),
            pre_coupon_money,
            "coupon claim event payout must strictly conserve cash"
        );
        assert_eq!(
            ev_game.planet_by_id("base:earth").unwrap().polity.credit_rating,
            100,
            "honored sovereign bond coupon maintains 100 credit rating"
        );

        // Now set Polity as player-controlled (`set_controlled_actor`) -> next month MUST queue `PendingEvent`
        // instead of auto-deciding, and choosing option 1 (`default_coupon`) deducts 15 credit rating!
        let polity_cp = CounterpartyId::Polity {
            polity_id: PolityId(0),
        };
        ev_game.set_controlled_actor(Some(polity_cp));
        ev_game.settle_planet_month("base:earth", PolityId(0));
        assert_eq!(
            ev_game.pending_events().len(),
            1,
            "player-controlled Polity must receive a PendingEvent for due bond coupons"
        );
        let inst_id = ev_game.pending_events()[0].instance_id;
        ev_game.resolve_pending_event(inst_id, 1).unwrap(); // option 1 = `default_coupon` (-15 rating)
        assert!(ev_game.pending_events().is_empty());
        assert_eq!(
            ev_game.planet_by_id("base:earth").unwrap().polity.credit_rating,
            85,
            "player choosing default_coupon option must apply declarative -15 credit rating effect"
        );
    }

    #[test]
    fn service_flow_surplus_allocation_tax_sharing_polity_tree_and_foreign_investment_work() {
        let mut game = Game::with_legacy_apple_catalog();
        game.set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        let cash_id = game.goods().cash_id();
        let service_id = game.goods().id_of(GOOD_SERVICE).unwrap();
        let earth0 = game.planet_by_id("base:earth").unwrap();
        let starter_idx = earth0.starter_tile_index;
        let cap_sec_u16 = earth0
            .occupied_tiles
            .iter()
            .find(|t| t.index == starter_idx)
            .unwrap()
            .sector_id
            .unwrap();
        let cap_sec = SectorId(cap_sec_u16);

        // 1. Flow service (`GOOD_SERVICE`, `retention_permille == 0`) + Household Surplus Consumption/Investment:
        //    Give Sector `cap_sec`'s Pops generous savings so they retain `reserve_months` and spend surplus
        //    on extra `base:service` (boosting satisfaction) + inject into `SectorCapital`, while month-end
        //    wipes remaining `base:service` inventory to `0`!
        for (_, (sec, pops)) in game.world.query_mut::<(&Sector, &mut SectorPops)>() {
            if sec.id == cap_sec {
                for g in &mut pops.groups {
                    if g.count > 0 {
                        g.inv.available[usize::from(cash_id.0)] = 900 * CASH_SCALE;
                    }
                }
            }
        }
        let m_before = game.planet_money_supply_cents("base:earth");
        game.settle_planet_month("base:earth", PolityId(0));
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            m_before,
            "flow service market + Pop surplus consumption/investment must strictly conserve cash!"
        );
        assert_eq!(
            game.planet_good_total("base:earth", service_id),
            0,
            "flow service (`retention_permille == 0`) must clear to zero at month-end without hoarding!"
        );
        // Verify starter `商业街` Workshop sold `base:service` for positive revenue!
        let cbd_rev = game
            .world
            .query::<(&Planet, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (p, pt))| {
                (p.id == "base:earth").then(|| {
                    pt.workshops[starter_idx][TileSlotKey::Secondary1.as_index()]
                        .as_ref()
                        .unwrap()
                        .last_revenue_cents
                })
            })
            .unwrap();
        assert!(
            cbd_rev > 0,
            "商业街 Workshop must earn real revenue selling `base:service` on the local order book, got {cbd_rev}"
        );

        // 2. Uncapped private workshops (no 2-slot / 6-slot hard lock):
        //    Privatizing or building private workshops works purely on buyer cash, without free-CBD hacks!
        let surf = game.planet_surface_by_id("base:earth").unwrap();
        let other_sec_snap = earth0
            .sectors
            .iter()
            .find(|s| s.id != cap_sec_u16)
            .unwrap()
            .clone();
        let other_sec = SectorId(other_sec_snap.id);
        let other_tiles: Vec<usize> = surf
            .tiles
            .iter()
            .filter(|t| t.region_id == other_sec_snap.geo_region_id && t.tags.has(TileTag::Land))
            .take(3)
            .map(|t| usize::from(t.index))
            .collect();
        assert_eq!(other_tiles.len(), 3);
        for (_, (sec, cap)) in game.world.query_mut::<(&Sector, &mut SectorCapital)>() {
            if sec.id == other_sec {
                cap.vault.available[usize::from(cash_id.0)] = 1_200 * CASH_SCALE;
            }
        }
        for &ti in &other_tiles {
            game.build_on_tile(
                "base:earth",
                ti,
                BUILDING_APPLE_ORCHARD,
                Some(TileSlotKey::Primary),
            )
            .unwrap();
            game.privatize_slot("base:earth", ti, TileSlotKey::Primary)
                .unwrap();
        }
        let after_3_priv = game.planet_by_id("base:earth").unwrap();
        let other_after = after_3_priv
            .sectors
            .iter()
            .find(|s| s.id == other_sec.0)
            .unwrap();
        assert_eq!(
            other_after.cbd_count, 0,
            "privatizing 3 workshops in a sector with 0 CBDs must NOT spawn free CBDs or hit a 2-slot lock!"
        );
        assert_eq!(
            other_after.cbd_hosted, 3,
            "3 unincorporated private workshops must coexist freely without any CBD!"
        );

        // 3. Multi-level homogeneous `Polity` tree (`中央 -> 地方大区政府`), source tax-sharing (`60%` central / `40%` local),
        //    Channel-1 intergovernmental property protection (`一级政府一级产权`), and skip-level grant / central SOE!
        let pre_sub_money = game.planet_money_supply_cents("base:earth");
        let prov_polity = game
            .create_polity(
                "东亚大区地方政府".into(),
                Some(PolityId(0)),
                200, // 20% local statutory tax rate
                600, // 60% remitted directly at source to Central (`PolityId(0)`), 40% retained locally!
                500 * CASH_SCALE,
                Some(PolityId(0)),
            )
            .unwrap();
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            pre_sub_money,
            "funding a subordinate Polity from Central vault must strictly conserve cash!"
        );
        game.set_sector_admin_polity(PolityId(0), "base:earth", other_sec, prov_polity)
            .unwrap();
        // Provincial government builds its own local public orchard on `Secondary0` of `other_tiles[0]`:
        game.build_on_tile_as(
            prov_polity,
            "base:earth",
            other_tiles[0],
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary0),
        )
        .unwrap();
        // Central government (`PolityId(0)`) CANNOT overwrite a slot owned by `prov_polity` (`一级政府一级产权`),
        // test on a primary local-SOE tile:
        let prov_primary_tile = usize::from(
            surf.tiles
                .iter()
                .find(|t| {
                    t.region_id == other_sec_snap.geo_region_id
                        && t.tags.has(TileTag::Land)
                        && !other_tiles.contains(&usize::from(t.index))
                })
                .unwrap()
                .index,
        );
        game.build_on_tile_as(
            prov_polity,
            "base:earth",
            prov_primary_tile,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Primary),
        )
        .unwrap();
        assert!(
            game.build_on_tile_as(
                PolityId(0),
                "base:earth",
                prov_primary_tile,
                BUILDING_REGIONAL_CORE,
                Some(TileSlotKey::Primary),
            )
            .is_err(),
            "Central government must NOT directly overwrite a subordinate Polity's own SOE slot!"
        );
        // Skip-level fiscal transfer (`Central -> Provincial`) conserves cash:
        game.transfer_polity_funds(PolityId(0), prov_polity, 100 * CASH_SCALE)
            .unwrap();

        // Inject profit into a `LocalPrivate` workshop in `other_sec` AND into `prov_polity`'s own SOE,
        // then run settlement to verify:
        // - `60%` of taxes in `other_sec` go directly to `PolityId(0)` and `40%` go to `prov_polity`;
        // - `100%` of `prov_polity` SOE surplus sweeps into `prov_polity.vault`!
        for (_, (_p, pt)) in game.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[other_tiles[0]][TileSlotKey::Primary.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 400 * CASH_SCALE;
            }
            if let Some(ws) =
                pt.workshops[prov_primary_tile][TileSlotKey::Primary.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 400 * CASH_SCALE;
            }
        }
        let m_pre_tax = game.planet_money_supply_cents("base:earth");
        game.settle_planet_month("base:earth", PolityId(0));
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            m_pre_tax,
            "source tax-sharing + multi-polity SOE sweep must strictly conserve cash!"
        );
        let central_snap = game.polity_snapshot(PolityId(0)).unwrap();
        let prov_snap = game.polity_snapshot(prov_polity).unwrap();
        assert!(
            central_snap.last_tax_income > 0 && prov_snap.last_tax_income > 0,
            "both Central and Provincial vaults must receive source-split tax revenue ({} vs {})",
            central_snap.last_tax_income,
            prov_snap.last_tax_income
        );
        assert!(
            prov_snap.last_state_enterprise_net > 0,
            "Provincial SOE surplus must sweep into Provincial vault, got {}",
            prov_snap.last_state_enterprise_net
        );

        // 4. Vic3 / Manchuria (`满铁`) Foreign Direct Investment Rights & Sovereign Forced Expropriation:
        //    Spawn an external foreign sovereign (`PolityId(2)`), verify it CANNOT build without investment rights,
        //    grant treaty investment rights in `other_sec`, build a foreign state orchard (`State { foreign }`),
        //    verify its post-tax profit sweeps to the foreign treasury while paying host taxes to `other_sec`,
        //    and then execute `expropriate_slot` (`强制无偿没收`) — seizing ownership and deducting 20 credit rating!
        let foreign_polity = game
            .create_polity(
                "外星特许帝国".into(),
                None,
                150,
                0,
                600 * CASH_SCALE,
                None,
            )
            .unwrap();
        assert!(
            game.build_on_tile_as(
                foreign_polity,
                "base:earth",
                prov_primary_tile,
                BUILDING_APPLE_ORCHARD,
                Some(TileSlotKey::Secondary0),
            )
            .is_err(),
            "foreign Polity without treaty investment rights must be blocked from building!"
        );
        game.grant_investment_right(PolityId(0), "base:earth", other_sec, foreign_polity)
            .unwrap();
        game.build_on_tile_as(
            foreign_polity,
            "base:earth",
            prov_primary_tile,
            BUILDING_APPLE_ORCHARD,
            Some(TileSlotKey::Secondary0),
        )
        .unwrap();
        for (_, (_p, pt)) in game.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) =
                pt.workshops[prov_primary_tile][TileSlotKey::Secondary0.as_index()].as_mut()
            {
                ws.inv.available[usize::from(cash_id.0)] = 350 * CASH_SCALE;
            }
        }
        let foreign_before = game.polity_snapshot(foreign_polity).unwrap().treasury_cents;
        let m_pre_fdi = game.planet_money_supply_cents("base:earth");
        game.settle_planet_month("base:earth", PolityId(0));
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            m_pre_fdi,
            "foreign state workshop host-tax + repatriation sweep must strictly conserve cash!"
        );
        let foreign_after = game.polity_snapshot(foreign_polity).unwrap().treasury_cents;
        assert!(
            foreign_after > foreign_before,
            "foreign state workshop post-tax profit must repatriate to foreign Polity vault ({foreign_before} -> {foreign_after})"
        );

        let rating_before_seize = game.polity_snapshot(PolityId(0)).unwrap().credit_rating;
        let m_pre_seize = game.planet_money_supply_cents("base:earth");
        game.expropriate_slot(
            PolityId(0),
            "base:earth",
            prov_primary_tile,
            TileSlotKey::Secondary0,
        )
        .unwrap();
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            m_pre_seize,
            "forced expropriation without compensation must strictly conserve cash"
        );
        let rating_after_seize = game.polity_snapshot(PolityId(0)).unwrap().credit_rating;
        assert_eq!(
            rating_after_seize,
            rating_before_seize - 20,
            "EVENT_FORCED_EXPROPRIATION must deduct 20 credit rating points from seizing sovereign"
        );
    }

    #[test]
    fn external_gamedata_hot_reload_in_place_rejects_bad_jsonc_and_resizes_live_inventories() {
        use std::fs;

        let src_dir = crate::data_bundle::resolve_gamedata_dir()
            .expect("外部 gamedata 目录必须存在");
        let tmp_root = std::env::temp_dir().join(format!(
            "space_strategy_hot_reload_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(tmp_root.join("catalog/buildings")).unwrap();
        fs::create_dir_all(tmp_root.join("rules")).unwrap();
        fs::create_dir_all(tmp_root.join("maps")).unwrap();

        for rel in [
            "catalog/pop_types.jsonc",
            "catalog/goods.jsonc",
            "catalog/transport_hubs.jsonc",
            "rules/ai_decision.jsonc",
            "rules/events.jsonc",
            "maps/earth_authored.json",
        ] {
            fs::copy(src_dir.join(rel), tmp_root.join(rel)).unwrap();
        }
        for entry in fs::read_dir(src_dir.join("catalog/buildings")).unwrap() {
            let p = entry.unwrap().path();
            if let Some(name) = p.file_name() {
                fs::copy(&p, tmp_root.join("catalog/buildings").join(name)).unwrap();
            }
        }

        let mut game = Game::default();
        game.set_monetary_policy("base:earth", MonetaryPolicy::Tight)
            .unwrap();
        for _ in 0..60 {
            game.step_once();
        }
        let earth_sec_ids: Vec<u16> = game
            .planet_by_id("base:earth")
            .unwrap()
            .sectors
            .iter()
            .map(|s| s.id)
            .collect();
        for sid in earth_sec_ids {
            game.toggle_private_land_use("base:earth", sid, false)
                .unwrap();
        }
        let cash_before = game.planet_money_supply_cents("base:earth");
        let goods_len_before = game.goods().len();

        // 1. Broken JSONC syntax must be rejected atomically without corrupting live state
        let ai_path = tmp_root.join("rules/ai_decision.jsonc");
        let orig_ai = fs::read_to_string(&ai_path).unwrap();
        fs::write(&ai_path, "{ broken_jsonc_syntax ").unwrap();
        assert!(
            game.reload_data_in_place_from_dir(&tmp_root).is_err(),
            "损坏的 JSONC 必须被前置校验拦截"
        );
        assert_eq!(game.ai_config().max_payback_months, 20);
        assert_eq!(game.planet_money_supply_cents("base:earth"), cash_before);

        // 2. Patch AI decision parameter + building cost + append a new commodity in goods.jsonc
        fs::write(
            &ai_path,
            orig_ai.replace("\"max_payback_months\": 20", "\"max_payback_months\": 14"),
        )
        .unwrap();

        let civic_path = tmp_root.join("catalog/buildings/01_civic_commercial.jsonc");
        let orig_civic = fs::read_to_string(&civic_path).unwrap();
        fs::write(
            &civic_path,
            orig_civic.replace("\"build_cost\": 800", "\"build_cost\": 777"),
        )
        .unwrap();

        let goods_path = tmp_root.join("catalog/goods.jsonc");
        let orig_goods = fs::read_to_string(&goods_path).unwrap();
        let last_bracket = orig_goods.rfind(']').unwrap();
        let patched_goods = format!(
            "{},\n  {{ \"id\": \"mod:fusion_cell\", \"name\": \"聚变电池\", \"unit_volume\": 1, \"genesis_price_cents\": 1500, \"retention_permille\": 990, \"transport_channel\": \"physical\" }}\n]",
            orig_goods[..last_bracket].trim_end().trim_end_matches(',')
        );
        fs::write(&goods_path, patched_goods).unwrap();

        let msg = game
            .reload_data_in_place_from_dir(&tmp_root)
            .expect("合法增量 JSONC 原位热重载必须成功");
        assert!(msg.contains("已热重载外部数据"));
        assert_eq!(game.ai_config().max_payback_months, 14);
        assert_eq!(
            game.buildings()
                .by_id("base:planetary_capital")
                .unwrap()
                .build_cost,
            777
        );
        assert_eq!(game.goods().len(), goods_len_before + 1);
        assert!(game.goods().id_of("mod:fusion_cell").is_some());
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            cash_before,
            "原位热重载并扩容仓库绝不可破坏货币守恒"
        );

        // 3. Advance another full month to verify resized inventories & books settle cleanly
        for _ in 0..30 {
            game.step_once();
        }
        assert_eq!(
            game.planet_money_supply_cents("base:earth"),
            cash_before,
            "热重载后继续推进月度结算仍须保持严格货币守恒"
        );

        // 4. Reload & restart world also succeeds cleanly
        game.reload_and_restart_from_dir(&tmp_root)
            .expect("重载并重开对局必须成功");
        assert_eq!(game.counters().monthly_ticks, 0);
        assert!(game.goods().id_of("mod:fusion_cell").is_some());

        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn colony_frontier_claiming_core_sites_pioneer_outpost_and_split_province_conservation() {
        use crate::{
            CalendarConfig, GameSpeed, MonetaryPolicy, ScenarioPreset, BUILDING_COLONIAL_OUTPOST,
            BUILDING_REGIONAL_CORE,
        };

        // 1. Both Earth & Colony surfaces mark `CoreSite` in every land macro-region.
        let earth_game = Game::with_scenario_preset(CalendarConfig::default(), ScenarioPreset::Earth)
            .expect("地球剧本初始化成功");
        let earth_surf = earth_game.planet_surface_by_id("base:earth").unwrap();
        for reg in earth_surf.regions.iter().filter(|r| !r.is_ocean && r.land_tile_count > 0) {
            let core_cnt = earth_surf
                .tiles
                .iter()
                .filter(|t| t.region_id == reg.id && t.tags.has(TileTag::CoreSite))
                .count();
            assert!(core_cnt >= 1, "地球陆区 {} 必须至少拥有 1 个中心选址格", reg.name);
        }

        // 2. Colony Start (`殖民地开局`): 9 claimed land tiles, 22 admin capacity (`4 + 18`), rest wilderness.
        let mut game =
            Game::with_scenario_preset(CalendarConfig::default(), ScenarioPreset::Colony)
                .expect("殖民地剧本初始化成功");
        game.set_monetary_policy("base:colony", MonetaryPolicy::Tight)
            .unwrap();
        game.set_speed(GameSpeed::Speed4);

        let snap0 = game.planet_by_id("base:colony").unwrap();
        let surf = game.planet_surface_by_id("base:colony").unwrap();
        assert_eq!(snap0.scenario, ScenarioPreset::Colony);
        assert_eq!(snap0.sectors.len(), 1);
        assert_eq!(snap0.polity.admin_used, 9);
        assert_eq!(snap0.polity.admin_capacity, 22);
        assert_eq!(std::mem::size_of::<crate::Tile>(), 24, "Tile 必须保持 <= 24 字节紧凑布局");

        // Building on an unclaimed tile is rejected; non-contiguous claiming is rejected.
        let far_unclaimed = surf
            .tiles
            .iter()
            .find(|t| {
                t.tags.has(TileTag::Land)
                    && snap0.tile_sectors[usize::from(t.index)].is_none()
                    && t.neighbors
                        .iter()
                        .all(|&nb| snap0.tile_sectors[usize::from(nb)].is_none())
            })
            .expect("存在远端不接壤荒野格");
        assert!(
            game.claim_tile("base:colony", usize::from(far_unclaimed.index))
                .is_err(),
            "不接壤荒野格不可隔空框入"
        );
        assert!(
            game.build_on_tile(
                "base:colony",
                usize::from(far_unclaimed.index),
                "base:open_farm",
                None
            )
            .is_err(),
            "无主荒野格不可直接建厂"
        );

        // 3. Trek across a geographic `SurfaceRegion` border to a neighbor's `CoreSite`!
        let start_reg = surf.tiles[snap0.starter_tile_index].region_id;
        for (_, pol) in game.world.query_mut::<&mut Polity>() {
            if pol.id == PolityId(0) {
                pol.base_admin_capacity = 60;
            }
        }
        let _ = game.claim_region_frontier("base:colony", start_reg, 32);
        let snap_after_home = game.planet_by_id("base:colony").unwrap();

        let neighbor_reg_opt = surf
            .regions
            .iter()
            .filter(|r| !r.is_ocean && r.id != start_reg && r.land_tile_count > 0)
            .find(|r| {
                surf.tiles.iter().any(|t| {
                    t.region_id == r.id
                        && t.tags.has(TileTag::Land)
                        && snap_after_home.tile_sectors[usize::from(t.index)].is_none()
                        && t.neighbors.iter().any(|&nb| {
                            snap_after_home.tile_sectors[usize::from(nb)].is_some()
                        })
                })
            })
            .map(|r| r.id);

        if let Some(nb_reg) = neighbor_reg_opt {
            let snap_nb = game
                .claim_region_frontier("base:colony", nb_reg, 32)
                .expect("跨入相邻空白大区自动设立新 Sector");
            assert!(snap_nb.sectors.len() >= 2, "跨大区框地必须自动创建新实控辖区 Sector");

            let nb_core_tile = surf
                .tiles
                .iter()
                .find(|t| {
                    t.region_id == nb_reg
                        && t.tags.has(TileTag::CoreSite)
                        && snap_nb.tile_sectors[usize::from(t.index)].is_some()
                })
                .map(|t| usize::from(t.index));

            if let Some(core_idx) = nb_core_tile {
                let pop_before = game.planet_by_id("base:colony").unwrap().total_population;
                let money_before = game.planet_money_supply_cents("base:colony");
                let admin_before = game.polity_admin_usage(PolityId(0)).1;

                // Founding `殖民据点` in blank `nb_reg` conserve-migrates pioneers & staffs immediately!
                let after_outpost = game
                    .build_on_tile(
                        "base:colony",
                        core_idx,
                        BUILDING_COLONIAL_OUTPOST,
                        Some(TileSlotKey::Primary),
                    )
                    .expect("在中心选址格建造殖民据点成功");
                assert_eq!(
                    after_outpost.total_population, pop_before,
                    "跨区先遣迁工不得凭空增减星球总人口"
                );
                assert_eq!(
                    game.planet_money_supply_cents("base:colony"),
                    money_before - 220 * CASH_SCALE,
                    "建造据点与先遣迁工仅扣除建造成本，家庭储蓄严格守恒"
                );
                let admin_after = game.polity_admin_usage(PolityId(0)).1;
                assert!(
                    admin_after > admin_before,
                    "新建殖民据点立即在岗并提供额外 base:admin 额度"
                );

                // Upgrading `殖民据点 -> 大区核心` in-place on the same `CoreSite` succeeds!
                game.build_on_tile(
                    "base:colony",
                    core_idx,
                    BUILDING_REGIONAL_CORE,
                    Some(TileSlotKey::Primary),
                )
                .expect("殖民据点可原位改建升级为大区核心");
            }
        }

        // 4. Split-province test: a second sovereign `Polity` claiming unclaimed land in an already-settled
        //    geographic `SurfaceRegion` spawns a separate `Sector(geo_region_id, PolityId(1))`!
        let rival_pol = game
            .create_polity("边疆自治邦".into(), None, 120, 0, 500 * CASH_SCALE, Some(PolityId(0)))
            .unwrap();
        let target_split_tile = surf
            .tiles
            .iter()
            .find(|t| {
                t.tags.has(TileTag::Land)
                    && game.planet_by_id("base:colony").unwrap().tile_sectors[usize::from(t.index)]
                        .is_none()
                    && game
                        .planet_by_id("base:colony")
                        .unwrap()
                        .sectors
                        .iter()
                        .any(|s| s.geo_region_id == t.region_id)
            })
            .or_else(|| {
                surf.tiles.iter().find(|t| {
                    t.tags.has(TileTag::Land)
                        && game.planet_by_id("base:colony").unwrap().tile_sectors
                            [usize::from(t.index)]
                        .is_none()
                })
            })
            .unwrap();
        let split_snap = game
            .claim_tile_as(rival_pol, "base:colony", usize::from(target_split_tile.index))
            .expect("第二政权在同一地理大区框地生成分离省 Sector");
        assert!(
            split_snap
                .sectors
                .iter()
                .any(|s| s.owner_polity_id == rival_pol.0 && s.geo_region_id == target_split_tile.region_id),
            "分离省 Sector(G, rival_pol) 必须成功建立"
        );

        // 5. Pause private land construction (which is an intentional construction sink) and run 3 full
        //    monthly settlement cycles (`Tight` 0% minting) to verify exact single-cent cash conservation!
        let active_sec_ids: Vec<u16> = game
            .planet_by_id("base:colony")
            .unwrap()
            .sectors
            .iter()
            .map(|s| s.id)
            .collect();
        for sid in active_sec_ids {
            game.toggle_private_land_use("base:colony", sid, false)
                .unwrap();
        }
        let cash_before_sim = game.planet_money_supply_cents("base:colony");
        for _ in 0..90 {
            game.step_once();
        }
        assert_eq!(
            game.planet_money_supply_cents("base:colony"),
            cash_before_sim,
            "殖民地剧本多政权分离省连续 3 个月结算必须严格保持货币守恒"
        );
    }

    #[test]
    fn colony_six_year_sim_keeps_staple_prices_stable_and_avoids_mass_destitution() {
        use crate::{CalendarConfig, GameSpeed, ScenarioPreset};

        let mut game =
            Game::with_scenario_preset(CalendarConfig::default(), ScenarioPreset::Colony)
                .expect("殖民地剧本初始化成功");
        game.set_speed(GameSpeed::Speed4);

        // Run 72 full monthly cycles (6 full in-game years = 2,160 days) with autonomous AI & Moderate minting
        for _ in 0..(30 * 72) {
            game.step_once();
        }

        let snap = game.planet_by_id("base:colony").unwrap();
        let sec = &snap.sectors[0];
        let staple_book = sec
            .market_books
            .iter()
            .find(|b| b.good_id == snap.primary_staple_id)
            .expect("主粮盘口必须存在");

        let resting_asks: i64 = staple_book.order_book.asks.iter().map(|l| l.qty).sum();
        assert!(
            staple_book.order_book.ma_price_cents <= 1_400,
            "殖民星运行 6 年后主粮均价不得恶性失控锁死（期望 <= 14.00 CR，实际 {} cents）",
            staple_book.order_book.ma_price_cents
        );
        assert!(
            sec.apple_supply + resting_asks >= sec.apple_demand * 2,
            "殖民星着陆区水电农闭环 6 年后主粮产储必须充足覆盖需求（月产 {} + 余粮 {} vs 需求 {}）",
            sec.apple_supply,
            resting_asks,
            sec.apple_demand
        );
        assert!(
            sec.vagrant_count * 3 <= sec.population,
            "殖民星运行 6 年不得发生大规模流民赤贫崩盘（流民 {}/总人口 {}）",
            sec.vagrant_count,
            sec.population
        );
    }

    #[test]
    fn entity_deficit_coverage_no_auto_soe_layoff_and_private_bankruptcy_salvage_options_work() {
        use crate::{
            CalendarConfig, CounterpartyId, GameSpeed, PolityId, ScenarioPreset, TileSlotKey,
        };

        // 1. State-owned workshop (`BuildingOwner::State`) running at an operating deficit is
        //    funded by `Polity.vault` without autonomous layoff, conserving exact money supply!
        let mut game =
            Game::with_scenario_preset(CalendarConfig::default(), ScenarioPreset::Colony)
                .expect("殖民地剧本初始化成功");
        game.set_monetary_policy("base:colony", crate::MonetaryPolicy::Tight)
            .unwrap();
        let snap0 = game.planet_by_id("base:colony").unwrap();
        let starter = snap0.starter_tile_index;

        // Build an oversized 2x renewable power plant as `State` on a claimed land tile so output exceeds demand
        let land_tile = snap0
            .tile_sectors
            .iter()
            .enumerate()
            .find_map(|(ti, sec_opt)| {
                (sec_opt.is_some()
                    && ti != starter
                    && game
                        .planet_surface_by_id("base:colony")
                        .unwrap()
                        .tiles[ti]
                        .tags
                        .has(TileTag::Land))
                .then_some(ti)
            })
            .expect("存在可用已框入陆地格");
        game.build_on_tile(
            "base:colony",
            land_tile,
            "base:renewable_power_plant",
            Some(TileSlotKey::Primary),
        )
        .expect("国库出资建造国有电站成功");

        let money_after_build = game.planet_money_supply_cents("base:colony");
        for _ in 0..(30 * 12) {
            game.step_once();
        }
        assert_eq!(
            game.planet_money_supply_cents("base:colony"),
            money_after_build,
            "紧缩零增发下国企亏损由国库双向拨补必须严格保持全盘货币守恒"
        );
        let snap_1y = game.planet_by_id("base:colony").unwrap();
        let state_plant = snap_1y
            .occupied_tiles
            .iter()
            .find(|t| t.index == land_tile)
            .and_then(|t| t.primary.as_ref())
            .expect("国有电站必须保留在原主槽");
        assert!(
            state_plant.jobs_filled > 0,
            "国库充裕时亏损国有重点工场由财政兜底，不得自动裁员清零（当前在岗 {}）",
            state_plant.jobs_filled
        );

        // 2. `LocalPrivate` bankruptcy offers two clean paths: government salvage acquisition OR clean teardown
        //    (without turning bankrupt primary slots into 0-worker state zombie shells, and keeping independent secondaries!).
        let mut game2 =
            Game::with_scenario_preset(CalendarConfig::default(), ScenarioPreset::Colony)
                .expect("殖民地剧本初始化成功");
        game2
            .set_monetary_policy("base:colony", crate::MonetaryPolicy::Tight)
            .unwrap();
        // Bind `PolityId(0)` as player-controlled event actor to verify declarative 2-option `PendingEvent`
        game2.set_controlled_actor(Some(CounterpartyId::Polity {
            polity_id: PolityId(0),
        }));

        // Place a `LocalPrivate` primary plant + an independent `LocalPrivate` secondary plant on `land_tile`
        game2
            .build_on_tile(
                "base:colony",
                land_tile,
                "base:renewable_power_plant",
                Some(TileSlotKey::Primary),
            )
            .unwrap();
        game2
            .build_on_tile(
                "base:colony",
                land_tile,
                "base:desalination_plant",
                Some(TileSlotKey::Secondary0),
            )
            .unwrap();
        // Inject pool cash and privatize primary slot into `LocalPrivate`, then induce 3-month insolvency
        game2.adjust_cash(
            "base:colony",
            crate::planet::CashSink::Capital(SectorId(0)),
            500 * CASH_SCALE,
        );
        game2
            .privatize_slot("base:colony", land_tile, TileSlotKey::Primary)
            .unwrap();

        let cash_idx = usize::from(game2.goods.cash_id().0);
        let worker_pop_id = {
            let mut pid_found = None;
            for (_, (_, pops)) in game2.world.query::<(&Sector, &SectorPops)>().iter() {
                if let Some(g) = pops.groups.first() {
                    pid_found = Some(g.id);
                    break;
                }
            }
            pid_found.unwrap()
        };
        let mut drained = 0i64;
        for (_, (_, pt)) in game2.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[land_tile][TileSlotKey::Primary.as_index()].as_mut() {
                drained = ws.inv.available[cash_idx];
                ws.inv.available[cash_idx] = 0;
                for v in ws.inv.available.iter_mut() {
                    *v = 0;
                }
                ws.record_unpaid_claim(worker_pop_id, 1_500);
                ws.had_wage_arrears = true;
                ws.consecutive_arrears_months = 3;
            }
        }
        game2.adjust_cash(
            "base:colony",
            crate::planet::CashSink::Capital(SectorId(0)),
            drained,
        );
        let money_before_ev = game2.planet_money_supply_cents("base:colony");

        // Advance 1 month so `EVENT_PRIVATE_BANKRUPTCY` queues a 2-option `PendingEvent`
        for _ in 0..30 {
            game2.step_once();
        }
        let bank_ev = game2
            .snapshot()
            .pending_events
            .into_iter()
            .find(|pe| pe.event_id == crate::rules::EVENT_PRIVATE_BANKRUPTCY)
            .expect("私营工场资不抵债满 3 个月必须触发破产二选一事件");
        assert_eq!(bank_ev.enabled_options.len(), 2, "国库充足时必须同时提供收购与清拆两个选项");

        // Clone world state or test Option 0 (salvage acquisition) first on `game2`!
        game2
            .resolve_pending_event(bank_ev.instance_id, 0)
            .expect("执行选项 0：按破产拆解残值收购成功");
        assert_eq!(
            game2.planet_money_supply_cents("base:colony"),
            money_before_ev,
            "政府按残值收购破产私企并清偿欠薪、注入周转金必须严格守恒"
        );
        let acq_snap = game2.planet_by_id("base:colony").unwrap();
        let acq_tile = acq_snap
            .occupied_tiles
            .iter()
            .find(|t| t.index == land_tile)
            .unwrap();
        assert_eq!(
            acq_tile.primary.as_ref().unwrap().owner.kind,
            "state",
            "破产收购后槽位产权必须转为政府公有"
        );

        // Also verify Option 1 (decline acquisition -> clean teardown without zombie shell, keeping Secondary0!)
        game2
            .toggle_private_land_use("base:colony", 0, false)
            .unwrap();
        game2.adjust_cash(
            "base:colony",
            crate::planet::CashSink::Capital(SectorId(0)),
            600 * CASH_SCALE,
        );
        game2
            .privatize_slot("base:colony", land_tile, TileSlotKey::Primary)
            .unwrap();
        let mut drained2 = 0i64;
        for (_, (_, pt)) in game2.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if let Some(ws) = pt.workshops[land_tile][TileSlotKey::Primary.as_index()].as_mut() {
                drained2 = ws.inv.available[cash_idx];
                for v in ws.inv.available.iter_mut() {
                    *v = 0;
                }
                ws.record_unpaid_claim(worker_pop_id, 1_200);
                ws.had_wage_arrears = true;
                ws.consecutive_arrears_months = 3;
            }
        }
        game2.adjust_cash(
            "base:colony",
            crate::planet::CashSink::Capital(SectorId(0)),
            drained2,
        );
        for _ in 0..30 {
            game2.step_once();
        }
        let bank_ev2 = game2
            .snapshot()
            .pending_events
            .into_iter()
            .find(|pe| pe.event_id == crate::rules::EVENT_PRIVATE_BANKRUPTCY)
            .expect("第二次破产事件入队");
        game2
            .resolve_pending_event(bank_ev2.instance_id, 1)
            .expect("执行选项 1：拒绝收购直接清拆成功");
        assert_eq!(
            game2.planet_money_supply_cents("base:colony"),
            money_before_ev + 600 * CASH_SCALE,
            "破产直接清拆全过程货币必须严格守恒"
        );
        let torn_snap = game2.planet_by_id("base:colony").unwrap();
        let torn_tile = torn_snap
            .occupied_tiles
            .iter()
            .find(|t| t.index == land_tile)
            .expect("同地块独立次槽建筑必须继续保留");
        assert!(
            torn_tile.primary.is_none(),
            "拒绝收购时破产主槽必须彻底清空，不得残留 0 人国有死壳"
        );
        assert!(
            torn_tile.secondary[0].is_some(),
            "主槽破产清拆后，无主槽前置依赖的普通次槽建筑必须继续独立运行"
        );

        // 3. Long-horizon 20-year (240 months) Colony simulation stays solvent & avoids 3 CR deflation spiral!
        let mut long_game =
            Game::with_scenario_preset(CalendarConfig::default(), ScenarioPreset::Colony)
                .expect("殖民地剧本初始化成功");
        long_game.set_speed(GameSpeed::Speed4);
        for _ in 0..(30 * 240) {
            long_game.step_once();
        }
        let snap_20y = long_game.planet_by_id("base:colony").unwrap();
        let sec_20y = &snap_20y.sectors[0];
        let ag_20y = sec_20y
            .market_books
            .iter()
            .find(|b| b.good_id == snap_20y.primary_staple_id)
            .unwrap();
        assert!(
            snap_20y.polity.treasury >= 200,
            "殖民星挂机 20 年后国库仍须保持健康盈余（当前 {} CR）",
            snap_20y.polity.treasury
        );
        assert!(
            (450..=1_400).contains(&ag_20y.order_book.ma_price_cents),
            "殖民星挂机 20 年后主粮均价不得跌入亏本通缩或天价死锁（当前 {} cents）",
            ag_20y.order_book.ma_price_cents
        );
    }
