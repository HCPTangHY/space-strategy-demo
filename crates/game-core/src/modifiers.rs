//! Placeholder hook for the future Buff & Stat-Modifier system (`Buff / 修正系统预留挂载层`).
//!
//! Objective formulas in `building`, `planet`, and `actors/*` evaluate deterministic entity
//! capacities and outputs; future buff effects will aggregate into `StatModifier` before
//! applying `(base + flat_add) * (1000 + permille_mul) / 1000`.

use serde::{Deserialize, Serialize};

/// Generic additive + multiplicative (`permille`, `1000 = +100%`) stat modifier accumulator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatModifier {
    pub flat_add: i64,
    pub permille_mul: i64,
}

impl StatModifier {
    pub const IDENTITY: Self = Self {
        flat_add: 0,
        permille_mul: 0,
    };

    #[inline]
    pub fn apply(self, base: i64) -> i64 {
        let added = base.saturating_add(self.flat_add);
        (added.saturating_mul(1000 + self.permille_mul)) / 1000
    }
}
