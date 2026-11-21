//! Fiscal Tax-Sharing & Conservation Cash Transfer Helpers (`财政分税与现金守恒划转原语`).

use crate::economy::PolityId;
use crate::market::{GoodId, Inventory};
use std::collections::BTreeMap;

#[inline]
pub(crate) fn cash_idx(cash: GoodId) -> usize {
    usize::from(cash.0)
}

pub(crate) fn move_cash(invs: &mut [Inventory], cash: GoodId, from: usize, to: usize, amount: i64) -> i64 {
    let c = cash_idx(cash);
    let moved = amount.min(invs[from].available[c]).max(0);
    if moved > 0 && from != to {
        invs[from].available[c] -= moved;
        invs[to].available[c] += moved;
    }
    moved
}

#[derive(Clone, Copy)]
pub(crate) struct PolitySettleMeta {
    pub(crate) inv: usize,
    pub(crate) parent: Option<PolityId>,
    pub(crate) tax_rate_permille: i64,
    pub(crate) upward_share_permille: i64,
    pub(crate) credit_rating: i16,
    pub(crate) credit_delta: i16,
    pub(crate) vault_before: i64,
    pub(crate) tax_income: i64,
    pub(crate) state_net: i64,
    pub(crate) freight_rev: i64,
    pub(crate) seigniorage: i64,
    pub(crate) admin_upkeep: i64,
}

/// Direct source tax-sharing (`源头国地分流直征`): splits `gross_tax` directly out of `from_inv`
/// across `admin_polity` and its upward `parent_polity` chain without routing national tax through local vaults.
pub(crate) fn collect_tax_at_source(
    invs: &mut [Inventory],
    cash: GoodId,
    from_inv: usize,
    gross_tax: i64,
    admin_polity: PolityId,
    polity_meta: &mut BTreeMap<PolityId, PolitySettleMeta>,
    fallback_polity: PolityId,
) -> i64 {
    if gross_tax <= 0 {
        return 0;
    }
    let avail = invs[from_inv].avail(cash).max(0);
    let mut rem = gross_tax.min(avail);
    if rem <= 0 {
        return 0;
    }
    let mut total_paid = 0i64;
    let mut cur = if polity_meta.contains_key(&admin_polity) {
        admin_polity
    } else {
        fallback_polity
    };
    for _ in 0..16 {
        let Some(meta) = polity_meta.get(&cur).copied() else {
            break;
        };
        let (local_due, up_due) = if let Some(parent_id) = meta.parent {
            if polity_meta.contains_key(&parent_id) {
                let up = (rem * meta.upward_share_permille) / 1000;
                (rem - up, Some((parent_id, up)))
            } else {
                (rem, None)
            }
        } else {
            (rem, None)
        };
        if local_due > 0 {
            let moved = move_cash(invs, cash, from_inv, meta.inv, local_due);
            if let Some(m) = polity_meta.get_mut(&cur) {
                m.tax_income += moved;
            }
            total_paid += moved;
        }
        match up_due {
            Some((parent_id, up)) if up > 0 => {
                cur = parent_id;
                rem = up;
            }
            _ => break,
        }
    }
    total_paid
}
