//! Pure-Disk JSONC Data Loader & Cross-Catalog Schema Validator (`外部 gamedata 磁盘读取与热重载解析器`).
//!
//! Zero `include_str!` / `include_bytes!`: the compiled Rust binary is pure mechanism code
//! that reads `gamedata/` (`catalog/`, `rules/`, `maps/`) from disk at startup and hot-reload.

use crate::ai::AiDecisionConfig;
use crate::economy::{PopTypeCatalog, PopTypeDef};
use crate::market::{GoodCatalog, GoodDef, TransportChannel};
use crate::rules::{EventCatalog, EventDef, RuleEffect};
use crate::{BuildingCatalog, BuildingDef};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Strips `//` line comments, `/* ... */` block comments, and trailing commas before `]` / `}`
/// while preserving string literals and exact newline positions so `serde_json` line numbers stay accurate.
pub fn strip_jsonc(input: &str) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut no_comments = Vec::with_capacity(len);
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        let b = bytes[i];
        if in_string {
            no_comments.push(b);
            if b == b'\\' && i + 1 < len {
                no_comments.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if b == b'"' {
            in_string = true;
            no_comments.push(b);
            i += 1;
        } else if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
        } else if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    no_comments.push(b'\n');
                }
                i += 1;
            }
            if i + 1 < len {
                i += 2;
            } else {
                i = len;
            }
        } else {
            no_comments.push(b);
            i += 1;
        }
    }

    // Pass 2: replace trailing commas before `]` or `}` with spaces (preserving line/column layout).
    let mut in_str = false;
    let mut j = 0;
    while j < no_comments.len() {
        let b = no_comments[j];
        if in_str {
            if b == b'\\' && j + 1 < no_comments.len() {
                j += 2;
                continue;
            }
            if b == b'"' {
                in_str = false;
            }
            j += 1;
            continue;
        }
        if b == b'"' {
            in_str = true;
            j += 1;
            continue;
        }
        if b == b',' {
            let mut k = j + 1;
            while k < no_comments.len() && matches!(no_comments[k], b' ' | b'\t' | b'\r' | b'\n') {
                k += 1;
            }
            if k < no_comments.len() && matches!(no_comments[k], b']' | b'}') {
                no_comments[j] = b' ';
            }
        }
        j += 1;
    }

    String::from_utf8(no_comments).unwrap_or_else(|_| input.to_string())
}

/// Parses any `serde::Deserialize` structure from a JSONC string.
pub fn parse_jsonc<T: DeserializeOwned>(jsonc: &str) -> Result<T, String> {
    let cleaned = strip_jsonc(jsonc);
    serde_json::from_str(&cleaned).map_err(|e| format!("JSONC 解析失败：{e}"))
}

/// Parses a named JSONC object/value and prefixes any syntax/schema error with its filename.
pub fn parse_jsonc_named<T: DeserializeOwned>(file_label: &str, jsonc: &str) -> Result<T, String> {
    let cleaned = strip_jsonc(jsonc);
    serde_json::from_str(&cleaned).map_err(|e| format!("文件 {file_label} 解析失败：{e}"))
}

/// Parses a named JSONC array file and prefixes any syntax/schema error with its filename.
pub fn parse_jsonc_list_named<T: DeserializeOwned>(
    file_label: &str,
    jsonc: &str,
) -> Result<Vec<T>, String> {
    parse_jsonc_named::<Vec<T>>(file_label, jsonc)
}

/// Cross-sector infrastructure channel topology & pricing mode driven by staffed hub buildings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportHubKind {
    /// Adjacent land-sector multi-hop relay (`0.5 * src + sum(mid) + 0.5 * dst` × terrain factor).
    LandRelay,
    /// Coastal water-connected point-to-point shipping (`src_port + dst_port + sea_steps * distance_fee`).
    SeaPointToPoint,
    /// All-terrain planetary point-to-point airlift/suborbital (`src_port + dst_port + air_steps * distance_fee`).
    SuborbitalPointToPoint,
    /// Adjacent land-sector utility grid/pipeline relay for `TransportChannel::PowerGrid` (`base:power`, `base:water`).
    UtilityGridRelay,
}

/// Pure data specification mapping a cross-sector transport mode to its local order-book capacity flow good
/// and integer share consumption per unit of transported cargo volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportHubDef {
    pub building_id: String,
    pub kind: TransportHubKind,
    /// `LocalOnly` flow good produced by this hub and consumed on route order books (`base:cap_land`, etc.).
    pub capacity_good_id: String,
    /// Capacity shares consumed at origin and destination per `1` cargo volume (before terrain/distance multiplier).
    pub endpoint_shares: i64,
    /// Capacity shares consumed at each intermediate transit sector (`>0` for multi-hop land/grid relays).
    #[serde(default)]
    pub transit_shares: i64,
    /// Distance step divisor adding `+1` share at each endpoint (`0` = no distance step surcharge).
    #[serde(default)]
    pub steps_per_extra_share: u16,
    /// Maximum extra distance shares added per endpoint.
    #[serde(default)]
    pub max_extra_shares: i64,
}

/// Assembled raw economy definitions (from split modular JSONC files or a combined mod bundle).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyBundleDef {
    pub schema_version: u32,
    pub pop_types: Vec<PopTypeDef>,
    pub goods: Vec<GoodDef>,
    pub buildings: Vec<BuildingDef>,
    #[serde(default)]
    pub transport_hubs: Vec<TransportHubDef>,
}

/// Validated runtime catalogs compiled from `EconomyBundleDef`.
#[derive(Clone, Debug)]
pub struct EconomyBundle {
    pub schema_version: u32,
    pub pop_types: PopTypeCatalog,
    pub goods: GoodCatalog,
    pub buildings: BuildingCatalog,
    pub transport_hubs: Vec<TransportHubDef>,
}

impl EconomyBundle {
    pub fn from_def(def: EconomyBundleDef) -> Result<Self, String> {
        let pop_types = PopTypeCatalog::from_defs(def.pop_types)?;
        let goods = GoodCatalog::from_defs(def.goods)?;
        let buildings = BuildingCatalog::from_defs(def.buildings)?;

        // Cross-reference validation across buildings, pop roles, goods, and transport hubs
        for b in buildings.defs() {
            for (role_id, count) in &b.jobs_by_role {
                if *count == 0 {
                    return Err(format!("建筑 {} 包含人数为 0 的岗位 {}", b.id, role_id));
                }
                if pop_types.id_of(role_id).is_none() {
                    return Err(format!("建筑 {} 引用了未知的职业 ID：{}", b.id, role_id));
                }
            }
            for (gid_str, qty) in b.monthly_inputs.iter().chain(b.monthly_output.iter()) {
                if *qty <= 0 {
                    return Err(format!("建筑 {} 的配方商品 {} 数量必须大于 0", b.id, gid_str));
                }
                let Some(gid) = goods.id_of(gid_str) else {
                    return Err(format!("建筑 {} 引用了未知的商品 ID：{}", b.id, gid_str));
                };
                if goods.get(gid).is_currency {
                    return Err(format!("建筑 {} 不可将法定货币 {} 列入物理配方", b.id, gid_str));
                }
            }
        }

        for hub in &def.transport_hubs {
            let b = buildings.by_id(&hub.building_id)?;
            let Some(cap_gid) = goods.id_of(&hub.capacity_good_id) else {
                return Err(format!(
                    "跨区枢纽 {} 引用了未知的运力商品 ID：{}",
                    hub.building_id, hub.capacity_good_id
                ));
            };
            let cap_gdef = goods.get(cap_gid);
            if cap_gdef.transport_channel != TransportChannel::LocalOnly
                || cap_gdef.retention_permille != 0
                || cap_gdef.unit_volume != 0
            {
                return Err(format!(
                    "运力商品 {} 必须为零体积、月末清零的 local_only 流量商品",
                    hub.capacity_good_id
                ));
            }
            if !b.monthly_output.iter().any(|(gid, q)| gid == &hub.capacity_good_id && *q > 0) {
                return Err(format!(
                    "跨区枢纽建筑 {} 必须在 monthly_output 中产出 {}",
                    hub.building_id, hub.capacity_good_id
                ));
            }
            if hub.endpoint_shares <= 0 {
                return Err(format!("跨区枢纽 {} 的端点运力消耗份额必须大于 0", hub.building_id));
            }
        }

        Ok(Self {
            schema_version: def.schema_version,
            pop_types,
            goods,
            buildings,
            transport_hubs: def.transport_hubs,
        })
    }

    /// Assembles and validates an `EconomyBundle` from split modular `(filename, jsonc)` slices.
    pub fn from_split_jsonc(
        schema_version: u32,
        pop_type_files: &[(&str, &str)],
        good_files: &[(&str, &str)],
        building_files: &[(&str, &str)],
        transport_hub_files: &[(&str, &str)],
    ) -> Result<Self, String> {
        let mut pop_types = Vec::new();
        for &(name, src) in pop_type_files {
            pop_types.extend(parse_jsonc_list_named::<PopTypeDef>(name, src)?);
        }
        let mut goods = Vec::new();
        for &(name, src) in good_files {
            goods.extend(parse_jsonc_list_named::<GoodDef>(name, src)?);
        }
        let mut buildings = Vec::new();
        for &(name, src) in building_files {
            buildings.extend(parse_jsonc_list_named::<BuildingDef>(name, src)?);
        }
        let mut transport_hubs = Vec::new();
        for &(name, src) in transport_hub_files {
            transport_hubs.extend(parse_jsonc_list_named::<TransportHubDef>(name, src)?);
        }
        Self::from_def(EconomyBundleDef {
            schema_version,
            pop_types,
            goods,
            buildings,
            transport_hubs,
        })
    }

    pub fn from_jsonc_str(jsonc: &str) -> Result<Self, String> {
        let def: EconomyBundleDef = parse_jsonc(jsonc)?;
        Self::from_def(def)
    }
}

/// Complete external disk dataset (`catalog/*` + `rules/ai_decision.jsonc` + `rules/events.jsonc`).
#[derive(Clone, Debug)]
pub struct FullGameData {
    pub economy: EconomyBundle,
    pub ai_config: AiDecisionConfig,
    pub events: EventCatalog,
}

/// Resolves the external `gamedata/` root directory on disk.
pub fn resolve_gamedata_dir() -> Result<PathBuf, String> {
    if let Ok(custom) = std::env::var("SPACE_STRATEGY_DATA_DIR") {
        let p = PathBuf::from(custom.trim());
        if p.join("catalog/goods.jsonc").is_file() {
            return Ok(p);
        }
        return Err(format!(
            "环境变量 SPACE_STRATEGY_DATA_DIR 指向的目录缺少 catalog/goods.jsonc：{}",
            p.display()
        ));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("gamedata"));
        candidates.push(cwd.join("../gamedata"));
        candidates.push(cwd.join("../../gamedata"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            candidates.push(bin_dir.join("gamedata"));
            candidates.push(bin_dir.join("../gamedata"));
            candidates.push(bin_dir.join("../../gamedata"));
            candidates.push(bin_dir.join("../../../gamedata"));
        }
    }

    for cand in candidates {
        if cand.join("catalog/goods.jsonc").is_file() {
            return Ok(cand);
        }
    }
    Err("未找到外部 gamedata/ 数据目录（可通过 SPACE_STRATEGY_DATA_DIR 指定路径）。".into())
}

fn read_utf8_file(dir: &Path, rel: &str) -> Result<String, String> {
    let full = dir.join(rel);
    fs::read_to_string(&full).map_err(|e| format!("读取数据文件 {} 失败：{e}", full.display()))
}

fn validate_followup_effects(
    effects: &[RuleEffect],
    catalog: &EventCatalog,
    owner_id: &str,
) -> Result<(), String> {
    for eff in effects {
        match eff {
            RuleEffect::QueueFollowupEvent { event_id, .. } => {
                if catalog.get(event_id).is_none() {
                    return Err(format!(
                        "事件 {owner_id} 引用了不存在的后续事件 ID：{event_id}"
                    ));
                }
            }
            RuleEffect::If {
                then_effects,
                else_effects,
                ..
            } => {
                validate_followup_effects(then_effects, catalog, owner_id)?;
                validate_followup_effects(else_effects, catalog, owner_id)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Loads `EconomyBundle` from `dir/catalog/` on disk (scanning all `catalog/buildings/*.jsonc`).
pub fn load_economy_bundle_from_dir(dir: &Path) -> Result<EconomyBundle, String> {
    let pop_src = read_utf8_file(dir, "catalog/pop_types.jsonc")?;
    let goods_src = read_utf8_file(dir, "catalog/goods.jsonc")?;
    let hubs_src = read_utf8_file(dir, "catalog/transport_hubs.jsonc")?;

    let b_dir = dir.join("catalog/buildings");
    let entries =
        fs::read_dir(&b_dir).map_err(|e| format!("读取建筑目录 {} 失败：{e}", b_dir.display()))?;
    let mut b_paths: Vec<PathBuf> = entries
        .filter_map(|res| res.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()).is_some_and(|ext| {
                ext.eq_ignore_ascii_case("jsonc") || ext.eq_ignore_ascii_case("json")
            })
        })
        .collect();
    b_paths.sort();
    if b_paths.is_empty() {
        return Err(format!("建筑目录 {} 下没有任何 .jsonc 文件。", b_dir.display()));
    }

    let mut b_owned: Vec<(String, String)> = Vec::with_capacity(b_paths.len());
    for p in b_paths {
        let label = format!(
            "catalog/buildings/{}",
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.jsonc")
        );
        let content =
            fs::read_to_string(&p).map_err(|e| format!("读取建筑文件 {} 失败：{e}", p.display()))?;
        b_owned.push((label, content));
    }
    let b_refs: Vec<(&str, &str)> = b_owned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    EconomyBundle::from_split_jsonc(
        1,
        &[("catalog/pop_types.jsonc", pop_src.as_str())],
        &[("catalog/goods.jsonc", goods_src.as_str())],
        &b_refs,
        &[("catalog/transport_hubs.jsonc", hubs_src.as_str())],
    )
}

/// Loads `EventCatalog` from `dir/rules/events.jsonc` on disk and validates follow-up links.
pub fn load_events_from_dir(dir: &Path) -> Result<EventCatalog, String> {
    let raw = read_utf8_file(dir, "rules/events.jsonc")?;
    let defs = parse_jsonc_list_named::<EventDef>("rules/events.jsonc", &raw)?;
    let cat = EventCatalog::from_defs(defs)?;
    for ev in cat.defs() {
        for opt in &ev.options {
            validate_followup_effects(&opt.effects, &cat, &ev.id)?;
        }
    }
    Ok(cat)
}

pub fn load_events_from_disk() -> Result<EventCatalog, String> {
    let dir = resolve_gamedata_dir()?;
    load_events_from_dir(&dir)
}

/// Loads `AiDecisionConfig` from `dir/rules/ai_decision.jsonc` on disk.
pub fn load_ai_config_from_dir(dir: &Path) -> Result<AiDecisionConfig, String> {
    let raw = read_utf8_file(dir, "rules/ai_decision.jsonc")?;
    let cfg: AiDecisionConfig = parse_jsonc_named("rules/ai_decision.jsonc", &raw)?;
    if cfg.max_payback_months <= 0 {
        return Err("文件 rules/ai_decision.jsonc 中 max_payback_months 必须大于 0。".into());
    }
    Ok(cfg)
}

/// Loads and cross-validates the complete external `gamedata/` directory (`catalog/*` + `rules/*`).
pub fn load_full_gamedata_from_dir(dir: &Path) -> Result<FullGameData, String> {
    let economy = load_economy_bundle_from_dir(dir)?;
    let ai_config = load_ai_config_from_dir(dir)?;
    let events = load_events_from_dir(dir)?;
    Ok(FullGameData {
        economy,
        ai_config,
        events,
    })
}

pub fn load_full_gamedata() -> Result<FullGameData, String> {
    let dir = resolve_gamedata_dir()?;
    load_full_gamedata_from_dir(&dir)
}

/// Loads and validates the external `gamedata/catalog/` economy bundle from disk.
pub fn load_economy_v1_bundle() -> Result<EconomyBundle, String> {
    let dir = resolve_gamedata_dir()?;
    load_economy_bundle_from_dir(&dir)
}

/// Recursively computes the newest file modification timestamp inside `dir` (for optional auto-reload).
pub fn gamedata_latest_mtime(dir: &Path) -> Option<SystemTime> {
    fn walk(p: &Path, best: &mut Option<SystemTime>) {
        let Ok(meta) = fs::metadata(p) else {
            return;
        };
        if meta.is_dir() {
            if let Ok(rd) = fs::read_dir(p) {
                for entry in rd.flatten() {
                    walk(&entry.path(), best);
                }
            }
        } else if let Ok(m) = meta.modified() {
            if best.is_none_or(|cur| m > cur) {
                *best = Some(m);
            }
        }
    }
    let mut best = None;
    walk(dir, &mut best);
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::WORKSHOP_BASE_CAPACITY;
    use crate::market::{CASH_SCALE, MAX_ORDER_PRICE_CENTS, MIN_ORDER_PRICE_CENTS};
    use std::collections::BTreeSet;

    #[test]
    fn strip_jsonc_preserves_strings_and_removes_comments_and_trailing_commas() {
        let raw = r#"{
            // 行注释
            "url": "https://example.com//not-a-comment/*still-string*/",
            /* 多行
               块注释 */
            "items": [1, 2, 3, ],
        }"#;
        let v: serde_json::Value = parse_jsonc(raw).expect("JSONC 应成功解析");
        assert_eq!(
            v["url"].as_str(),
            Some("https://example.com//not-a-comment/*still-string*/")
        );
        assert_eq!(v["items"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn economy_v1_split_jsonc_loads_validates_warehouse_fit_closure_and_sraffa_positive_margins() {
        let bundle = load_economy_v1_bundle().expect("economy_v1 分片 JSONC 必须通过完整交叉校验");

        // 1. 规模核验：1 货币 + 1 直辖行政额度 + 18 可交易商品（含 4 类运力流量商品），7 类职业，26 座建筑，4 类跨区战略基建
        assert_eq!(bundle.goods.len(), 20);
        assert_eq!(bundle.goods.tradable_good_ids().len(), 18);
        assert_eq!(bundle.pop_types.len(), 7);
        assert_eq!(bundle.buildings.defs().len(), 26);
        assert_eq!(bundle.transport_hubs.len(), 4);

        // 2. 初始基准价必须在盘口合法区间内留足上下浮动空间
        for gid in bundle.goods.tradable_good_ids() {
            let g = bundle.goods.get(gid);
            assert!(
                g.genesis_price_cents >= MIN_ORDER_PRICE_CENTS * 2
                    && g.genesis_price_cents <= MAX_ORDER_PRICE_CENTS / 2,
                "商品 {} 初始价 {} 超出合理浮动安全带",
                g.id,
                g.genesis_price_cents
            );
        }

        // 3. 全链路供需闭合：14 种非货币商品每一种都有生产建筑，且每一种都有下游消耗方（无断头废品）
        let mut produced: BTreeSet<String> = BTreeSet::new();
        let mut consumed: BTreeSet<String> = BTreeSet::new();
        for g in bundle.goods.defs() {
            if !g.is_currency && (g.staple_demand_per_100 > 0 || g.service_demand_per_100 > 0) {
                consumed.insert(g.id.clone());
            }
        }
        for b in bundle.buildings.defs() {
            for (in_id, _) in &b.monthly_inputs {
                consumed.insert(in_id.clone());
            }
            for (out_id, _) in &b.monthly_output {
                produced.insert(out_id.clone());
            }
        }
        for hub in &bundle.transport_hubs {
            consumed.insert(hub.capacity_good_id.clone());
        }
        for gid in bundle.goods.tradable_good_ids() {
            let id = &bundle.goods.get(gid).id;
            assert!(produced.contains(id), "商品 {id} 没有任何生产建筑！");
            assert!(consumed.contains(id), "商品 {id} 没有任何下游消耗方（凑数废品）！");
        }

        // 4. 单槽仓库容量安全 + 产销工场 Sraffa 基准正毛利校验
        for b in bundle.buildings.defs() {
            let mut wage_bill_cents = 0i64;
            for (role_str, cnt) in &b.jobs_by_role {
                let pid = bundle.pop_types.id_of(role_str).unwrap();
                let base_w_cents =
                    bundle.pop_types.get(pid).base_wage_per_100 * CASH_SCALE / 100;
                wage_bill_cents += i64::from(*cnt) * base_w_cents;
            }

            let mut input_cost_cents = 0i64;
            let mut input_vol = 0i64;
            for (in_str, qty) in &b.monthly_inputs {
                let gid = bundle.goods.id_of(in_str).unwrap();
                let g = bundle.goods.get(gid);
                input_cost_cents += *qty * g.genesis_price_cents;
                input_vol += *qty * i64::from(g.unit_volume);
            }

            let mut output_rev_cents = 0i64;
            let mut output_vol = 0i64;
            for (out_str, qty) in &b.monthly_output {
                let gid = bundle.goods.id_of(out_str).unwrap();
                let g = bundle.goods.get(gid);
                output_rev_cents += *qty * g.genesis_price_cents;
                output_vol += *qty * i64::from(g.unit_volume);
            }

            // 3 个月原料安全缓冲 + 2 个月成品待售体积不得撑爆 1x 工场仓库 (120)
            let peak_buffer_vol = input_vol * 3 + output_vol * 2;
            assert!(
                peak_buffer_vol <= WORKSHOP_BASE_CAPACITY,
                "建筑 {} 缓冲体积 {} 超出单槽仓库上限 {}",
                b.id,
                peak_buffer_vol,
                WORKSHOP_BASE_CAPACITY
            );

            if b.is_privatizable_industry() {
                let total_cost = wage_bill_cents + input_cost_cents;
                let profit = output_rev_cents - total_cost;
                let margin_permille = (profit * 1000) / output_rev_cents.max(1);
                assert!(
                    profit > 0 && margin_permille >= 130,
                    "可私营产销建筑 {} 在基准价下毛利不足：营收={}c 成本={}c 毛利={}c ({}‰)",
                    b.id,
                    output_rev_cents,
                    total_cost,
                    profit,
                    margin_permille
                );
            }
        }

        // 5. 四座跨区战略基建在 70% 标准负载下运力销售营收必须覆盖工资与物料成本
        for hub in &bundle.transport_hubs {
            let b = bundle.buildings.by_id(&hub.building_id).unwrap();
            let wage_bill_cents: i64 = b
                .jobs_by_role
                .iter()
                .map(|(r, c)| {
                    let pid = bundle.pop_types.id_of(r).unwrap();
                    i64::from(*c) * (bundle.pop_types.get(pid).base_wage_per_100 * CASH_SCALE / 100)
                })
                .sum();
            let input_cost_cents: i64 = b
                .monthly_inputs
                .iter()
                .map(|(g, q)| *q * bundle.goods.get(bundle.goods.id_of(g).unwrap()).genesis_price_cents)
                .sum();
            let cap_gid = bundle.goods.id_of(&hub.capacity_good_id).unwrap();
            let cap_px = bundle.goods.get(cap_gid).genesis_price_cents;
            let cap_out: i64 = b
                .monthly_output
                .iter()
                .filter(|(g, _)| g == &hub.capacity_good_id)
                .map(|(_, q)| *q)
                .sum();
            let rev_70 = (cap_out * 70 / 100) * cap_px;
            assert!(
                rev_70 > wage_bill_cents + input_cost_cents,
                "跨区基建 {} 在 70% 负载下亏损：营收={}c 成本={}c",
                hub.building_id,
                rev_70,
                wage_bill_cents + input_cost_cents
            );
        }
    }

    #[test]
    fn bundled_transport_bottleneck_call_auction_modal_overflow_grid_and_economy_v1_sim_work() {
        use crate::economy::GeoTransportTopology;
        use crate::market::{
            clear_bundled_transport_markets, release_resting_orders, BundledModeSpec,
            BundledTransportNetwork, BuySubmission, GoodId, Inventory, SectorOrderBook,
            SellSubmission,
        };
        use crate::{CalendarConfig, Game};
        use std::collections::BTreeMap;

        let bundle = load_economy_v1_bundle().unwrap();
        let goods = &bundle.goods;
        let cash = goods.cash_id();
        let food = goods.id_of("base:agri_food").unwrap();
        let ore = goods.id_of("base:basic_ore").unwrap();
        let elec = goods.id_of("base:electronics").unwrap();
        let power = goods.id_of("base:power").unwrap();
        let cap_land = goods.id_of("base:cap_land").unwrap();
        let cap_sea = goods.id_of("base:cap_sea").unwrap();
        let cap_air = goods.id_of("base:cap_air").unwrap();
        let cap_grid = goods.id_of("base:cap_grid").unwrap();

        let modes: Vec<BundledModeSpec> = bundle
            .transport_hubs
            .iter()
            .map(|h| BundledModeSpec {
                kind: h.kind,
                cap_good: goods.id_of(&h.capacity_good_id).unwrap(),
                endpoint_shares: h.endpoint_shares,
                steps_per_extra_share: h.steps_per_extra_share,
                max_extra_shares: h.max_extra_shares,
            })
            .collect();

        // ── Case 1: A (0) -> B (1, congested bottleneck) -> C (2, oversupplied) Land Relay ──
        let geo = GeoTransportTopology {
            region_count: 3,
            land_terrain_mult: vec![
                0, 1, 0, // A <-> B flat plain (mult 1)
                1, 0, 1, // B <-> C flat plain (mult 1)
                0, 1, 0,
            ],
            sea_steps: vec![GeoTransportTopology::UNREACHABLE_STEPS; 9],
            coast_count: 0,
            tile_coast_id: Vec::new(),
            coast_sea_steps: Vec::new(),
            air_steps: vec![
                0, 3, 6, //
                3, 0, 3, //
                6, 3, 0,
            ],
        };

        // Inventories:
        // 0: Polity vault, 1: A farm, 2: A rail, 3: B bottleneck road/rail, 4: C oversupplied rail,
        // 5: C urgent buyer (bid 16 CR), 6: C regular buyer (bid 14 CR)
        let mut invs = vec![
            Inventory::new(goods.len(), 10_000),
            Inventory::new(goods.len(), 500),
            Inventory::new(goods.len(), 500),
            Inventory::new(goods.len(), 500),
            Inventory::new(goods.len(), 500),
            Inventory::new(goods.len(), 200),
            Inventory::new(goods.len(), 200),
        ];
        invs[1].produce_capped(goods, food, 100);
        invs[2].produce_capped(goods, cap_land, 100);
        invs[3].produce_capped(goods, cap_land, 20); // 2 shares/transit -> max 10 units!
        invs[4].produce_capped(goods, cap_land, 200);
        invs[5].available[usize::from(cash.0)] = 50_000;
        invs[6].available[usize::from(cash.0)] = 50_000;

        let cash_before: i64 = invs.iter().map(|i| i.total(cash)).sum();

        let mut good_books: BTreeMap<GoodId, Vec<SectorOrderBook>> = BTreeMap::new();
        for &g in &[food, cap_land] {
            let px = goods.get(g).genesis_price_cents;
            good_books.insert(g, (0..3).map(|_| SectorOrderBook::new(g, px)).collect());
        }

        let mut orders: BTreeMap<GoodId, (Vec<SellSubmission>, Vec<BuySubmission>)> =
            BTreeMap::new();
        orders.insert(
            cap_land,
            (
                vec![
                    SellSubmission {
                        sector_id: 0,
                        goods_inv: 2,
                        cash_inv: 2,
                        ask_price_cents: 100,
                        qty: 100,
                        allow_contract_pool: false,
                    },
                    SellSubmission {
                        sector_id: 1,
                        goods_inv: 3,
                        cash_inv: 3,
                        ask_price_cents: 100,
                        qty: 20,
                        allow_contract_pool: false,
                    },
                    SellSubmission {
                        sector_id: 2,
                        goods_inv: 4,
                        cash_inv: 4,
                        ask_price_cents: 50,
                        qty: 200,
                        allow_contract_pool: false,
                    },
                ],
                Vec::new(),
            ),
        );
        orders.insert(
            food,
            (
                vec![SellSubmission {
                    sector_id: 0,
                    goods_inv: 1,
                    cash_inv: 1,
                    ask_price_cents: 800,
                    qty: 100,
                    allow_contract_pool: false,
                }],
                vec![
                    BuySubmission {
                        sector_id: 2,
                        goods_inv: 5,
                        cash_inv: 5,
                        bid_price_cents: 1_600,
                        qty: 10,
                        allow_contract_pool: false,
                    },
                    BuySubmission {
                        sector_id: 2,
                        goods_inv: 6,
                        cash_inv: 6,
                        bid_price_cents: 1_400,
                        qty: 15,
                        allow_contract_pool: false,
                    },
                ],
            ),
        );

        let sec_geo = [0u16, 1, 2];
        let emb = [false; 9];
        let tariffs = [
            0, 0, 100, // 1.00 CR customs tariff into C from A!
            0, 0, 0, //
            0, 0, 0,
        ];
        let tariff_recv = [0usize, 0, 0];
        let mut pol_rev = BTreeMap::from([(0usize, 0i64)]);
        let net = BundledTransportNetwork {
            sector_count: 3,
            sector_geo_region: &sec_geo,
            geo: &geo,
            modes: &modes,
            inv_coast_id: &[],
            embargoed: &emb,
            tariff_per_vol_cents: &tariffs,
            tariff_receiver_inv: &tariff_recv,
        };

        clear_bundled_transport_markets(
            goods,
            &net,
            &mut good_books,
            orders,
            &mut invs,
            &mut pol_rev,
        );
        for (&g, books) in good_books.iter_mut() {
            release_resting_orders(goods, g, books, &mut invs);
        }

        let cash_after: i64 = invs.iter().map(|i| i.total(cash)).sum();
        assert_eq!(cash_before, cash_after, "捆绑运力竞价必须严格保持货币守恒");
        assert_eq!(invs[5].total(food), 10, "B 区 20 份过境运力刚好放行前 10 份高价粮单");
        assert_eq!(invs[6].total(food), 0, "超出 B 区瓶颈运力的次高价买单被卡在门外");
        assert_eq!(invs[1].total(cash), 10 * 800, "A 区充裕农场按底价 8.00 CR 实收货款");
        assert_eq!(invs[2].total(cash), 10 * 100, "A 区未满载铁路按底价 1.00 CR 实收运费");
        assert_eq!(invs[4].total(cash), 10 * 50, "C 区过剩铁路按折价 0.50 CR 实收运费");
        assert_eq!(invs[0].total(cash), 10 * 100, "C 区东道海关精准收到 1.00 CR/件关税");
        assert!(
            invs[3].total(cash) >= 20 * 250,
            "唯一抢空的瓶颈 B 区运力必须吃到边际买方竞价溢价（实收 {} cents >> 底价 2000 cents）",
            invs[3].total(cash)
        );
        assert!(
            good_books[&cap_land][1].ma_price_cents > 120,
            "瓶颈 B 区陆运力 MA 必须被买方抢购推高：{}",
            good_books[&cap_land][1].ma_price_cents
        );
        assert!(
            good_books[&cap_land][2].ma_price_cents < 100,
            "过剩 C 区陆运力 MA 必须向 0.50 CR 下探：{}",
            good_books[&cap_land][2].ma_price_cents
        );

        // ── Case 2: Sea -> Suborbital Modal Overflow (Electronics flies, Bulk Ore waits) + PowerGrid Relay ──
        let geo_sea = GeoTransportTopology {
            region_count: 2,
            land_terrain_mult: vec![
                0, 1, //
                1, 0,
            ],
            sea_steps: vec![
                0, 3, // near-sea connected (1 share per port per vol)
                3, 0,
            ],
            coast_count: 0,
            tile_coast_id: Vec::new(),
            coast_sea_steps: Vec::new(),
            air_steps: vec![
                0, 4, // 1 share per suborbital port per vol
                4, 0,
            ],
        };
        let mut invs2 = vec![
            Inventory::new(goods.len(), 10_000), // 0: vault
            Inventory::new(goods.len(), 500),    // 1: S0 port + airport + grid + sellers
            Inventory::new(goods.len(), 500),    // 2: S1 port + airport + grid + buyers
        ];
        // Sea ports only have 4 shares each -> after 4 electronics cross by sea, sea is full;
        // remaining electronics overflow onto suborbital air (10 shares each), while bulk ore (6 CR, vol 2 -> 24 CR air fee) refuses air!
        invs2[1].produce_capped(goods, cap_sea, 4);
        invs2[2].produce_capped(goods, cap_sea, 4);
        invs2[1].produce_capped(goods, cap_air, 10);
        invs2[2].produce_capped(goods, cap_air, 10);
        invs2[1].produce_capped(goods, cap_grid, 10);
        invs2[2].produce_capped(goods, cap_grid, 10);
        invs2[1].produce_capped(goods, elec, 8);
        invs2[1].produce_capped(goods, ore, 10);
        invs2[1].produce_capped(goods, power, 10);
        invs2[2].available[usize::from(cash.0)] = 200_000;

        let mut books2: BTreeMap<GoodId, Vec<SectorOrderBook>> = BTreeMap::new();
        for &g in &[elec, ore, power, cap_sea, cap_air, cap_grid] {
            let px = goods.get(g).genesis_price_cents;
            books2.insert(g, (0..2).map(|_| SectorOrderBook::new(g, px)).collect());
        }
        let mut orders2: BTreeMap<GoodId, (Vec<SellSubmission>, Vec<BuySubmission>)> =
            BTreeMap::new();
        for &(cg, qty) in &[(cap_sea, 4), (cap_air, 10), (cap_grid, 10)] {
            let px = goods.get(cg).genesis_price_cents;
            orders2.insert(
                cg,
                (
                    vec![
                        SellSubmission {
                            sector_id: 0,
                            goods_inv: 1,
                            cash_inv: 1,
                            ask_price_cents: px,
                            qty,
                            allow_contract_pool: false,
                        },
                        SellSubmission {
                            sector_id: 1,
                            goods_inv: 2,
                            cash_inv: 2,
                            ask_price_cents: px,
                            qty,
                            allow_contract_pool: false,
                        },
                    ],
                    Vec::new(),
                ),
            );
        }
        orders2.insert(
            elec,
            (
                vec![SellSubmission {
                    sector_id: 0,
                    goods_inv: 1,
                    cash_inv: 1,
                    ask_price_cents: 2_800,
                    qty: 8,
                    allow_contract_pool: false,
                }],
                vec![BuySubmission {
                    sector_id: 1,
                    goods_inv: 2,
                    cash_inv: 2,
                    bid_price_cents: 4_500,
                    qty: 8,
                    allow_contract_pool: false,
                }],
            ),
        );
        orders2.insert(
            ore,
            (
                vec![SellSubmission {
                    sector_id: 0,
                    goods_inv: 1,
                    cash_inv: 1,
                    ask_price_cents: 600,
                    qty: 10,
                    allow_contract_pool: false,
                }],
                vec![BuySubmission {
                    sector_id: 1,
                    goods_inv: 2,
                    cash_inv: 2,
                    bid_price_cents: 1_400, // Enough for sea (6 + 6 = 12 CR), NOT enough for vol-2 air (6 + 24 = 30 CR)!
                    qty: 10,
                    allow_contract_pool: false,
                }],
            ),
        );
        orders2.insert(
            power,
            (
                vec![SellSubmission {
                    sector_id: 0,
                    goods_inv: 1,
                    cash_inv: 1,
                    ask_price_cents: 300,
                    qty: 10,
                    allow_contract_pool: false,
                }],
                vec![BuySubmission {
                    sector_id: 1,
                    goods_inv: 2,
                    cash_inv: 2,
                    bid_price_cents: 600,
                    qty: 6,
                    allow_contract_pool: false,
                }],
            ),
        );
        let net2 = BundledTransportNetwork {
            sector_count: 2,
            sector_geo_region: &[0, 1],
            geo: &geo_sea,
            modes: &modes,
            inv_coast_id: &[],
            embargoed: &[false; 4],
            tariff_per_vol_cents: &[0; 4],
            tariff_receiver_inv: &[0, 0],
        };
        let mut pol_rev2 = BTreeMap::new();
        clear_bundled_transport_markets(
            goods,
            &net2,
            &mut books2,
            orders2,
            &mut invs2,
            &mut pol_rev2,
        );
        assert_eq!(
            invs2[2].avail(elec),
            8,
            "高附加值精密电子应先占满 4 份廉价海运，剩余 4 份自动溢出改走亚轨道空运！"
        );
        assert_eq!(
            invs2[2].avail(ore),
            0,
            "海运被电子抢满后，粗重低价原矿（体积2）付不起 24 CR 亚轨道运费，自动放弃空运！"
        );
        assert_eq!(
            invs2[2].avail(power),
            6,
            "电力必须通过骨干管网输配力跨邻区成交 6 单位！"
        );

        // ── Case 2B: Isthmus East Coast vs West Coast Ports (`coast_id` O(1) isolation & cape routing!) ──
        let u = GeoTransportTopology::UNREACHABLE_STEPS;
        let geo_isthmus = GeoTransportTopology {
            region_count: 2,
            land_terrain_mult: vec![0; 4],
            sea_steps: vec![
                0, 2, // Region-level min would have falsely said 2 steps!
                2, 0,
            ],
            coast_count: 3,
            tile_coast_id: vec![0, 1, 2],
            coast_sea_steps: vec![
                0, u, u, // Coast 0 (East Inland Sea): unreachable to Coast 1 & Coast 2!
                u, 0, 2, // Coast 1 (Isthmus West Coast): 2 steps to Coast 2!
                u, 2, 0, // Coast 2 (Target West Island)
            ],
            air_steps: vec![
                0, 5, //
                5, 0,
            ],
        };

        // Step A: Only East Coast port (`inv 1 -> coast 0`) exists in Sector 0
        let mut invs_ist = vec![
            Inventory::new(goods.len(), 10_000), // 0: vault
            Inventory::new(goods.len(), 500),    // 1: S0 East Coast port (`coast 0`)
            Inventory::new(goods.len(), 500),    // 2: S0 West Coast port (`coast 1`, 0 shares initially!)
            Inventory::new(goods.len(), 500),    // 3: S1 West Island port (`coast 2`)
            Inventory::new(goods.len(), 500),    // 4: S0 farm seller
            Inventory::new(goods.len(), 500),    // 5: S1 buyer
        ];
        invs_ist[1].produce_capped(goods, cap_sea, 50);
        invs_ist[3].produce_capped(goods, cap_sea, 50);
        invs_ist[4].produce_capped(goods, food, 20);
        invs_ist[5].available[usize::from(cash.0)] = 50_000;
        let inv_coasts = [u16::MAX, 0, 1, 2, u16::MAX, u16::MAX];

        let mut books_ist: BTreeMap<GoodId, Vec<SectorOrderBook>> = BTreeMap::new();
        for &g in &[food, cap_sea] {
            let px = goods.get(g).genesis_price_cents;
            books_ist.insert(g, (0..2).map(|_| SectorOrderBook::new(g, px)).collect());
        }
        let mut orders_ist_a: BTreeMap<GoodId, (Vec<SellSubmission>, Vec<BuySubmission>)> =
            BTreeMap::new();
        orders_ist_a.insert(
            cap_sea,
            (
                vec![
                    SellSubmission {
                        sector_id: 0,
                        goods_inv: 1,
                        cash_inv: 1,
                        ask_price_cents: 150,
                        qty: 50,
                        allow_contract_pool: false,
                    },
                    SellSubmission {
                        sector_id: 1,
                        goods_inv: 3,
                        cash_inv: 3,
                        ask_price_cents: 150,
                        qty: 50,
                        allow_contract_pool: false,
                    },
                ],
                Vec::new(),
            ),
        );
        orders_ist_a.insert(
            food,
            (
                vec![SellSubmission {
                    sector_id: 0,
                    goods_inv: 4,
                    cash_inv: 4,
                    ask_price_cents: 800,
                    qty: 20,
                    allow_contract_pool: false,
                }],
                vec![BuySubmission {
                    sector_id: 1,
                    goods_inv: 5,
                    cash_inv: 5,
                    bid_price_cents: 1_800,
                    qty: 15,
                    allow_contract_pool: false,
                }],
            ),
        );
        let net_ist = BundledTransportNetwork {
            sector_count: 2,
            sector_geo_region: &[0, 1],
            geo: &geo_isthmus,
            modes: &modes,
            inv_coast_id: &inv_coasts,
            embargoed: &[false; 4],
            tariff_per_vol_cents: &[0; 4],
            tariff_receiver_inv: &[0, 0],
        };
        let mut pol_rev_ist = BTreeMap::new();
        clear_bundled_transport_markets(
            goods,
            &net_ist,
            &mut books_ist,
            orders_ist_a,
            &mut invs_ist,
            &mut pol_rev_ist,
        );
        for (&g, books) in books_ist.iter_mut() {
            release_resting_orders(goods, g, books, &mut invs_ist);
        }
        assert_eq!(
            invs_ist[5].total(food),
            0,
            "仅在东海岸内陆海建港时，绝不可借西海岸向西洋岛屿发货！"
        );

        // Step B: Now open the West Coast port (`inv 2 -> coast 1`, 6 shares) alongside the 50-share East port!
        invs_ist[2].produce_capped(goods, cap_sea, 6);
        let mut orders_ist_b: BTreeMap<GoodId, (Vec<SellSubmission>, Vec<BuySubmission>)> =
            BTreeMap::new();
        orders_ist_b.insert(
            cap_sea,
            (
                vec![
                    SellSubmission {
                        sector_id: 0,
                        goods_inv: 1,
                        cash_inv: 1,
                        ask_price_cents: 150,
                        qty: 50,
                        allow_contract_pool: false,
                    },
                    SellSubmission {
                        sector_id: 0,
                        goods_inv: 2,
                        cash_inv: 2,
                        ask_price_cents: 150,
                        qty: 6,
                        allow_contract_pool: false,
                    },
                    SellSubmission {
                        sector_id: 1,
                        goods_inv: 3,
                        cash_inv: 3,
                        ask_price_cents: 150,
                        qty: 50,
                        allow_contract_pool: false,
                    },
                ],
                Vec::new(),
            ),
        );
        orders_ist_b.insert(
            food,
            (
                vec![SellSubmission {
                    sector_id: 0,
                    goods_inv: 4,
                    cash_inv: 4,
                    ask_price_cents: 800,
                    qty: 20,
                    allow_contract_pool: false,
                }],
                vec![BuySubmission {
                    sector_id: 1,
                    goods_inv: 5,
                    cash_inv: 5,
                    bid_price_cents: 1_800,
                    qty: 15,
                    allow_contract_pool: false,
                }],
            ),
        );
        clear_bundled_transport_markets(
            goods,
            &net_ist,
            &mut books_ist,
            orders_ist_b,
            &mut invs_ist,
            &mut pol_rev_ist,
        );
        for (&g, books) in books_ist.iter_mut() {
            release_resting_orders(goods, g, books, &mut invs_ist);
        }
        assert_eq!(
            invs_ist[5].total(food),
            6,
            "西海岸港口投运后精准按西岸 6 份海运力成交 6 单位粮食！"
        );
        assert_eq!(
            invs_ist[1].total(cash),
            0,
            "东海岸内陆海港口未参与西向航线，营收严格为 0！"
        );
        assert!(
            invs_ist[2].total(cash) >= 6 * 500,
            "抢空的西海岸港口独立享受边际瓶颈竞价溢价（实收 {} cents >> 底价 900 cents），不被同区闲置东岸港稀释！",
            invs_ist[2].total(cash)
        );

        // ── Case 3: Full 6-month `Game::with_economy_v1` planetary simulation ──
        let mut game = Game::with_economy_v1(CalendarConfig::default())
            .expect("economy_v1 完整世界初始化必须成功");
        for _ in 0..(30 * 6) {
            game.step_once();
        }
        let snap = game.snapshot();
        assert_eq!(game.counters().monthly_ticks, 6);
        assert!(
            snap.planets[0].total_population > 0,
            "economy_v1 开局水电农商闭环运转 6 个月后人口与经济应健康存续"
        );
    }
}
