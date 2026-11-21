//! Runtime External Data Hot-Reload Engine (`外部 gamedata 热重载引擎`).
//!
//! Two safe reload modes:
//! 1. `reload_data_in_place`: Validates all JSONC files first; aligns `GoodId`, `PopTypeId`,
//!    and `BuildingTypeId` handles by stable string `id` without losing current save state,
//!    resizing all live `Inventory`, `SectorMarket`, and `RoleWageBook` containers automatically.
//! 2. `reload_and_restart`: Validates all JSONC files + `maps/earth_authored.json`, reloads the
//!    Earth Voronoi map cache, and rebuilds a fresh world from external disk data.

use crate::building::BuildingTypeId;
use crate::data_bundle::{load_full_gamedata_from_dir, resolve_gamedata_dir};
use crate::economy::{Polity, PopTypeId, Sector, SectorCapital, SectorMarket, SectorPops};
use crate::market::{RoleWageBook, CASH_SCALE};
use crate::planet::{Planet, PlanetTiles};
use crate::surface::{self, DEFAULT_GRID_CELLS};
use crate::Game;
use std::collections::BTreeSet;
use std::path::Path;

impl Game {
    /// Non-destructively hot-reloads `gamedata/` (`catalog/*` + `rules/*`) into the live session.
    pub fn reload_data_in_place(&mut self) -> Result<String, String> {
        let dir = resolve_gamedata_dir()?;
        self.reload_data_in_place_from_dir(&dir)
    }

    /// Non-destructively hot-reloads external data from a specific `gamedata` directory.
    pub fn reload_data_in_place_from_dir(&mut self, dir: &Path) -> Result<String, String> {
        // Step 1: Parse & cross-validate the full external dataset BEFORE mutating any state.
        let bundle = load_full_gamedata_from_dir(dir)?;

        // Step 2: Collect active BuildingTypeIds and PopTypeIds referenced by the live world.
        let mut active_buildings: BTreeSet<BuildingTypeId> = BTreeSet::new();
        for (_, pt) in self.world.query::<&PlanetTiles>().iter() {
            for tile in &pt.tiles {
                if let Some(tid) = tile.slots.primary {
                    active_buildings.insert(tid);
                }
                for tid in tile.slots.secondary.iter().flatten() {
                    active_buildings.insert(*tid);
                }
            }
        }

        let mut active_pops: BTreeSet<PopTypeId> = BTreeSet::new();
        for (_, pops) in self.world.query::<&SectorPops>().iter() {
            for g in &pops.groups {
                if g.count > 0 {
                    active_pops.insert(g.pop_type);
                }
            }
        }

        // Step 3: Validate all three catalog patches on clones first so partial failure cannot occur.
        let mut next_goods = self.goods.clone();
        next_goods.patch_in_place(bundle.economy.goods)?;

        let mut next_pops = self.pop_catalog.clone();
        next_pops.patch_in_place(bundle.economy.pop_types, &active_pops)?;

        let mut next_buildings = self.buildings.clone();
        next_buildings.patch_in_place(bundle.economy.buildings, &active_buildings)?;

        // Step 4: Commit updated catalogs, AI heuristics, transport hubs & events atomically.
        self.goods = next_goods;
        self.pop_catalog = next_pops;
        self.buildings = next_buildings;
        self.transport_hubs = bundle.economy.transport_hubs;
        self.ai_config = bundle.ai_config;
        self.events = bundle.events;

        // Step 5: Resize every live Inventory, SectorMarket, and RoleWageBook for any newly added IDs.
        let good_len = self.goods.len();
        let pop_len = self.pop_catalog.len();
        for (_, pol) in self.world.query_mut::<&mut Polity>() {
            pol.vault.ensure_len(good_len);
        }
        for (_, (_planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            for row in &mut pt.workshops {
                for ws in row.iter_mut().flatten() {
                    ws.inv.ensure_len(good_len);
                }
            }
            for ent in &mut pt.enterprises {
                ent.vault.ensure_len(good_len);
            }
            for tr in &mut pt.trusts {
                tr.vault.ensure_len(good_len);
            }
        }
        for (_, (sec, mkt, cap, pops)) in self.world.query_mut::<(
            &mut Sector,
            &mut SectorMarket,
            &mut SectorCapital,
            &mut SectorPops,
        )>() {
            sec.state_depot.ensure_len(good_len);
            cap.vault.ensure_len(good_len);
            mkt.ensure_good_books(&self.goods);
            while mkt.wage_books.len() < pop_len {
                let idx = mkt.wage_books.len();
                let base_w =
                    (self.pop_catalog.get(PopTypeId(idx as u16)).base_wage_per_100 * CASH_SCALE
                        / 100)
                        .max(15);
                mkt.wage_books.push(RoleWageBook::new(idx as u16, base_w));
            }
            for g in &mut pops.groups {
                g.inv.ensure_len(good_len);
            }
        }

        Ok(format!(
            "已热重载外部数据：{} 种商品 · {} 座建筑 · {} 类职业 · {} 条事件",
            self.goods.len(),
            self.buildings.defs().len(),
            self.pop_catalog.len(),
            self.events.defs().len()
        ))
    }

    /// Reloads all external JSONC catalogs, rules, AND `maps/earth_authored.json`, then restarts the world.
    pub fn reload_and_restart(&mut self) -> Result<String, String> {
        let dir = resolve_gamedata_dir()?;
        self.reload_and_restart_from_dir(&dir)
    }

    /// Reloads all external data + Earth map from `dir` and rebuilds the world from day 1.
    pub fn reload_and_restart_from_dir(&mut self, dir: &Path) -> Result<String, String> {
        let full = load_full_gamedata_from_dir(dir)?;
        surface::reload_earth_map_from_dir(dir)?;

        let cal = self.calendar.clone();
        let cur_scenario = self.scenario;
        let controlled = self.controlled_actor;
        let mut fresh = Self::with_catalogs_and_hubs_scenario(
            cal,
            cur_scenario,
            full.economy.goods,
            full.economy.buildings,
            full.economy.pop_types,
            full.economy.transport_hubs,
            full.events,
            DEFAULT_GRID_CELLS,
        );
        fresh.ai_config = full.ai_config;
        fresh.controlled_actor = controlled;
        *self = fresh;

        let scenario_label = match cur_scenario {
            crate::protocol::ScenarioPreset::Earth => "地球开局",
            crate::protocol::ScenarioPreset::Colony => "殖民地开局",
        };
        Ok(format!(
            "已重载外部数据并重开「{scenario_label}」（{} 种商品 · {} 座建筑）",
            self.goods.len(),
            self.buildings.defs().len()
        ))
    }
}
