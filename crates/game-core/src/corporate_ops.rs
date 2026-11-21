//! Corporations, Chartered Trusts, Securities & Bilateral Contracts (`企业、托拉斯、金融证券与双边契约层`).

use crate::economy::{
    BilateralSupplyContract, BuildingOwner, CounterpartyId, Polity, Sector, SectorId,
    SectorMarket, SectorPops, Trust, TrustId, CAMPUS_HOSTING_CAPACITY,
};
use crate::market::{GoodDef, GoodId, Inventory, MarketClearingRule, TransportChannel};
use crate::planet::{CashSink, Planet, PlanetTiles};
use crate::rules::{self, PendingEventPayload, RuleContext};
use crate::Game;

impl Game {
    /// Channel-3 point-to-point bilateral supply agreement between two named entities (`Polity`,
    /// `Enterprise`, `Trust`). Strictly rejects `LocalPrivate` / `SectorCapital`!
    #[allow(clippy::too_many_arguments)]
    pub fn sign_bilateral_supply_contract(
        &mut self,
        planet_id: &str,
        seller_owner: BuildingOwner,
        buyer_owner: BuildingOwner,
        sector_id: SectorId,
        good: GoodId,
        monthly_qty: i64,
        unit_price_cents: i64,
        months: u16,
    ) -> Result<u32, String> {
        let seller = CounterpartyId::try_from_owner(seller_owner)?;
        let buyer = CounterpartyId::try_from_owner(buyer_owner)?;
        if seller == buyer {
            return Err("双边契约的买卖双方不能是同一个主体。".into());
        }
        if monthly_qty <= 0 || unit_price_cents < 0 || months == 0 {
            return Err("双边契约的交货量与期限必须大于 0。".into());
        }
        if usize::from(good.0) >= self.goods.len() || self.goods.get(good).is_currency {
            return Err("双边供货契约标的必须是有效的非货币商品。".into());
        }
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                let cid = pt.next_contract_id.max(1);
                pt.next_contract_id = cid + 1;
                pt.bilateral_contracts.push(BilateralSupplyContract {
                    id: cid,
                    seller,
                    buyer,
                    sector_id,
                    good,
                    monthly_qty,
                    unit_price_cents,
                    remaining_months: months,
                    seller_honors: true,
                    buyer_honors: true,
                    default_count: 0,
                });
                return Ok(cid);
            }
        }
        Err(format!("找不到星球：{planet_id}"))
    }

    /// Channel-3 bilateral honoring / deliberate breach switch (only callable by seller or buyer).
    pub fn set_bilateral_contract_honoring(
        &mut self,
        planet_id: &str,
        actor_owner: BuildingOwner,
        contract_id: u32,
        honor: bool,
    ) -> Result<(), String> {
        let actor = CounterpartyId::try_from_owner(actor_owner)?;
        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                let Some(bc) = pt
                    .bilateral_contracts
                    .iter_mut()
                    .find(|c| c.id == contract_id)
                else {
                    return Err(format!("找不到双边契约 #{contract_id}"));
                };
                if bc.seller == actor {
                    bc.seller_honors = honor;
                    return Ok(());
                } else if bc.buyer == actor {
                    bc.buyer_honors = honor;
                    return Ok(());
                } else {
                    return Err("仅双边契约当事方可修改履约指令。".into());
                }
            }
        }
        Err(format!("找不到星球：{planet_id}"))
    }

    /// Charters a named `Trust` (`星际托拉斯`), registers its standardized `Telecom` equity good
    /// in `GoodCatalog` (automatically spawning `SectorOrderBook`s across every `SectorMarket`!),
    /// transfers `initial_cash_cents` from the named founder's vault into `Trust.vault` (exact cash
    /// conservation!), and mints `total_shares` into the founder's vault.
    #[allow(clippy::too_many_arguments)]
    pub fn charter_trust(
        &mut self,
        planet_id: &str,
        name: String,
        industry_tag: String,
        share_good_id: String,
        share_good_name: String,
        total_shares: i64,
        genesis_share_price_cents: i64,
        founder_owner: BuildingOwner,
        initial_cash_cents: i64,
    ) -> Result<(TrustId, GoodId), String> {
        let founder = CounterpartyId::try_from_owner(founder_owner)?;
        if total_shares <= 0 || initial_cash_cents < 0 {
            return Err("托拉斯总股本必须大于 0，初始注资不能为负。".into());
        }
        let founder_sink = match founder {
            CounterpartyId::Polity { polity_id } => {
                if self.polity_cash_cents(polity_id) < initial_cash_cents {
                    return Err("发起政权国库现金不足以完成托拉斯注资。".into());
                }
                CashSink::Polity(polity_id)
            }
            CounterpartyId::Enterprise { enterprise_id } => {
                if self.enterprise_cash_cents(planet_id, enterprise_id) < initial_cash_cents {
                    return Err("发起企业金库现金不足以完成托拉斯注资。".into());
                }
                CashSink::Enterprise(enterprise_id)
            }
            CounterpartyId::Trust { trust_id } => {
                if self.trust_cash_cents(planet_id, trust_id) < initial_cash_cents {
                    return Err("发起托拉斯金库现金不足以完成子公司注资。".into());
                }
                CashSink::Trust(trust_id)
            }
        };

        let next_trust_idx = self
            .world
            .query::<(&Planet, &PlanetTiles)>()
            .iter()
            .find_map(|(_, (p, pt))| (p.id == planet_id).then_some(pt.trusts.len()))
            .ok_or_else(|| format!("找不到星球：{planet_id}"))?;

        let eq_gid = self.goods.register_good(GoodDef {
            id: share_good_id,
            name: share_good_name,
            is_currency: false,
            non_market: false,
            unit_volume: 0,
            retention_permille: 1000,
            transport_channel: TransportChannel::Telecom,
            genesis_price_cents: genesis_share_price_cents.max(100),
            staple_demand_per_100: 0,
            service_demand_per_100: 0,
            excess_satisfaction_bonus: 0,
            capital_pool_cap_cents_per_unit: 0,
            strategic_depot_target: 0,
            coupon_permille_per_month: 0,
            issuer_key: Some(format!("trust:{next_trust_idx}")),
            market_rule: MarketClearingRule::spot_security(),
        })?;

        // Automatically open order books for the new security in every `SectorMarket`!
        for (_, mkt) in self.world.query_mut::<&mut SectorMarket>() {
            mkt.ensure_good_books(&self.goods);
        }

        // Deduct founder cash and deliver `total_shares` into founder's vault, then create `Trust.vault`.
        self.adjust_cash(planet_id, founder_sink, -initial_cash_cents);
        let eq_idx = usize::from(eq_gid.0);
        match founder {
            CounterpartyId::Polity { polity_id } => {
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == polity_id {
                        pol.vault.ensure_len(self.goods.len());
                        pol.vault.available[eq_idx] += total_shares;
                        break;
                    }
                }
            }
            CounterpartyId::Enterprise { enterprise_id } => {
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id == planet_id {
                        if let Some(ent) = pt.enterprises.get_mut(usize::from(enterprise_id.0)) {
                            ent.vault.ensure_len(self.goods.len());
                            ent.vault.available[eq_idx] += total_shares;
                        }
                        break;
                    }
                }
            }
            CounterpartyId::Trust { trust_id } => {
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id == planet_id {
                        if let Some(tr) = pt.trusts.get_mut(usize::from(trust_id.0)) {
                            tr.vault.ensure_len(self.goods.len());
                            tr.vault.available[eq_idx] += total_shares;
                        }
                        break;
                    }
                }
            }
        }

        for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
            if planet.id == planet_id {
                let tid = TrustId(pt.trusts.len() as u16);
                pt.trusts.push(Trust::new(
                    &self.goods,
                    tid,
                    name,
                    industry_tag,
                    Some(eq_gid),
                    total_shares,
                    CAMPUS_HOSTING_CAPACITY * 2,
                    initial_cash_cents,
                ));
                return Ok((tid, eq_gid));
            }
        }
        Err(format!("找不到星球：{planet_id}"))
    }

    /// Registers a standardized tradable debt/claim security in `GoodCatalog` (`Telecom`, weightless,
    /// `MarketClearingRule::spot_security()`), spawns order books across every `SectorMarket`, and
    /// mints `units_issued` into `issuer_owner`'s own vault (zero cash created out of thin air —
    /// cash only moves when buyers purchase the claim on the open order book; monthly settlement
    /// invokes `EVENT_COUPON_CLAIM_DUE` on the issuer!).
    #[allow(clippy::too_many_arguments)]
    pub fn issue_standardized_claim(
        &mut self,
        planet_id: &str,
        issuer_owner: BuildingOwner,
        claim_good_id: String,
        claim_name: String,
        face_price_cents: i64,
        coupon_permille_per_month: i64,
        units_issued: i64,
    ) -> Result<GoodId, String> {
        let issuer = CounterpartyId::try_from_owner(issuer_owner)?;
        if units_issued <= 0 || face_price_cents <= 0 || coupon_permille_per_month < 0 {
            return Err("债权证券的面值与发行份额必须大于 0。".into());
        }
        let claim_gid = self.goods.register_good(GoodDef {
            id: claim_good_id,
            name: claim_name,
            is_currency: false,
            non_market: false,
            unit_volume: 0,
            retention_permille: 1000,
            transport_channel: TransportChannel::Telecom,
            genesis_price_cents: face_price_cents.max(100),
            staple_demand_per_100: 0,
            service_demand_per_100: 0,
            excess_satisfaction_bonus: 0,
            capital_pool_cap_cents_per_unit: 0,
            strategic_depot_target: 0,
            coupon_permille_per_month,
            issuer_key: Some(issuer.key_string()),
            market_rule: MarketClearingRule::spot_security(),
        })?;
        for (_, mkt) in self.world.query_mut::<&mut SectorMarket>() {
            mkt.ensure_good_books(&self.goods);
        }
        let c_idx = usize::from(claim_gid.0);
        match issuer {
            CounterpartyId::Polity { polity_id } => {
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == polity_id {
                        pol.vault.ensure_len(self.goods.len());
                        pol.vault.available[c_idx] += units_issued;
                        return Ok(claim_gid);
                    }
                }
            }
            CounterpartyId::Enterprise { enterprise_id } => {
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id == planet_id {
                        if let Some(ent) = pt.enterprises.get_mut(usize::from(enterprise_id.0)) {
                            ent.vault.ensure_len(self.goods.len());
                            ent.vault.available[c_idx] += units_issued;
                            return Ok(claim_gid);
                        }
                    }
                }
            }
            CounterpartyId::Trust { trust_id } => {
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id == planet_id {
                        if let Some(tr) = pt.trusts.get_mut(usize::from(trust_id.0)) {
                            tr.vault.ensure_len(self.goods.len());
                            tr.vault.available[c_idx] += units_issued;
                            return Ok(claim_gid);
                        }
                    }
                }
            }
        }
        Err("找不到债权证券发行主体金库。".into())
    }

    /// Resolves a player-queued `PendingEvent` by executing the chosen `EventOptionDef`'s declarative
    /// `RuleEffect`s against real `Inventory` containers with strict cash & good conservation.
    pub fn resolve_pending_event(
        &mut self,
        instance_id: u64,
        option_index: usize,
    ) -> Result<(), String> {
        let pos = self
            .pending_events
            .iter()
            .position(|pe| pe.instance_id == instance_id)
            .ok_or_else(|| format!("找不到待处理的决策事件 #{instance_id}"))?;
        let pending = self.pending_events.remove(pos);
        let ev_def = self
            .events
            .get(&pending.event_id)
            .cloned()
            .ok_or_else(|| format!("未知的事件定义：{}", pending.event_id))?;
        if option_index >= ev_def.options.len() {
            return Err(format!("事件选项序号 {option_index} 超出范围。"));
        }

        let goods = &self.goods;
        let cash = goods.cash_id();

        match pending.payload {
            PendingEventPayload::BilateralContract {
                planet_id,
                contract_id,
                seller,
                buyer,
                sector_id,
                good,
                qty,
                unit_price_cents,
            } => {
                let ruling = self
                    .world
                    .query::<&Planet>()
                    .iter()
                    .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
                    .ok_or_else(|| format!("找不到星球：{planet_id}"))?;

                let mut invs: Vec<Inventory> = Vec::new();
                let mut pol_rating = 100i16;
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == ruling {
                        pol_rating = pol.credit_rating;
                        invs.push(std::mem::take(&mut pol.vault));
                        break;
                    }
                }
                if invs.is_empty() {
                    return Err("找不到星球执政政权金库。".into());
                }
                let mut depot_inv_opt = None;
                for (_, sec) in self.world.query_mut::<&mut Sector>() {
                    if sec.planet_id == planet_id && sec.id == sector_id {
                        depot_inv_opt = Some(invs.len());
                        invs.push(std::mem::take(&mut sec.state_depot));
                        break;
                    }
                }

                let mut pol_delta = 0i16;
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id != planet_id {
                        continue;
                    }
                    let mut ent_invs = Vec::with_capacity(pt.enterprises.len());
                    for ent in &mut pt.enterprises {
                        ent_invs.push(invs.len());
                        invs.push(std::mem::take(&mut ent.vault));
                    }
                    let mut tr_invs = Vec::with_capacity(pt.trusts.len());
                    for tr in &mut pt.trusts {
                        tr_invs.push(invs.len());
                        invs.push(std::mem::take(&mut tr.vault));
                    }
                    for inv in invs.iter_mut() {
                        inv.ensure_len(goods.len());
                    }

                    let resolve_cp = |cp: CounterpartyId| -> Option<(usize, usize)> {
                        let weightless = goods.get(good).unit_volume == 0;
                        match cp {
                            CounterpartyId::Polity { polity_id } if polity_id == ruling => {
                                let g = if weightless {
                                    0
                                } else {
                                    depot_inv_opt.unwrap_or(0)
                                };
                                Some((g, 0))
                            }
                            CounterpartyId::Enterprise { enterprise_id } => ent_invs
                                .get(usize::from(enterprise_id.0))
                                .copied()
                                .map(|i| (i, i)),
                            CounterpartyId::Trust { trust_id } => tr_invs
                                .get(usize::from(trust_id.0))
                                .copied()
                                .map(|i| (i, i)),
                            _ => None,
                        }
                    };
                    let get_rating = |pt_r: &PlanetTiles, cp: CounterpartyId| -> i16 {
                        match cp {
                            CounterpartyId::Polity { polity_id } if polity_id == ruling => {
                                pol_rating
                            }
                            CounterpartyId::Enterprise { enterprise_id } => pt_r
                                .enterprises
                                .get(usize::from(enterprise_id.0))
                                .map(|e| e.credit_rating)
                                .unwrap_or(100),
                            CounterpartyId::Trust { trust_id } => pt_r
                                .trusts
                                .get(usize::from(trust_id.0))
                                .map(|t| t.credit_rating)
                                .unwrap_or(100),
                            _ => 100,
                        }
                    };

                    if let (Some((sg, sc)), Some((bg, bc))) = (resolve_cp(seller), resolve_cp(buyer))
                    {
                        let ctx = RuleContext {
                            seller_honors: true,
                            buyer_honors: true,
                            seller_cash_cents: invs[sc].avail(cash),
                            buyer_cash_cents: invs[bc].avail(cash),
                            seller_good_avail: invs[sg].avail(good),
                            buyer_good_fit: invs[bg].max_receivable_units(goods, good),
                            seller_credit_rating: get_rating(pt, seller),
                            buyer_credit_rating: get_rating(pt, buyer),
                            contract_qty: qty,
                            unit_price_cents,
                            ..RuleContext::default()
                        };
                        let mut report = rules::EffectExecutionReport::default();
                        rules::execute_rule_effects(
                            &ev_def.options[option_index].effects,
                            &ctx,
                            goods,
                            &mut invs,
                            sg,
                            sc,
                            bg,
                            bc,
                            Some(good),
                            &[],
                            0,
                            &mut report,
                        );
                        for (cp, delta) in [
                            (seller, report.seller_rating_delta),
                            (buyer, report.buyer_rating_delta),
                        ] {
                            match cp {
                                CounterpartyId::Polity { polity_id } if polity_id == ruling => {
                                    pol_delta = pol_delta.saturating_add(delta);
                                }
                                CounterpartyId::Enterprise { enterprise_id } => {
                                    if let Some(e) =
                                        pt.enterprises.get_mut(usize::from(enterprise_id.0))
                                    {
                                        e.credit_rating = (e.credit_rating + delta).clamp(0, 100);
                                    }
                                }
                                CounterpartyId::Trust { trust_id } => {
                                    if let Some(t) = pt.trusts.get_mut(usize::from(trust_id.0)) {
                                        t.credit_rating = (t.credit_rating + delta).clamp(0, 100);
                                    }
                                }
                                _ => {}
                            }
                        }
                        if let Some(c_mut) = pt
                            .bilateral_contracts
                            .iter_mut()
                            .find(|c| c.id == contract_id)
                        {
                            c_mut.default_count =
                                c_mut.default_count.saturating_add(report.default_increment);
                            if report.contract_cancelled {
                                c_mut.remaining_months = 0;
                            } else {
                                c_mut.remaining_months = c_mut.remaining_months.saturating_sub(1);
                            }
                        }
                        pt.bilateral_contracts.retain(|c| c.remaining_months > 0);
                    }

                    for (i, ent) in pt.enterprises.iter_mut().enumerate() {
                        ent.vault = std::mem::take(&mut invs[ent_invs[i]]);
                    }
                    for (i, tr) in pt.trusts.iter_mut().enumerate() {
                        tr.vault = std::mem::take(&mut invs[tr_invs[i]]);
                    }
                    break;
                }
                if let Some(d_i) = depot_inv_opt {
                    for (_, sec) in self.world.query_mut::<&mut Sector>() {
                        if sec.planet_id == planet_id && sec.id == sector_id {
                            sec.state_depot = std::mem::take(&mut invs[d_i]);
                            break;
                        }
                    }
                }
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == ruling {
                        pol.vault = std::mem::take(&mut invs[0]);
                        pol.credit_rating = (pol.credit_rating + pol_delta).clamp(0, 100);
                        break;
                    }
                }
                Ok(())
            }
            PendingEventPayload::CouponClaim {
                planet_id,
                issuer,
                claim_good,
                coupon_cents_per_unit,
            } => {
                let ruling = self
                    .world
                    .query::<&Planet>()
                    .iter()
                    .find_map(|(_, p)| (p.id == planet_id).then_some(p.ruling_polity))
                    .ok_or_else(|| format!("找不到星球：{planet_id}"))?;
                let mut invs: Vec<Inventory> = Vec::new();
                let mut pol_rating = 100i16;
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == ruling {
                        pol_rating = pol.credit_rating;
                        invs.push(std::mem::take(&mut pol.vault));
                        break;
                    }
                }
                if invs.is_empty() {
                    return Err("找不到星球执政政权金库。".into());
                }
                let mut sec_pop_indices: Vec<(SectorId, Vec<usize>)> = Vec::new();
                for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                    if sec.planet_id == planet_id {
                        let mut p_invs = Vec::with_capacity(pops.groups.len());
                        for g in &mut pops.groups {
                            p_invs.push(invs.len());
                            invs.push(std::mem::take(&mut g.inv));
                        }
                        sec_pop_indices.push((sec.id, p_invs));
                    }
                }
                let mut pol_delta = 0i16;
                for (_, (planet, pt)) in self.world.query_mut::<(&Planet, &mut PlanetTiles)>() {
                    if planet.id != planet_id {
                        continue;
                    }
                    let mut ent_invs = Vec::with_capacity(pt.enterprises.len());
                    for ent in &mut pt.enterprises {
                        ent_invs.push(invs.len());
                        invs.push(std::mem::take(&mut ent.vault));
                    }
                    let mut tr_invs = Vec::with_capacity(pt.trusts.len());
                    for tr in &mut pt.trusts {
                        tr_invs.push(invs.len());
                        invs.push(std::mem::take(&mut tr.vault));
                    }
                    for inv in invs.iter_mut() {
                        inv.ensure_len(goods.len());
                    }
                    let issuer_inv = match issuer {
                        CounterpartyId::Polity { polity_id } if polity_id == ruling => Some(0),
                        CounterpartyId::Enterprise { enterprise_id } => {
                            ent_invs.get(usize::from(enterprise_id.0)).copied()
                        }
                        CounterpartyId::Trust { trust_id } => {
                            tr_invs.get(usize::from(trust_id.0)).copied()
                        }
                        _ => None,
                    };
                    if let Some(iss_i) = issuer_inv {
                        let holders: Vec<(usize, i64)> = invs
                            .iter()
                            .enumerate()
                            .filter_map(|(i, inv)| {
                                (i != iss_i && inv.total(claim_good) > 0)
                                    .then_some((i, inv.total(claim_good)))
                            })
                            .collect();
                        let total_held: i64 = holders.iter().map(|&(_, h)| h).sum();
                        let ctx = RuleContext {
                            seller_honors: true,
                            buyer_honors: true,
                            seller_cash_cents: invs[iss_i].avail(cash),
                            seller_credit_rating: pol_rating,
                            contract_qty: total_held,
                            unit_price_cents: coupon_cents_per_unit,
                            ..RuleContext::default()
                        };
                        let mut report = rules::EffectExecutionReport::default();
                        rules::execute_rule_effects(
                            &ev_def.options[option_index].effects,
                            &ctx,
                            goods,
                            &mut invs,
                            iss_i,
                            iss_i,
                            iss_i,
                            iss_i,
                            Some(claim_good),
                            &holders,
                            coupon_cents_per_unit,
                            &mut report,
                        );
                        match issuer {
                            CounterpartyId::Polity { polity_id } if polity_id == ruling => {
                                pol_delta = pol_delta.saturating_add(report.seller_rating_delta);
                            }
                            CounterpartyId::Enterprise { enterprise_id } => {
                                if let Some(e) =
                                    pt.enterprises.get_mut(usize::from(enterprise_id.0))
                                {
                                    e.credit_rating = (e.credit_rating
                                        + report.seller_rating_delta)
                                        .clamp(0, 100);
                                }
                            }
                            CounterpartyId::Trust { trust_id } => {
                                if let Some(t) = pt.trusts.get_mut(usize::from(trust_id.0)) {
                                    t.credit_rating = (t.credit_rating
                                        + report.seller_rating_delta)
                                        .clamp(0, 100);
                                }
                            }
                            _ => {}
                        }
                    }
                    for (i, ent) in pt.enterprises.iter_mut().enumerate() {
                        ent.vault = std::mem::take(&mut invs[ent_invs[i]]);
                    }
                    for (i, tr) in pt.trusts.iter_mut().enumerate() {
                        tr.vault = std::mem::take(&mut invs[tr_invs[i]]);
                    }
                    break;
                }
                for (sid, p_invs) in sec_pop_indices {
                    for (_, (sec, pops)) in self.world.query_mut::<(&Sector, &mut SectorPops)>() {
                        if sec.planet_id == planet_id && sec.id == sid {
                            for (gi, inv_idx) in p_invs.into_iter().enumerate() {
                                pops.groups[gi].inv = std::mem::take(&mut invs[inv_idx]);
                            }
                            break;
                        }
                    }
                }
                for (_, pol) in self.world.query_mut::<&mut Polity>() {
                    if pol.id == ruling {
                        pol.vault = std::mem::take(&mut invs[0]);
                        pol.credit_rating = (pol.credit_rating + pol_delta).clamp(0, 100);
                        break;
                    }
                }
                Ok(())
            }
            PendingEventPayload::Custom { .. } => Ok(()),
            PendingEventPayload::BankruptSlot {
                planet_id,
                tile_index,
                slot,
                admin_polity,
                ..
            } => {
                let mut report = rules::EffectExecutionReport::default();
                let mut dummy_invs = vec![Inventory::new(self.goods.len(), 0)];
                rules::execute_rule_effects(
                    &ev_def.options[option_index].effects,
                    &RuleContext::default(),
                    &self.goods,
                    &mut dummy_invs,
                    0,
                    0,
                    0,
                    0,
                    None,
                    &[],
                    0,
                    &mut report,
                );
                if report.bankruptcy_acquisition_approved {
                    self.acquire_bankrupt_slot_as(admin_polity, &planet_id, tile_index, slot)
                        .map(|_| ())
                } else {
                    self.demolish_bankrupt_slot(&planet_id, tile_index, slot)
                }
            }
        }
    }
}
