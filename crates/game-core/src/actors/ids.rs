//! Compact strongly-typed entity & ownership identifiers across economic tiers.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PolityId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SectorId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EnterpriseId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TrustId(pub u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PopTypeId(pub u16);

/// Stable per-sector handle uniquely identifying one `(pop_type, culture_id, employer)` `PopGroup`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PopId(pub u32);

/// Persistent locked labor contract stored on an individual building slot.

/// Compact ownership tag stored per tile building slot and on employed `PopGroup`s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuildingOwner {
    #[default]
    Unowned,
    State {
        polity_id: PolityId,
    },
    LocalPrivate {
        home_sector: SectorId,
    },
    Enterprise {
        enterprise_id: EnterpriseId,
    },
    Trust {
        trust_id: TrustId,
    },
}

pub(crate) const fn default_credit_rating() -> i16 {
    100
}

/// Named sovereign or corporate counterparty eligible for Channel-3 bilateral contracts & ratings.
/// By construction, `LocalPrivate` and `SectorCapital` cannot enter Channel 3!
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CounterpartyId {
    Polity { polity_id: PolityId },
    Enterprise { enterprise_id: EnterpriseId },
    Trust { trust_id: TrustId },
}

impl CounterpartyId {
    pub fn try_from_owner(owner: BuildingOwner) -> Result<Self, String> {
        match owner {
            BuildingOwner::State { polity_id } => Ok(Self::Polity { polity_id }),
            BuildingOwner::Enterprise { enterprise_id } => Ok(Self::Enterprise { enterprise_id }),
            BuildingOwner::Trust { trust_id } => Ok(Self::Trust { trust_id }),
            BuildingOwner::LocalPrivate { .. } => Err(
                "通道三禁区：本地民间私产（LocalPrivate / SectorCapital）属于匿名聚合池，禁止直接签署双边契约或计算信用评级。"
                    .into(),
            ),
            BuildingOwner::Unowned => Err("无产权归属对象不可作为双边契约主体。".into()),
        }
    }

    pub fn key_string(self) -> String {
        match self {
            Self::Polity { polity_id } => format!("polity:{}", polity_id.0),
            Self::Enterprise { enterprise_id } => format!("enterprise:{}", enterprise_id.0),
            Self::Trust { trust_id } => format!("trust:{}", trust_id.0),
        }
    }

    pub fn parse_key(s: &str) -> Option<Self> {
        let (kind, raw_id) = s.split_once(':')?;
        let id_u16: u16 = raw_id.parse().ok()?;
        match kind {
            "polity" => Some(Self::Polity {
                polity_id: PolityId(id_u16),
            }),
            "enterprise" => Some(Self::Enterprise {
                enterprise_id: EnterpriseId(id_u16),
            }),
            "trust" => Some(Self::Trust {
                trust_id: TrustId(id_u16),
            }),
            _ => None,
        }
    }
}
