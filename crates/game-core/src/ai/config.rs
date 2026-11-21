//! Declarative AI Heuristic Parameters (`AI 决策启发式参数配置`).
//!
//! Strictly separated from objective entity/building formulas: changing `AiDecisionConfig`
//! adjusts how AI-managed workshops, households, private pools, enterprises, and local polities
//! quote prices, stockpile inputs, hire/renew staff, and select build sites without altering
//! physical recipes or warehouse capacities.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiDecisionConfig {
    /// Warehouse fill permille (`<= 350`) below which workshops quote full `max(cost, 100% MA)`.
    pub ask_low_fill_permille: i64,
    /// Warehouse fill permille (`<= 750`) separating normal trim (`95% MA`) from overstock clearance.
    pub ask_high_fill_permille: i64,
    /// Minimum MA permille (`800` = `80% MA`) during overstock clearance.
    pub ask_clearance_ma_permille: i64,
    /// Minimum unit-cost floor permille (`500` = `50% cost`) when warehouse is 100% full.
    pub ask_full_cost_floor_permille: i64,
    /// Liquidation ask price percent of MA (`50` = `50% MA`) after consecutive wage arrears.
    pub distressed_liquidation_ma_percent: i64,
    /// Target input buffer months (`2` = `2.0x` monthly consumption) for storable physical inputs.
    pub physical_input_buffer_months: i64,
    /// Urgent input bid premium percent (`112` = `112% MA`) when stock < 1 month of recipe inputs.
    pub urgent_input_bid_percent: i64,
    /// Household urgent staple bid percent (`125` = `125% MA`) when pantry is below 1 month need.
    pub household_urgent_staple_bid_percent: i64,
    /// Warehouse fill permille (`900` = `90%`) at or above which workshops pause new hiring.
    pub hiring_stop_fill_permille: i64,
    /// Maximum payback horizon in months (`20` => `>= 5%/month` ROI) for autonomous private factories.
    pub max_payback_months: i64,
    /// Minimum `SectorCapital` credits (`380 CR`) required before investing across sector borders.
    pub cross_sector_invest_min_credits: i64,
    /// Minimum fiscal reserve credits (`160 CR`) retained by sub-national local polities before building.
    pub local_polity_reserve_credits: i64,
    /// Regional slot auto-selection heuristic weights (`build_in_region`).
    pub site_scoring: SiteScoringWeights,
}

/// AI heuristic weights for regional automatic slot selection (`build_in_region`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteScoringWeights {
    pub affordable_bonus: f64,
    pub same_tile_sibling_bonus: f64,
    pub primary_slot_bonus: f64,
    pub land_tile_bonus: f64,
}

impl Default for SiteScoringWeights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl SiteScoringWeights {
    pub const DEFAULT: Self = Self {
        affordable_bonus: 200.0,
        same_tile_sibling_bonus: 50.0,
        primary_slot_bonus: 8.0,
        land_tile_bonus: 15.0,
    };
}

impl Default for AiDecisionConfig {
    fn default() -> Self {
        Self {
            ask_low_fill_permille: 350,
            ask_high_fill_permille: 750,
            ask_clearance_ma_permille: 800,
            ask_full_cost_floor_permille: 500,
            distressed_liquidation_ma_percent: 50,
            physical_input_buffer_months: 2,
            urgent_input_bid_percent: 112,
            household_urgent_staple_bid_percent: 125,
            hiring_stop_fill_permille: 900,
            max_payback_months: 20,
            cross_sector_invest_min_credits: 380,
            local_polity_reserve_credits: 160,
            site_scoring: SiteScoringWeights::DEFAULT,
        }
    }
}
