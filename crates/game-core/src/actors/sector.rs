//! Sector, Local Private Capital Pool & Geographic Trade Topology Tier (`大区辖区、民间池与空间交通拓扑层`).

use super::household::PopTypeCatalog;
use super::ids::{PolityId, SectorId};
use super::polity::DepotPolicy;
use crate::market::{GoodCatalog, GoodId, Inventory, OrderBookSnapshot, RoleWageBook, SectorOrderBook, CASH_SCALE};
use crate::surface::{TileTag, TileTags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Controlled sub-regional ECS entity (`实控辖区` + distributed `state_depot`!).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sector {
    pub id: SectorId,
    pub planet_id: String,
    pub geo_region_id: u16,
    pub owner_polity: PolityId,
    /// Immediate governing `Polity` (defaults to `owner_polity` for central direct rule `直辖`;
    /// may point to a subordinate planetary/regional `Polity` under tax-sharing).
    #[serde(default)]
    pub admin_polity: PolityId,
    /// External `Polity`s granted treaty-based foreign direct investment rights in this sector.
    #[serde(default)]
    pub foreign_investment_rights: BTreeSet<PolityId>,
    pub name: String,
    pub allow_private_land_use: bool,
    pub habitability_sum_permille: u32,
    pub is_coastal: bool,
    pub state_depot: Inventory,
    pub depot_policy: DepotPolicy,
}

/// Data-driven multi-good sector market: automatically maintains one `SectorOrderBook` per non-currency
/// good in `GoodCatalog` (commodities, bonds, `Trust` equity shares).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorMarket {
    pub good_books: BTreeMap<GoodId, SectorOrderBook>,
    pub wage_books: Vec<RoleWageBook>,
    pub last_production: i64,
    pub last_subsistence_output: i64,
    pub last_consumption: i64,
    /// Per-good order-book depth at the close of the last settlement (resting orders are released afterwards).
    #[serde(skip)]
    pub closing_depths: BTreeMap<GoodId, OrderBookSnapshot>,
}

impl SectorMarket {
    pub fn new(good_catalog: &GoodCatalog, pop_catalog: &PopTypeCatalog) -> Self {
        let mut good_books = BTreeMap::new();
        for gid in good_catalog.tradable_good_ids() {
            let genesis = good_catalog.get(gid).genesis_price_cents;
            good_books.insert(gid, SectorOrderBook::new(gid, genesis));
        }
        let wage_books = pop_catalog
            .defs()
            .iter()
            .enumerate()
            .map(|(i, d)| RoleWageBook::new(i as u16, d.base_wage_per_100 * CASH_SCALE / 100))
            .collect();
        Self {
            good_books,
            wage_books,
            last_production: 0,
            last_subsistence_output: 0,
            last_consumption: 0,
            closing_depths: BTreeMap::new(),
        }
    }

    /// Automatically creates missing `SectorOrderBook`s whenever new goods/securities are registered.
    pub fn ensure_good_books(&mut self, good_catalog: &GoodCatalog) {
        for gid in good_catalog.tradable_good_ids() {
            self.good_books
                .entry(gid)
                .or_insert_with(|| SectorOrderBook::new(gid, good_catalog.get(gid).genesis_price_cents));
        }
    }

    #[inline]
    pub fn book(&self, good: GoodId) -> Option<&SectorOrderBook> {
        self.good_books.get(&good)
    }

    #[inline]
    pub fn book_mut(&mut self, good: GoodId) -> Option<&mut SectorOrderBook> {
        self.good_books.get_mut(&good)
    }

    #[inline]
    pub fn apple_book(&self) -> &SectorOrderBook {
        self.good_books
            .get(&GoodId(1))
            .or_else(|| self.good_books.values().next())
            .expect("至少包含一种可交易商品盘口")
    }

    #[inline]
    pub fn apple_book_mut(&mut self) -> &mut SectorOrderBook {
        if self.good_books.contains_key(&GoodId(1)) {
            self.good_books.get_mut(&GoodId(1)).unwrap()
        } else {
            self.good_books.values_mut().next().expect("至少包含一种可交易商品盘口")
        }
    }

    #[inline]
    pub fn closing_depth(&self, good: GoodId) -> Option<&OrderBookSnapshot> {
        self.closing_depths.get(&good)
    }
}

/// Local private investment pool backed by a unified `Inventory` (`base:cash` in cents).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorCapital {
    pub vault: Inventory,
    pub last_dividends_cents: i64,
    pub last_reinvested_cents: i64,
}

impl SectorCapital {
    pub fn new(good_catalog: &GoodCatalog, initial_credits: i64) -> Self {
        let mut vault = Inventory::new(good_catalog.len(), 0);
        let cash_id = good_catalog.cash_id();
        vault.available[usize::from(cash_id.0)] = initial_credits.max(0) * CASH_SCALE;
        Self {
            vault,
            last_dividends_cents: 0,
            last_reinvested_cents: 0,
        }
    }

    #[inline]
    pub fn pool_cents(&self, good_catalog: &GoodCatalog) -> i64 {
        self.vault.avail(good_catalog.cash_id())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorTradeLink {
    pub sector_a: SectorId,
    pub sector_b: SectorId,
    pub monthly_capacity: i64,
    pub transport_fee: i64,
    pub is_maritime: bool,
}

/// Static planetary natural-geography transport matrix:
/// - `land_terrain_mult[a * R + b]`: `0` if not land-adjacent, else `1` (plains), `2` (hills/forest/arid/tundra), `3` (mountains);
/// - `sea_steps[a * R + b]`: region-level water BFS fallback (`u16::MAX` if unreachable);
/// - `tile_coast_id[tile]`: coast segment ID (`0..coast_count`, `u16::MAX` if non-coastal) partitioning East/West coasts & inland-sea shores;
/// - `coast_sea_steps[ca * C + cb]`: exact `O(1)` water-only BFS distance between coast segments (`u16::MAX` if water-disconnected);
/// - `air_steps[a * R + b]`: point-to-point ballistic distance steps (`1..=12`) between region centers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeoTransportTopology {
    pub region_count: usize,
    pub land_terrain_mult: Vec<u8>,
    pub sea_steps: Vec<u16>,
    #[serde(default)]
    pub coast_count: usize,
    #[serde(default)]
    pub tile_coast_id: Vec<u16>,
    #[serde(default)]
    pub coast_sea_steps: Vec<u16>,
    pub air_steps: Vec<u16>,
}

impl GeoTransportTopology {
    pub const UNREACHABLE_STEPS: u16 = u16::MAX;

    pub fn compute(
        regions: &[crate::surface::SurfaceRegion],
        tile_regions: &[u16],
        tile_tags: &[TileTags],
        tile_neighbors: &[Vec<u16>],
    ) -> Self {
        let r_count = regions.len();
        if r_count == 0 {
            return Self::default();
        }

        // 1. Land border terrain multiplier (`1..=3` across shared non-water edges, `0` if non-adjacent)
        let mut edge_sum = vec![0u32; r_count * r_count];
        let mut edge_cnt = vec![0u32; r_count * r_count];
        let mut coastal_waters_of: Vec<Vec<usize>> = vec![Vec::new(); r_count];

        for (idx, nbs) in tile_neighbors.iter().enumerate() {
            let ra = usize::from(*tile_regions.get(idx).unwrap_or(&u16::MAX));
            if ra >= r_count {
                continue;
            }
            let ta = tile_tags[idx];
            if !ta.is_water() && ta.has(TileTag::Coastal) {
                for &nb_u16 in nbs {
                    let nb = usize::from(nb_u16);
                    if tile_tags.get(nb).copied().unwrap_or_default().is_water() {
                        coastal_waters_of[ra].push(nb);
                    }
                }
            }
            if ta.is_water() {
                continue;
            }
            for &nb_u16 in nbs {
                let nb = usize::from(nb_u16);
                if nb <= idx {
                    continue;
                }
                let rb = usize::from(*tile_regions.get(nb).unwrap_or(&u16::MAX));
                if rb >= r_count || ra == rb {
                    continue;
                }
                let tb = tile_tags[nb];
                if tb.is_water() {
                    continue;
                }
                let mult: u32 = if ta.has(TileTag::Mountains) || tb.has(TileTag::Mountains) {
                    3
                } else if ta.has(TileTag::Hills)
                    || tb.has(TileTag::Hills)
                    || ta.has(TileTag::Arid)
                    || tb.has(TileTag::Arid)
                    || ta.has(TileTag::Tundra)
                    || tb.has(TileTag::Tundra)
                    || ta.has(TileTag::Forest)
                    || tb.has(TileTag::Forest)
                {
                    2
                } else {
                    1
                };
                edge_sum[ra * r_count + rb] += mult;
                edge_cnt[ra * r_count + rb] += 1;
                edge_sum[rb * r_count + ra] += mult;
                edge_cnt[rb * r_count + ra] += 1;
            }
        }

        let mut land_terrain_mult = vec![0u8; r_count * r_count];
        for i in 0..(r_count * r_count) {
            if edge_cnt[i] > 0 {
                let avg = (edge_sum[i] + edge_cnt[i] / 2) / edge_cnt[i];
                land_terrain_mult[i] = avg.clamp(1, 3) as u8;
            }
        }

        // 2. Water-only BFS for maritime `sea_steps` (naturally isolates inland seas from global oceans!)
        for v in &mut coastal_waters_of {
            v.sort_unstable();
            v.dedup();
        }
        let tile_count = tile_tags.len();
        let mut water_adj_regions: Vec<Vec<usize>> = vec![Vec::new(); tile_count];
        for (r, waters) in coastal_waters_of.iter().enumerate() {
            for &w in waters {
                water_adj_regions[w].push(r);
            }
        }

        let mut sea_steps = vec![Self::UNREACHABLE_STEPS; r_count * r_count];
        for r in 0..r_count {
            sea_steps[r * r_count + r] = 0;
            if coastal_waters_of[r].is_empty() {
                continue;
            }
            let mut dist = vec![u16::MAX; tile_count];
            let mut q = std::collections::VecDeque::new();
            for &w in &coastal_waters_of[r] {
                dist[w] = 1;
                q.push_back(w);
            }
            while let Some(cur) = q.pop_front() {
                let d = dist[cur];
                for &other_r in &water_adj_regions[cur] {
                    if other_r != r && d < sea_steps[r * r_count + other_r] {
                        sea_steps[r * r_count + other_r] = d;
                    }
                }
                let next_d = d.saturating_add(1);
                for &nb_u16 in &tile_neighbors[cur] {
                    let nb = usize::from(nb_u16);
                    if tile_tags[nb].is_water() && dist[nb] == u16::MAX {
                        dist[nb] = next_d;
                        q.push_back(nb);
                    }
                }
            }
        }
        // Symmetrize `sea_steps`
        for a in 0..r_count {
            for b in (a + 1)..r_count {
                let m = sea_steps[a * r_count + b].min(sea_steps[b * r_count + a]);
                sea_steps[a * r_count + b] = m;
                sea_steps[b * r_count + a] = m;
            }
        }

        // 2B. Coast-segment partitioning (`coast_id: 0..coast_count`) + `coast_sea_steps` (`C × C` O(1) lookup):
        // Coastal land tiles in the same `region_id` share a `coast_id` iff their adjacent water tiles
        // connect within `<= 3` water hops (so East Coast vs West Coast across an isthmus or inland sea vs ocean
        // automatically split into distinct coast segments with zero runtime BFS!).
        let mut tile_coast_id = vec![u16::MAX; tile_count];
        let mut coast_waters: Vec<Vec<usize>> = Vec::new();
        for idx in 0..tile_count {
            let ra = usize::from(*tile_regions.get(idx).unwrap_or(&u16::MAX));
            if ra >= r_count
                || tile_tags[idx].is_water()
                || !tile_tags[idx].has(TileTag::Coastal)
                || tile_coast_id[idx] != u16::MAX
            {
                continue;
            }
            let cid = coast_waters.len() as u16;
            tile_coast_id[idx] = cid;
            let mut group_tiles = vec![idx];
            let mut gq = std::collections::VecDeque::new();
            gq.push_back(idx);

            while let Some(ct) = gq.pop_front() {
                // Find all same-region coastal land tiles reachable within `<= 3` local water steps from `ct`
                let mut wdist = vec![u8::MAX; tile_count];
                let mut wq = std::collections::VecDeque::new();
                for &nb_u16 in &tile_neighbors[ct] {
                    let nb = usize::from(nb_u16);
                    if tile_tags[nb].is_water() && wdist[nb] == u8::MAX {
                        wdist[nb] = 1;
                        wq.push_back(nb);
                    }
                    // Direct land-neighbor coastal tile touching the same water body
                    if !tile_tags[nb].is_water()
                        && tile_tags[nb].has(TileTag::Coastal)
                        && usize::from(*tile_regions.get(nb).unwrap_or(&u16::MAX)) == ra
                        && tile_coast_id[nb] == u16::MAX
                    {
                        let shares_water = tile_neighbors[ct].iter().any(|&w1| {
                            let u1 = usize::from(w1);
                            tile_tags[u1].is_water() && tile_neighbors[nb].contains(&w1)
                        });
                        if shares_water {
                            tile_coast_id[nb] = cid;
                            group_tiles.push(nb);
                            gq.push_back(nb);
                        }
                    }
                }
                while let Some(wcur) = wq.pop_front() {
                    let wd = wdist[wcur];
                    for &nb_u16 in &tile_neighbors[wcur] {
                        let nb = usize::from(nb_u16);
                        if !tile_tags[nb].is_water()
                            && tile_tags[nb].has(TileTag::Coastal)
                            && usize::from(*tile_regions.get(nb).unwrap_or(&u16::MAX)) == ra
                            && tile_coast_id[nb] == u16::MAX
                        {
                            tile_coast_id[nb] = cid;
                            group_tiles.push(nb);
                            gq.push_back(nb);
                        } else if tile_tags[nb].is_water() && wd < 3 && wdist[nb] == u8::MAX {
                            wdist[nb] = wd + 1;
                            wq.push_back(nb);
                        }
                    }
                }
            }

            let mut cw = Vec::new();
            for &gt in &group_tiles {
                for &nb_u16 in &tile_neighbors[gt] {
                    let nb = usize::from(nb_u16);
                    if tile_tags[nb].is_water() {
                        cw.push(nb);
                    }
                }
            }
            cw.sort_unstable();
            cw.dedup();
            coast_waters.push(cw);
        }

        let coast_count = coast_waters.len();
        let mut water_adj_coasts: Vec<Vec<usize>> = vec![Vec::new(); tile_count];
        for (c, waters) in coast_waters.iter().enumerate() {
            for &w in waters {
                water_adj_coasts[w].push(c);
            }
        }
        let mut coast_sea_steps = vec![Self::UNREACHABLE_STEPS; coast_count * coast_count];
        for c in 0..coast_count {
            coast_sea_steps[c * coast_count + c] = 0;
            if coast_waters[c].is_empty() {
                continue;
            }
            let mut dist = vec![u16::MAX; tile_count];
            let mut q = std::collections::VecDeque::new();
            for &w in &coast_waters[c] {
                dist[w] = 1;
                q.push_back(w);
            }
            while let Some(cur) = q.pop_front() {
                let d = dist[cur];
                for &oc in &water_adj_coasts[cur] {
                    if oc != c && d < coast_sea_steps[c * coast_count + oc] {
                        coast_sea_steps[c * coast_count + oc] = d;
                    }
                }
                let next_d = d.saturating_add(1);
                for &nb_u16 in &tile_neighbors[cur] {
                    let nb = usize::from(nb_u16);
                    if tile_tags[nb].is_water() && dist[nb] == u16::MAX {
                        dist[nb] = next_d;
                        q.push_back(nb);
                    }
                }
            }
        }
        for a in 0..coast_count {
            for b in (a + 1)..coast_count {
                let m = coast_sea_steps[a * coast_count + b]
                    .min(coast_sea_steps[b * coast_count + a]);
                coast_sea_steps[a * coast_count + b] = m;
                coast_sea_steps[b * coast_count + a] = m;
            }
        }

        // 3. Point-to-point suborbital `air_steps` (`1..=12` normalized by ~140 map units per step,
        //    using shortest cylindrical distance across the horizontal `0 <-> SURFACE_WIDTH` seam!)
        let mut air_steps = vec![0u16; r_count * r_count];
        for a in 0..r_count {
            for b in (a + 1)..r_count {
                let raw_dx = (regions[a].center[0] - regions[b].center[0]).abs();
                let dx = raw_dx.min(crate::surface::SURFACE_WIDTH - raw_dx);
                let dy = regions[a].center[1] - regions[b].center[1];
                let px_dist = (dx * dx + dy * dy).sqrt();
                let steps = ((px_dist / 140.0).ceil() as u16).clamp(1, 12);
                air_steps[a * r_count + b] = steps;
                air_steps[b * r_count + a] = steps;
            }
        }

        Self {
            region_count: r_count,
            land_terrain_mult,
            sea_steps,
            coast_count,
            tile_coast_id,
            coast_sea_steps,
            air_steps,
        }
    }

    #[inline]
    pub fn land_mult(&self, ra: u16, rb: u16) -> u8 {
        let (a, b) = (usize::from(ra), usize::from(rb));
        if a >= self.region_count || b >= self.region_count {
            return 0;
        }
        self.land_terrain_mult[a * self.region_count + b]
    }

    #[inline]
    pub fn sea_distance(&self, ra: u16, rb: u16) -> u16 {
        let (a, b) = (usize::from(ra), usize::from(rb));
        if a >= self.region_count || b >= self.region_count {
            return Self::UNREACHABLE_STEPS;
        }
        self.sea_steps[a * self.region_count + b]
    }

    #[inline]
    pub fn coast_sea_distance(&self, ca: u16, cb: u16) -> u16 {
        let (a, b) = (usize::from(ca), usize::from(cb));
        if a >= self.coast_count || b >= self.coast_count {
            return Self::UNREACHABLE_STEPS;
        }
        self.coast_sea_steps[a * self.coast_count + b]
    }

    #[inline]
    pub fn air_distance(&self, ra: u16, rb: u16) -> u16 {
        let (a, b) = (usize::from(ra), usize::from(rb));
        if a >= self.region_count || b >= self.region_count {
            return Self::UNREACHABLE_STEPS;
        }
        self.air_steps[a * self.region_count + b]
    }
}

pub fn derive_sector_trade_links(
    tile_sectors: &[u16],
    tile_tags: &[TileTags],
    tile_neighbors: &[Vec<u16>],
    sector_count: usize,
) -> Vec<SectorTradeLink> {
    if sector_count <= 1 {
        return Vec::new();
    }
    let mut pair_acc: BTreeMap<(u16, u16), (i64, i64, i64, i64)> = BTreeMap::new();

    for (idx, nbs) in tile_neighbors.iter().enumerate() {
        let sa = tile_sectors[idx];
        if sa == u16::MAX {
            continue;
        }
        let ta = tile_tags[idx];
        for &nb_u16 in nbs {
            let nb = usize::from(nb_u16);
            if nb <= idx {
                continue;
            }
            let sb = tile_sectors[nb];
            if sb == u16::MAX || sa == sb {
                continue;
            }
            let tb = tile_tags[nb];
            let key = if sa < sb { (sa, sb) } else { (sb, sa) };
            let entry = pair_acc.entry(key).or_insert((0, 0, 0, 0));

            if !ta.is_water() && !tb.is_water() {
                let (edge_cap, edge_fee) = if ta.has(TileTag::Mountains) || tb.has(TileTag::Mountains)
                {
                    (6, 4)
                } else if ta.has(TileTag::Arid)
                    || tb.has(TileTag::Arid)
                    || ta.has(TileTag::Tundra)
                    || tb.has(TileTag::Tundra)
                {
                    (10, 3)
                } else if ta.has(TileTag::Hills)
                    || tb.has(TileTag::Hills)
                    || ta.has(TileTag::Forest)
                    || tb.has(TileTag::Forest)
                {
                    (16, 2)
                } else {
                    (26, 1)
                };
                entry.0 += edge_cap;
                entry.1 += edge_fee;
                entry.2 += 1;
            } else {
                entry.3 += 1;
            }
        }
    }

    let mut links = Vec::with_capacity(pair_acc.len());
    for ((a, b), (land_cap, fee_sum, land_edges, sea_edges)) in pair_acc {
        if land_edges > 0 {
            let avg_fee = ((fee_sum + land_edges / 2) / land_edges).clamp(1, 5);
            let maritime_bonus = if sea_edges > 0 { 16 } else { 0 };
            links.push(SectorTradeLink {
                sector_a: SectorId(a),
                sector_b: SectorId(b),
                monthly_capacity: (land_cap + maritime_bonus).min(140),
                transport_fee: avg_fee,
                is_maritime: false,
            });
        } else if sea_edges > 0 {
            links.push(SectorTradeLink {
                sector_a: SectorId(a),
                sector_b: SectorId(b),
                monthly_capacity: (sea_edges * 10).clamp(16, 64),
                transport_fee: 2,
                is_maritime: true,
            });
        }
    }
    links
}
