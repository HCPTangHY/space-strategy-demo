//! Planetary Money-Supply & Commodity Conservation Audit Queries (`货币与实物总量守恒核查`).

use crate::economy::{Polity, Sector, SectorCapital, SectorPops};
use crate::market::GoodId;
use crate::planet::{Planet, PlanetTiles};
use crate::Game;

impl Game {
    /// Total `base:cash` cents held by every inventory on a planet plus its ruling polity vault.
    pub fn planet_money_supply_cents(&self, planet_id: &str) -> i64 {
        self.planet_good_total(planet_id, self.goods.cash_id())
    }

    /// Total units of `good` held anywhere on a planet (workshops, corporate & trust vaults,
    /// sector depots, local capital pools, Pops, and ruling polity vault).
    pub fn planet_good_total(&self, planet_id: &str, good: GoodId) -> i64 {
        let mut total = 0i64;
        let mut ruling = None;
        for (_, (planet, pt)) in self.world.query::<(&Planet, &PlanetTiles)>().iter() {
            if planet.id == planet_id {
                ruling = Some(planet.ruling_polity);
                for row in &pt.workshops {
                    for ws in row.iter().flatten() {
                        total += ws.inv.total(good);
                    }
                }
                for ent in &pt.enterprises {
                    total += ent.vault.total(good);
                }
                for tr in &pt.trusts {
                    total += tr.vault.total(good);
                }
            }
        }
        for (_, (sec, cap, pops)) in self
            .world
            .query::<(&Sector, &SectorCapital, &SectorPops)>()
            .iter()
        {
            if sec.planet_id == planet_id {
                total += sec.state_depot.total(good) + cap.vault.total(good);
                total += pops.groups.iter().map(|g| g.inv.total(good)).sum::<i64>();
            }
        }
        for (_, pol) in self.world.query::<&Polity>().iter() {
            total += pol.vault.total(good);
        }
        let _ = ruling;
        total
    }
}
