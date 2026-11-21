//! Autonomous Entity AI Decision Subsystems (`AI 决策模块组`: 参数配置 / 盘口报价 / 用工 / 投资).

pub mod config;
pub mod hiring;
pub mod investment;
pub mod pricing;

pub use config::{AiDecisionConfig, SiteScoringWeights};
pub(crate) use pricing::{collect_monthly_commodity_orders, contract_redemption_priority};
#[cfg(test)]
pub(crate) use pricing::compute_workshop_ask_cents;
