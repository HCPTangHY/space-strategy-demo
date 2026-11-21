//! Declarative Event & `if-else +-` Decision Rule Mechanism (`事件与脉冲规则引擎`).
//!
//! Pure data-serializable condition trees (`RuleCond`), Paradox-style `if-else +-` AI weight
//! modifiers (`IfElseWeightRule`, `AiWeightSpec`), conservation-safe effect primitives (`RuleEffect`),
//! and `EventCatalog` / `PendingEvent` execution for bilateral contracts, debt coupons, and pulses.

use crate::economy::{CounterpartyId, SectorId};
use crate::market::{GoodCatalog, GoodId, Inventory};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const EVENT_BILATERAL_CONTRACT_DUE: &str = "base:bilateral_contract_due";
pub const EVENT_COUPON_CLAIM_DUE: &str = "base:coupon_claim_due";
pub const EVENT_SECTOR_INVEST_COMMERCIAL: &str = "base:sector_invest_commercial";
pub const EVENT_FORCED_EXPROPRIATION: &str = "base:forced_expropriation";
pub const EVENT_PRIVATE_BANKRUPTCY: &str = "base:private_bankruptcy";

/// Observable numeric context variables evaluated by declarative `RuleCond` expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextVar {
    /// `1` if primary actor/seller honors the obligation, `0` if deliberate breach was ordered.
    SellerHonorsFlag,
    /// `1` if counterparty/buyer honors the obligation, `0` if deliberate breach was ordered.
    BuyerHonorsFlag,
    SellerCashCents,
    BuyerCashCents,
    SellerGoodAvail,
    BuyerGoodFit,
    SellerCreditRating,
    BuyerCreditRating,
    ContractQty,
    ContractDeliverCap,
    ContractUnitPriceCents,
    ContractFullBillCents,
    AffordableQty,
    InventoryFillPermille,
    MarginPermille,
    StaffingPermille,
    PoolCashCents,
    PoolCapCents,
    BuildCostCents,
    ServicePricePermille,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl CmpOp {
    #[inline]
    pub const fn eval(self, lhs: i64, rhs: i64) -> bool {
        match self {
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Eq => lhs == rhs,
            Self::Ne => lhs != rhs,
        }
    }
}

/// Declarative boolean condition tree (`if-else` predicate).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleCond {
    #[default]
    Always,
    Compare {
        var: ContextVar,
        op: CmpOp,
        value: i64,
    },
    CompareVars {
        lhs: ContextVar,
        op: CmpOp,
        rhs: ContextVar,
    },
    All {
        conds: Vec<RuleCond>,
    },
    Any {
        conds: Vec<RuleCond>,
    },
    Not {
        cond: Box<RuleCond>,
    },
}

/// Runtime numeric snapshot fed into `RuleCond` and `AiWeightSpec`.
#[derive(Clone, Debug, Default)]
pub struct RuleContext {
    pub seller_honors: bool,
    pub buyer_honors: bool,
    pub seller_cash_cents: i64,
    pub buyer_cash_cents: i64,
    pub seller_good_avail: i64,
    pub buyer_good_fit: i64,
    pub seller_credit_rating: i16,
    pub buyer_credit_rating: i16,
    pub contract_qty: i64,
    pub unit_price_cents: i64,
    pub inventory_fill_permille: i64,
    pub margin_permille: i64,
    pub staffing_permille: i64,
    pub pool_cash_cents: i64,
    pub pool_cap_cents: i64,
    pub build_cost_cents: i64,
    pub service_price_permille: i64,
}

impl RuleContext {
    #[inline]
    pub fn deliver_cap(&self) -> i64 {
        self.contract_qty.min(self.buyer_good_fit).max(0)
    }

    #[inline]
    pub fn deliverable_qty(&self) -> i64 {
        self.seller_good_avail.min(self.deliver_cap()).max(0)
    }

    #[inline]
    pub fn full_bill_cents(&self) -> i64 {
        self.deliver_cap() * self.unit_price_cents.max(0)
    }

    #[inline]
    pub fn affordable_qty(&self) -> i64 {
        let qty = self.deliverable_qty();
        if self.unit_price_cents > 0 {
            (self.buyer_cash_cents.max(0) / self.unit_price_cents)
                .min(qty)
                .max(0)
        } else {
            qty
        }
    }

    pub fn get_var(&self, var: ContextVar) -> i64 {
        match var {
            ContextVar::SellerHonorsFlag => i64::from(self.seller_honors),
            ContextVar::BuyerHonorsFlag => i64::from(self.buyer_honors),
            ContextVar::SellerCashCents => self.seller_cash_cents,
            ContextVar::BuyerCashCents => self.buyer_cash_cents,
            ContextVar::SellerGoodAvail => self.seller_good_avail,
            ContextVar::BuyerGoodFit => self.buyer_good_fit,
            ContextVar::SellerCreditRating => i64::from(self.seller_credit_rating),
            ContextVar::BuyerCreditRating => i64::from(self.buyer_credit_rating),
            ContextVar::ContractQty => self.contract_qty,
            ContextVar::ContractDeliverCap => self.deliver_cap(),
            ContextVar::ContractUnitPriceCents => self.unit_price_cents,
            ContextVar::ContractFullBillCents => self.full_bill_cents(),
            ContextVar::AffordableQty => self.affordable_qty(),
            ContextVar::InventoryFillPermille => self.inventory_fill_permille,
            ContextVar::MarginPermille => self.margin_permille,
            ContextVar::StaffingPermille => self.staffing_permille,
            ContextVar::PoolCashCents => self.pool_cash_cents,
            ContextVar::PoolCapCents => self.pool_cap_cents,
            ContextVar::BuildCostCents => self.build_cost_cents,
            ContextVar::ServicePricePermille => self.service_price_permille,
        }
    }
}

impl RuleCond {
    pub fn eval(&self, ctx: &RuleContext) -> bool {
        match self {
            Self::Always => true,
            Self::Compare { var, op, value } => op.eval(ctx.get_var(*var), *value),
            Self::CompareVars { lhs, op, rhs } => op.eval(ctx.get_var(*lhs), ctx.get_var(*rhs)),
            Self::All { conds } => conds.iter().all(|c| c.eval(ctx)),
            Self::Any { conds } => conds.iter().any(|c| c.eval(ctx)),
            Self::Not { cond } => !cond.eval(ctx),
        }
    }
}

/// Additive or multiplicative weight adjustment in an `if-else +-` AI rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "val", rename_all = "snake_case")]
pub enum WeightDelta {
    Add(i64),
    MulPermille(i64),
}

impl WeightDelta {
    #[inline]
    pub const fn apply(self, cur: i64) -> i64 {
        match self {
            Self::Add(d) => cur.saturating_add(d),
            Self::MulPermille(p) => ((cur as i128 * p as i128) / 1000) as i64,
        }
    }
}

/// Single `if cond { +- } else { +- }` branch inside an `AiWeightSpec`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IfElseWeightRule {
    pub cond: RuleCond,
    pub then_delta: WeightDelta,
    #[serde(default)]
    pub else_delta: Option<WeightDelta>,
}

/// Paradox-style declarative `if-else +-` AI scoring table (`base` + ordered conditional modifiers).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiWeightSpec {
    pub base: i64,
    #[serde(default)]
    pub rules: Vec<IfElseWeightRule>,
}

impl AiWeightSpec {
    pub fn constant(base: i64) -> Self {
        Self {
            base,
            rules: Vec::new(),
        }
    }

    pub fn score(&self, ctx: &RuleContext) -> i64 {
        let mut w = self.base;
        for r in &self.rules {
            if r.cond.eval(ctx) {
                w = r.then_delta.apply(w);
            } else if let Some(ed) = r.else_delta {
                w = ed.apply(w);
            }
        }
        w
    }
}

/// Identifies which party to a bilateral event an effect targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventParty {
    Seller,
    Buyer,
}

/// Conservation-preserving declarative effects executed when an event option is chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleEffect {
    /// Atomically delivers up to `AffordableQty` of `good` from seller to buyer and transfers
    /// `AffordableQty * unit_price_cents` of cash from buyer to seller (`Inventory` <-> `Inventory`).
    ExecuteContractDelivery,
    /// Atomically distributes coupon cash from `Seller` (issuer) across holder inventories pro-rata.
    ExecuteCouponPayout,
    /// Transfers up to `amount_cents` of cash between `from` and `to` inventories.
    TransferCash {
        from: EventParty,
        to: EventParty,
        amount_cents: i64,
    },
    /// Adjusts a named entity's `credit_rating` (`Polity` / `Enterprise` / `Trust`).
    AdjustCreditRating {
        party: EventParty,
        delta: i16,
    },
    /// Increments `default_count` on the active bilateral contract.
    IncrementContractDefault,
    /// Cancels active bilateral contract immediately (`remaining_months = 0`).
    CancelContract,
    /// Authorizes autonomous `SectorCapital` construction of a commercial/service facility in the evaluated sector.
    ApproveSectorInvestment,
    /// Authorizes host/local `Polity` acquisition of a bankrupt `LocalPrivate` workshop at salvage value.
    ApproveBankruptcyAcquisition,
    /// Conditional `if-else` effect block (allows fine-grained shortfall vs full-honor consequences).
    If {
        cond: RuleCond,
        then_effects: Vec<RuleEffect>,
        #[serde(default)]
        else_effects: Vec<RuleEffect>,
    },
    /// Queues a follow-up decision event for `target` party.
    QueueFollowupEvent {
        event_id: String,
        target: EventParty,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventOptionDef {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub trigger: RuleCond,
    pub ai_weight: AiWeightSpec,
    pub effects: Vec<RuleEffect>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDef {
    pub id: String,
    pub title: String,
    pub options: Vec<EventOptionDef>,
}

impl EventDef {
    /// Evaluates `trigger` and `ai_weight` (`if-else +-`) across all options, returning the winning index.
    pub fn choose_ai_option(&self, ctx: &RuleContext) -> Option<usize> {
        let mut best: Option<(usize, i64)> = None;
        for (idx, opt) in self.options.iter().enumerate() {
            if !opt.trigger.eval(ctx) {
                continue;
            }
            let s = opt.ai_weight.score(ctx);
            if best.map_or(true, |(_, bs)| s > bs) {
                best = Some((idx, s));
            }
        }
        best.map(|(i, _)| i)
    }
}

/// Payload binding a pending decision event to concrete world entities (for player choice or follow-up).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingEventPayload {
    BilateralContract {
        planet_id: String,
        contract_id: u32,
        seller: CounterpartyId,
        buyer: CounterpartyId,
        sector_id: SectorId,
        good: GoodId,
        qty: i64,
        unit_price_cents: i64,
    },
    CouponClaim {
        planet_id: String,
        issuer: CounterpartyId,
        claim_good: GoodId,
        coupon_cents_per_unit: i64,
    },
    Custom {
        planet_id: String,
        seller: CounterpartyId,
        buyer: CounterpartyId,
    },
    BankruptSlot {
        planet_id: String,
        tile_index: usize,
        slot: crate::building::TileSlotKey,
        admin_polity: crate::economy::PolityId,
        home_sector: SectorId,
        salvage_price_cents: i64,
    },
}

/// Queued decision event awaiting manual choice by a player-controlled `CounterpartyId`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEvent {
    pub instance_id: u64,
    pub event_id: String,
    pub title: String,
    pub target_actor: CounterpartyId,
    pub payload: PendingEventPayload,
    pub enabled_options: Vec<(usize, String, String)>,
}

/// Outcome summary produced by executing an event option's `RuleEffect` list on real `Inventory`s.
#[derive(Clone, Debug, Default)]
pub struct EffectExecutionReport {
    pub delivered_qty: i64,
    pub cash_paid_cents: i64,
    pub seller_rating_delta: i16,
    pub buyer_rating_delta: i16,
    pub default_increment: u16,
    pub contract_cancelled: bool,
    pub investment_approved: bool,
    pub bankruptcy_acquisition_approved: bool,
    pub followup_events: Vec<(String, EventParty)>,
}

/// Executes a slice of `RuleEffect`s against `invs` with strict physical & cash conservation.
#[allow(clippy::too_many_arguments)]
pub fn execute_rule_effects(
    effects: &[RuleEffect],
    ctx: &RuleContext,
    goods: &GoodCatalog,
    invs: &mut [Inventory],
    seller_goods_inv: usize,
    seller_cash_inv: usize,
    buyer_goods_inv: usize,
    buyer_cash_inv: usize,
    contract_good: Option<GoodId>,
    coupon_holders: &[(usize, i64)],
    coupon_cents_per_unit: i64,
    report: &mut EffectExecutionReport,
) {
    let cash_idx = usize::from(goods.cash_id().0);
    for eff in effects {
        match eff {
            RuleEffect::ExecuteContractDelivery => {
                let Some(gid) = contract_good else {
                    continue;
                };
                let g_idx = usize::from(gid.0);
                let seller_avail = invs[seller_goods_inv].avail(gid);
                let buyer_fit = invs[buyer_goods_inv].max_receivable_units(goods, gid);
                let cap_qty = ctx.contract_qty.min(buyer_fit).min(seller_avail).max(0);
                let unit_px = ctx.unit_price_cents.max(0);
                let buyer_cash = invs[buyer_cash_inv].available[cash_idx].max(0);
                let qty = if unit_px > 0 {
                    (buyer_cash / unit_px).min(cap_qty).max(0)
                } else {
                    cap_qty
                };
                if qty > 0 {
                    if seller_goods_inv != buyer_goods_inv {
                        invs[seller_goods_inv].available[g_idx] -= qty;
                        invs[buyer_goods_inv].available[g_idx] += qty;
                    }
                    let bill = qty * unit_px;
                    if bill > 0 && buyer_cash_inv != seller_cash_inv {
                        let moved = bill.min(invs[buyer_cash_inv].available[cash_idx]).max(0);
                        invs[buyer_cash_inv].available[cash_idx] -= moved;
                        invs[seller_cash_inv].available[cash_idx] += moved;
                        report.cash_paid_cents += moved;
                    }
                    report.delivered_qty += qty;
                }
            }
            RuleEffect::ExecuteCouponPayout => {
                if coupon_holders.is_empty() || coupon_cents_per_unit <= 0 {
                    continue;
                }
                let total_units: i64 = coupon_holders.iter().map(|&(_, u)| u.max(0)).sum();
                if total_units <= 0 {
                    continue;
                }
                let full_due = total_units.saturating_mul(coupon_cents_per_unit);
                let avail_cash = invs[seller_cash_inv].available[cash_idx].max(0);
                let budget = full_due.min(avail_cash);
                if budget <= 0 {
                    continue;
                }
                for &(h_inv, units) in coupon_holders {
                    if units <= 0 || h_inv == seller_cash_inv {
                        continue;
                    }
                    let share = ((i128::from(budget) * i128::from(units))
                        / i128::from(total_units)) as i64;
                    let moved = share.min(invs[seller_cash_inv].available[cash_idx]).max(0);
                    if moved > 0 {
                        invs[seller_cash_inv].available[cash_idx] -= moved;
                        invs[h_inv].available[cash_idx] += moved;
                        report.cash_paid_cents += moved;
                    }
                }
            }
            RuleEffect::TransferCash {
                from,
                to,
                amount_cents,
            } => {
                let src = match from {
                    EventParty::Seller => seller_cash_inv,
                    EventParty::Buyer => buyer_cash_inv,
                };
                let dst = match to {
                    EventParty::Seller => seller_cash_inv,
                    EventParty::Buyer => buyer_cash_inv,
                };
                if src != dst && *amount_cents > 0 {
                    let moved = (*amount_cents).min(invs[src].available[cash_idx]).max(0);
                    invs[src].available[cash_idx] -= moved;
                    invs[dst].available[cash_idx] += moved;
                    report.cash_paid_cents += moved;
                }
            }
            RuleEffect::AdjustCreditRating { party, delta } => match party {
                EventParty::Seller => {
                    report.seller_rating_delta = report.seller_rating_delta.saturating_add(*delta);
                }
                EventParty::Buyer => {
                    report.buyer_rating_delta = report.buyer_rating_delta.saturating_add(*delta);
                }
            },
            RuleEffect::IncrementContractDefault => {
                report.default_increment = report.default_increment.saturating_add(1);
            }
            RuleEffect::CancelContract => {
                report.contract_cancelled = true;
            }
            RuleEffect::ApproveSectorInvestment => {
                report.investment_approved = true;
            }
            RuleEffect::ApproveBankruptcyAcquisition => {
                report.bankruptcy_acquisition_approved = true;
            }
            RuleEffect::If {
                cond,
                then_effects,
                else_effects,
            } => {
                let branch = if cond.eval(ctx) {
                    then_effects
                } else {
                    else_effects
                };
                execute_rule_effects(
                    branch,
                    ctx,
                    goods,
                    invs,
                    seller_goods_inv,
                    seller_cash_inv,
                    buyer_goods_inv,
                    buyer_cash_inv,
                    contract_good,
                    coupon_holders,
                    coupon_cents_per_unit,
                    report,
                );
            }
            RuleEffect::QueueFollowupEvent { event_id, target } => {
                report.followup_events.push((event_id.clone(), *target));
            }
        }
    }
}

/// Editable catalog of declarative events & `if-else +-` decision pulses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCatalog {
    defs: Vec<EventDef>,
    by_id: BTreeMap<String, usize>,
}

impl Default for EventCatalog {
    fn default() -> Self {
        crate::data_bundle::load_events_from_disk()
            .expect("加载外部 gamedata/rules/events.jsonc 失败")
    }
}

impl EventCatalog {
    pub fn from_defs(defs: Vec<EventDef>) -> Result<Self, String> {
        let mut by_id = BTreeMap::new();
        for (i, d) in defs.iter().enumerate() {
            if d.id.trim().is_empty() {
                return Err("事件 ID 不能为空。".into());
            }
            if d.options.is_empty() {
                return Err(format!("事件 {} 至少需要包含一个决策选项。", d.id));
            }
            if by_id.insert(d.id.clone(), i).is_some() {
                return Err(format!("重复的事件 ID：{}", d.id));
            }
        }
        Ok(Self { defs, by_id })
    }

    #[inline]
    pub fn get(&self, id: &str) -> Option<&EventDef> {
        self.by_id.get(id).and_then(|&i| self.defs.get(i))
    }

    #[inline]
    pub fn defs(&self) -> &[EventDef] {
        &self.defs
    }

    pub fn register_event(&mut self, def: EventDef) -> Result<(), String> {
        if def.id.trim().is_empty() {
            return Err("事件 ID 不能为空。".into());
        }
        if def.options.is_empty() {
            return Err(format!("事件 {} 至少需要包含一个决策选项。", def.id));
        }
        if let Some(&idx) = self.by_id.get(&def.id) {
            self.defs[idx] = def;
        } else {
            let idx = self.defs.len();
            self.by_id.insert(def.id.clone(), idx);
            self.defs.push(def);
        }
        Ok(())
    }
}
