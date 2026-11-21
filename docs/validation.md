# 验证记录

## 具名实体与单槽工场双向盈亏结算、国有工场稳岗兜底及私产破产残值处置机制（2026-11-21）

地址：https://files.zhenxia.top/space-strategy/

- **单槽 `Workshop` 独立周转与归属实体双向盈亏分层（`settlement/mod.rs`、`ai/hiring.rs`、`docs/architecture.md`）**：保留全部生产与服务槽位各自独立的 `Workshop` 账本与存货核算，按产权主体分层处理盈亏归宿与裁员决策权——`LocalPrivate`（本地私产）超额纯利注入 `SectorCapital`，出现亏损时由工场自行缩编裁员、折价变卖存货；具名实体（`State` 政权、`Enterprise` 企业、`Trust` 托拉斯）旗下工场的超额纯利上缴归属实体金库，低于单月运营底线的周转缺口与未付工资亦由归属实体金库拨补（计入实体工场净收支，政权国库优先预留核心军政薪饷储备），单槽工场自身不再因短期亏损擅自自动裁员（政权默认维持编制不因单厂亏损裁员，企业与托拉斯在法人金库吃紧时由实体层面统一缩减亏损分厂）。
- **私产破产拆解残值收购与直接清拆双选接口（`rules.rs`、`events.jsonc`、`workshop_ops.rs`、`corporate_ops.rs`、`protocol.rs`、`runtime.rs`）**：仅 `LocalPrivate` 连续 3 个月欠薪资不抵债触发单槽破产处置，新增声明式事件 `base:private_bankruptcy` 与显式控制指令 `acquire_bankrupt_slot` / `resolve_pending_event`——辖区管辖政府或上级政府可按建造成本 `25%` 的破产拆解残值收购（残值注入原民间池、结清工人欠薪并补足首月周转金收归公有），或不予收购直接破产清拆；移除旧版主槽破产强行转为 `0` 人国有空壳的分支，清拆主槽时保留同地块无主槽前置依赖的普通独立次槽继续运行。
- **长协池成本保护、殖民据点财政配平与 20 年长周期回归验证（`ai/pricing.rs`、`01_civic_commercial.jsonc`、`scenario.rs`、`tests.rs`）**：正常工场在商品移动均价低于自身单位成本时暂停向亏本长协池供货，防止破产半价清算单将正常工场拖入长期低价欠薪循环；优化殖民据点人员结构使起步殖民地在长周期运行下保持财政稳健。`RUSTFLAGS="-D warnings" cargo test -p game-core -p web-preview` 全量 24 项单元测试全部通过（0 警告，含国有亏损工场财政兜底零裁员守恒、私产破产残值收购与清拆双路径守恒及殖民星 20 年自动挂机稳态验证）。

## 殖民地长周期经济稳态、盘口滞销解套与民间投资/用工边界修复（2026-11-20）

地址：https://files.zhenxia.top/space-strategy/

- **工场滞销报价、盘口均价回落与居民预算约束修复（`ai/pricing.rs`、`market/commodity_clearing.rs`、`settlement/mod.rs`）**：修正工场单位成本核算分母，按在岗人员正常额定产出与额定原料投入计算 `last_unit_cost_cents`（停工空置时归零），消除临时缺料月份将固定工资除以残次产量造成天价成本锁死的问题；实物工场在库存积压且上月回款低于半数估值时主动向成本线下调挂单价；盘口无现货成交时消除整除四舍五入卡顿，并在零买单零成交且高于初始基准价时允许 `MA` 温和回落；居民口粮急购按家庭实有可用现金约束出价与数量，避免超出钱包承受力的无效报价导致零买单断粮。
- **民间投资上下游核验、用工需求饱和保护与人口增长反馈（`ai/investment.rs`、`ai/hiring.rs`、`actors/household.rs`、`workshop_ops.rs`）**：民间投资池与企业新建工场前校验配方原料市场供给（流量公用设施要求本地具备富余挂单，工业中间品要求存在真实下游消耗或成交需求），并在资金池未满且本地客流不足半数产能时暂缓抢建商业街；工场在盘口存在未售出过剩流量或高库存滞销时暂停盲目扩招；辖区存在较高比例无业缺粮人口时自动放缓自然增长；修复 `migrate_pioneers_if_frontier` 中 `then_some` 先行求值导致的无符号减法下溢问题。
- **殖民地开局配比调平与 6 年长周期回归验收（`01_civic_commercial.jsonc`、`actors/polity.rs`、`scenario.rs`、`tests.rs`）**：将 `殖民据点` 基础水电产出调整为 `14 电力 + 14 工业淡水`，着陆点配置 `殖民据点 + 1x 现代农场` 与精干先遣人口储备；新增 `colony_six_year_sim_keeps_staple_prices_stable_and_avoids_mass_destitution`（连续推进 72 个月自动运行验证主粮价格平稳、产储覆盖需求且无大规模流民赤贫）。`RUSTFLAGS="-D warnings" cargo test -p game-core -p web-preview` 23 项单元测试全部通过（0 警告），静态 musl 版 `web-preview` 部署与公网 Playwright 全链路验收通过，线上服务已重置回 `0001.01.01` 暂停状态。

## 殖民地开局剧本、大区中心候选格、行政力连片框地与先遣据点迁工机制（2026-11-20）

地址：https://files.zhenxia.top/space-strategy/

- **大区中心候选格 `TileTag::CoreSite` 自动选点与外部数据配置（`surface.rs`、`building.rs`、`gamedata/`）**：地表生成与定稿地图加载时，在每个陆地地理大区远离跨区省界缝隙处自动标定 `1~2` 个腹地中心与天然良港候选格 `core_site`，约束行星首府、大区核心与殖民据点选址；`GoodDef` 新增 `non_market` 非盘口流通标记并在 `goods.jsonc` 配置 `base:admin`（行政管理力），在 `01_civic_commercial.jsonc` 配置零水电前置输入、自给基础行政力与本地水电运力的 `base:colonial_outpost`（殖民据点）。
- **政权 `base:admin` 直辖容量核算、无主荒野连片框地与分离省 `Sector` 自动创建（`polity_ops.rs`、`planet.rs`、`settlement/`）**：政权按基础保底额度与在岗核心/据点实际产出实时核算 `admin_used / admin_capacity`（每实控 `1` 格陆地占 `1` 份行政力，邻接 `CoastalSea` 大陆架 `0` 成本自动随迁）；支持单格 `claim_tile` 与大区 `claim_region_frontier` 连片框入无主荒野（`sector_id == u16::MAX`），跨入尚未设省的地理大区或同一地理大区存在多政权实控时，按 `(geo_region_id, owner_polity)` 自动拆分创建分离省 `Sector` 并刷新陆海交通拓扑。
- **跨大区殖民据点开埠与先遣人口守恒迁工（`workshop_ops.rs`）**：在空白新辖区已框入的 `core_site` 建造首座 `殖民据点` 时，自动从同政权人口充裕辖区按人数比例守恒迁入先遣人口及其家庭现金与口粮储备，并同步锁定初始岗位劳动合同，严守全盘货币与实物守恒。
- **双开局剧本（`ScenarioPreset`：`earth` / `colony`）与前端交互落地（`scenario.rs`、`runtime.rs`、`src/App.tsx`、`src/PlanetSurfaceView.tsx`）**：新增 `seed_colony_scenario_world`（新泰拉殖民星：着陆点 `core_site` 与周边共 `9` 格初始立足点、`9/22` 直辖额度，其余陆地为待拓荒无主荒野），保留 `seed_earth_scenario_world` 默认地球开局，支持顶部栏运行期一键切换剧本重开；地图区分渲染已框入政区实控边界、无主荒野暗色地块与 `core_site` 中心选址标记，大区窗口与地块窗口接入连片框地与据点升级改建操作。
- **全量编译、测试与公网端到端验证**：`RUSTFLAGS="-D warnings" cargo test -p game-core -p web-preview` 22 项单元测试全部通过（0 警告，含殖民地开局连片框地、跨区建殖民据点先遣迁工、升级大区核心及多政权同地理大区分离省货币实物守恒验证）；`npm run build`（`tsc --noEmit && vite build`）、静态 musl 版 `web-preview` 部署与公网 Playwright 自动化测试（含剧本切换、`9/22` 行政力框地增长、两级地图缩放回归及 `1280×800` / `640×600` 零横向溢出核验）全部通过，线上服务已重置回地球开局 `0001.01.01` 暂停状态。

## 商业服务流量机制、Pop 周转余钱消费投资、同构多级政权分税与跨国投资准入机制收口（2026-11-18）

地址：https://files.zhenxia.top/space-strategy/

- **商业街并入服务商品供需体系并移除旧私产座数硬锁（`market.rs`、`lib.rs`、`decision.rs`、`settlement.rs`）**：移除“2 座基础私产 / 6 座商业街托管上限”计数硬锁、挂牌私有化免费送商业街特判及国库建商业街归私产特判；为 `BuildingDef` 增加通用前置主槽标签校验 `required_primary_tags`（不追加机制专用容量字段）；新增 `GoodDef` 服务需求、超额转满意与动态资金池容量系数，使商业街作为普通单槽 `Workshop` 产出本地流量服务 `base:service`（零体积、月末清零、按 `1.0` 倍当月需求采购），口粮温饱度与服务满意度分轨独立结算，按本区服务实际产出动态放大 `SectorCapital` 上限，并由 `EVENT_SECTOR_INVEST_COMMERCIAL` 事件驱动民间商业设施涌现、通用行业标签驱动企业园区合并组建。
- **`PopGroup` 周转金储备与富余现金消费/投资闭环（`economy.rs`、`decision.rs`、`settlement.rs`）**：在 `PopTypeDef` 增加家庭应急储备月数 `reserve_months` 及富余现金消费/投资分配千分比；居民留足储备金后，富余现金自动拆分为本地盘口增购服务（转化为当月额外满意度）与公开盘口认购债券/托拉斯股权（无挂牌证券且本区民间池未满时注入 `SectorCapital`），同时移除 `Workshop` 初始成本对 `GENESIS_APPLE_PRICE_CENTS` 的残余引用。
- **同构多级 `Polity` 树、源头分税直征与跨国直接投资/强制没收原语（`economy.rs`、`lib.rs`、`settlement.rs`、`rules.rs`）**：`Polity` 新增 `parent_polity` 与 `upward_tax_share_permille`，`Sector` 新增 `admin_polity` 与 `foreign_investment_rights`；结算层解除单一国库限制，在扣税源头按上下级分税比例直征入库，并严格保护“一级政府、一级产权”；新增 `create_polity`、`set_sector_admin_polity`、`grant_investment_right`、`transfer_polity_funds`、`build_on_tile_as` 与 `expropriate_slot`（强制无偿没收并触发 `EVENT_FORCED_EXPROPRIATION` 扣减信用评级），使国内跨级直投央企、跨区公产、地方政府 AI 自建及外国凭条约直建工场共用同一套守恒结算算式。
- **架构文档与测试验证**：重写 `docs/architecture.md` 并附完整“机制—数据”划分总表；`RUSTFLAGS="-D warnings" cargo test -p game-core -p web-preview` 17 项单元测试全部通过（0 警告），`npm run build`（`tsc --noEmit && vite build`）构建通过。

## 尺度无关 N×N 矩阵、声明式事件与 if-else 规则执行器及纯数据目录解耦（2026-11-17）

地址：https://files.zhenxia.top/space-strategy/

- **抽象尺度无关 `N × N` 节点矩阵与参数化空间撮合机制（`market.rs`）**：将原先绑死行星陆海常量的 `LogisticsMatrix` 升级为尺度无关的 `NodeMatrix`（支持通过 `NodeNetworkSpec` 配置直连边、`HubCliqueRule` 共享枢纽团与可选 Floyd 多跳中转，并提供零和守恒的通用成对计算算子 `compute_pairwise_flows`）；将市场交易制度参数提取为挂载在 `GoodDef` 上的 `MarketClearingRule`（`contract_pool_percent`、`PricingMode`、`ma_inertia_weight`），由 `clear_spatial_good_market` 同时兼容大区网络与星球网络。
- **新增声明式事件与 `if-else +-` 规则执行器（`rules.rs`、`settlement.rs`、`lib.rs`）**：实现纯数据可序列化的条件树 `RuleCond`（含双变量比较 `CompareVars`）、类 P 社 `if-else +-` 权重打分器 `AiWeightSpec`、守恒效果原语 `RuleEffect`、`EventCatalog` 与待决事件队列 `PendingEvent`。双边供货契约到期（`EVENT_BILATERAL_CONTRACT_DUE`）与标准付息债权兑付（`EVENT_COUPON_CLAIM_DUE`、`issue_standardized_claim`）统一转为触发事件：AI 主体按 `if-else +-` 权重自动选择履约或违约选项，玩家控制主体（`controlled_actor`）排入 `PendingEvent` 队列由 `resolve_pending_event` 手动抉择。
- **消除机制层对具体商品、建筑与职业名称的硬编码耦合（`lib.rs`、`decision.rs`、`settlement.rs`、`economy.rs`）**：将 `BUILTIN_BUILDINGS` 升级为动态可注册的 `BuildingCatalog`（`Tile` 保持 `<= 24` 字节紧凑结构）；在 `PopTypeDef` 新增 `redemption_tier` 数据驱动清偿顺位（移除 `decision.rs` 对 `POP_SOLDIER`/`POP_BUREAUCRAT` 的硬编码）；在 `GoodDef` 新增 `staple_demand_per_100`、`strategic_depot_target` 与 `coupon_permille_per_month`，使 `decision.rs` 与 `settlement.rs` 彻底解耦 `GOOD_APPLE`，可在完全自定义的商品、建筑与职业数据表下独立运行并保持严格货币守恒。
- **架构文档同步与编译测试验证**：更新 `docs/architecture.md`，`RUSTFLAGS="-D warnings" cargo test -p game-core -p web-preview` 16 项单元测试全部通过（0 警告，含 4 星球跃迁门网络多跳撮合、无 `base:apple` 自定义多商品产业链守恒、主权公债票息兑付及玩家手动违约事件测试），`npm run build`（`tsc --noEmit && vite build`）构建通过。

## 五类主体、三套对称金库、自动多商品盘口与三条交互通道机制落地（2026-11-17）

地址：https://files.zhenxia.top/space-strategy/

- **更新 `docs/architecture.md` 双维度架构基线**：补充横向三层职责划分（Rust 代码机制 / 静态数据表 / 声明式规则脉冲）、五类 AI 决策主体（`PopGroup`、单槽 `Workshop`、`SectorCapital` 本地民间池、`Enterprise`/`Trust`、政权 `Polity`）、五项底层代码机制与三条实体交互通道规范。
- **移除无建筑自给“个体民”并收敛为 7 类现代职业（`economy.rs`、`lib.rs`、`settlement.rs`）**：默认职业表移除 `POP_SUBSISTENCE`，所有实物商品须由具体建筑槽位生产；默认人口蓄水池改为 `劳工`（未签约人员统一归入 `劳工 · 待业`，储蓄耗尽且缺粮时沿 `demote_to` 降入底层 `流民`），同时保留自定义模组配置 `subsistence_apple_per_100 > 0` 的数据表兼容能力。
- **三套对称金库与 `SectorCapital` 闭环（`economy.rs`、`decision.rs`、`settlement.rs`）**：为具名企业 `Enterprise` 与星际托拉斯 `Trust` 建立各自独立的法人金库 `vault: Inventory`，与辖区民间池 `SectorCapital.vault` 彻底分离；单槽 `Workshop` 超额利润按产权分别归集至 `Polity.vault`、`Enterprise.vault`/`Trust.vault`、`SectorCapital.vault`；`SectorCapital` 新增无商业街基础免托管额度（2 座）、满足条件时自动出资兴建 `商业街`、超出池顶上限部分 100% 溢出分红给当地 `资本家` 防死水锁钱，以及资金充裕时跨区前往未开发辖区投建首座开荒私营厂并落户新辖区名下。
- **自动多商品盘口与三条交互通道权限底座（`market.rs`、`economy.rs`、`lib.rs`）**：`SectorMarket` 遍历 `GoodCatalog` 自动为所有非货币商品建立 `SectorOrderBook`，并根据 `TransportChannel` 区分本地限定、电讯零运费无限运力（债券与 `Trust` 股权）及陆海物理运力；建筑支持通用 `monthly_inputs` → `monthly_output` 配方生产与原料自动挂买单；实装通道一产权核验（`check_channel1_slot_authority`，禁止政权绕过赎买直接改写私营/企业槽位编制）与通道三具名主体双边供货契约（`CounterpartyId` / `BilateralSupplyContract`，硬性拦截 `LocalPrivate`/`SectorCapital` 进入通道三，违约自动扣减 `credit_rating`）及 `Trust` 股权按全员 `Inventory` 持仓比例自动分红。
- **编译与测试验证**：`cargo test -p game-core -p web-preview` 15 项单元测试全部通过（0 警告，含三金库隔离、满池溢出分红、跨区开荒落户、自动建商业街、动态注册托拉斯股权盘口与分红守恒、通道一/通道三权限拦截及违约降级验证），`npm run build`（`tsc --noEmit && vite build`）构建通过。

## 自主实体决策层与客观结算层分离、两步跨区边际定价及优先兑付/破产闭环（2026-11-16）

地址：https://files.zhenxia.top/space-strategy/

- **确立架构规范 `docs/architecture.md` 并拆分 `decision.rs` / `settlement.rs`**：将凡涉及两个自主实体交互的操作（劳资期中买断裁员、`1/24` 到期重议、盘口挂单报价、履约/暂缓兑付开关、兑付优先级排序、民间资本投资）全部归入 `crates/game-core/src/decision.rs` 及统一槽位控制接口（`set_slot_target_jobs`、`set_slot_wage_redemption`）；`settlement.rs` 仅保留纯物理产出、中立盘口清算、优先债权扣划、消费腐损与破产退出。
- **跨辖区两步边际出厂价（Spatial Call Auction，`market.rs`）**：商品盘口改为第一步按跨区到岸价差配对数量与运力，第二步按各产地成交卖单边际要价 `max_filled_ask`（及供不应求时的买方最低净出价 `min_filled_netback`）统一结算该产地所有成交卖单的出厂离岸价；消除破产半价清算单单独贱卖及单点低价卖单误触发全区长协池减半的问题。
- **解雇补偿、兑付顺位与欠薪债权破产闭环**：有偿付能力的雇主期中主动减员须支付 1 个月合同工资作为补偿；多槽位付款主体（如国库）按 `军人` → 核心行政工程 → 文教普通岗位顺位兑付；工场欠薪违约自动记入优先于股东分红的 `UnpaidWageClaim`，私营工场连续 3 个月欠薪未清则通过盘口折价变现存货并完成破产退出，全程严格保持现金与实物守恒。
- **编译与测试验证**：`cargo test -p game-core -p web-preview` 14 项单元测试全部通过（0 警告），`npm run build`（`tsc --noEmit && vite build`）构建通过。

## 职业独立待业、破产流民滑落、长协现货解锁、欠薪工场裁员与通用投资核算（2026-11-15）

地址：https://files.zhenxia.top/space-strategy/

- **各职业独立待业 + 破产底层 `流民（base:vagrant）`**：移除笼统的通用失业职业，直接复用 `PopGroup` 已有的 `employer = BuildingOwner::Unowned` 表示各职业独立的 `XX · 待业`（保留各自储蓄与本职业优先应聘权，不稀释高技能职业存款，也不拉低无关高技能职业工资）；在 `PopTypeDef` 数据表中新增 `is_vagrant_pool`、`demote_to` 降级链与 `expected_sol` 生活期望档位，仅当待业群体储蓄耗尽且口粮不足时才按 `demote_to` 逐级滑落直至 `流民`。
- **长协锁价与现货驱动 `MA` 解锁**：保留长协按 `MA` 锁价交割（常规占比调为 `35%`，本地最低卖单价低于 `80% MA` 时买方减半长协提货转向便宜现货），但 `ma_price_cents` 与 `ma_wage_cents` 改为仅由现货盘口边际成交价（及无成交时的盘口最优报价）更新，消除旧 `MA` 自我锚定导致的滞销价格锁死。
- **修复僵尸欠薪工场、原地续签与自动投资去硬编码**：`Workshop` 记录上月真实回款 `last_revenue_cents` 与欠薪标记 `had_wage_arrears`，预期产出按仓库物理剩余库容封顶，欠薪或满仓无回款工场月初立即按实有现金裁员回原职业待业；有偿付能力且仍需人手的雇主在 `1/24` 合同到期时原地按 `MA` 续签；移除民间自动投资中写死“苹果园”的逻辑，改为遍历可私建建筑表执行通用盈亏回报率（静态回本 `<= 20` 个月）、本地未严重滞销以及预留 6 个月商业街/企业园区受薪人员工资三重核算。
- **前端展示与全链路验证**：辖区面板区分显示 `待业 N · 流民 M` 以及真实招工缺口 `缺工 N` 与因滞销缩编的 `停招空置 N`。`cargo test -p game-core -p web-preview` 13 项单元测试全部通过（0 警告），`npm run build`、musl 版 `web-preview` 部署与公网 Playwright 端到端测试全部通过，线上服务已重置回 `0001.01.01` 暂停状态。

## 持久劳动合同、雇主产权分组、独立失业职业与本地劳动力盘口（2026-11-15）

地址：https://files.zhenxia.top/space-strategy/

- `economy.rs` 与 `lib.rs`：在 `PopTypeCatalog` 中新增独立职业 `base:unemployed`（失业者），为每组 `PopGroup` 分配稳定的 `PopId` 并绑定雇主产权 `employer: BuildingOwner`（按 `(职业, 族群, 雇主产权)` 唯一划分），在建筑槽上维护持久劳动合同 `StaffContract { pop_id, count, monthly_bill_cents }`。挂牌私有化、赎买国有化及组建企业园区并购私产时，在岗员工合同与按比例折算的家庭储蓄、存粮同步迁入新东家名下的对应 `PopGroup`；拆除或改建时释放入失业者池。
- `market.rs` 与 `settlement.rs`：新增按辖区、按职业隔离的本地劳动力盘口（`RoleWageBook` + `clear_local_labor_book`，70% MA 续签/长协池 + 30% 集合竞价现货盘口）。在岗期间工资合同锁定并直接向目标 `PopId` 发放，跨雇主与跨族群之间互不平均；每月按 `1/24` 滚动到期重签，物价下跌不会下调名义工资，工场或商业中枢现金不足且亏损时按可承受薪酬缩编裁员进 `失业者`。
- 前端展示辖区失业人数、各职业本地工时行情（百人月薪）及按雇主区分的 Pop 合同薪资，并收紧窄屏顶栏间距。
- `cargo test -p game-core -p web-preview` 12 项测试全部通过（0 警告，含雇主隔离、廉价苹果下学者名义工资刚性、私有化随迁合同及亏损裁员入失业池测试），`npm run build`、musl 版 `web-preview` 部署与公网 Playwright 验收通过，线上服务已重置回 `0001.01.01` 暂停状态。

## 统一仓库、实物货币与跨区限价盘口第一版（2026-11-14）

地址：https://files.zhenxia.top/space-strategy/

- 新增 `market.rs`：`GoodCatalog` 数据表（`base:cash` 为零体积普通资源，按“分”存储）、带容量上限与挂单冻结的 `Inventory`、各辖区 `SectorOrderBook`，以及“70% 公开长协（MA 价，且不高于最便宜到岸价）+ 30% 限价盘口”的 `12×12` 同台到岸价撮合（本地运费为 0），长协池补齐买方现金预冻结原子交割。
- 新增 `settlement.rs` 月度结算：工场产出入自有仓库 → 央行按档位增发入国库 → 全员按同一规则同时挂单撮合 → 工场按数据表工资支付 → 超出两个月工资的现金上缴所有者（国有入国库，私产/企业入辖区资本池并按 65% 分红给股东 Pop）→ 人头税 → Pop 从自家粮仓消费 → 腐损。未成交挂单在结算后退回，收盘深度保留给界面显示。
- 修正 6 项底层结构与定价错误：`SectorPops` 解除对 `pop_type` 数组下标的硬编码，支持 `(pop_type, culture_id)` 任意多族群/文化并存；晋升与降级保持原族群并按人数比例带走现金与存粮；新建生产工场配备首月周转金；工场单位成本按正常名义产出核算（修复满仓时除以腐损空位虚高至 `12.00` 的问题），并改为按仓库水位分段报价（低库存报 `100% MA`，满仓折价清库存）。
- 新增国库、辖区官仓（容量随首府/核心增加）、央行档位（紧缩/温和/宽松）与官仓政策（封存/收储/平粜）命令，Web 与 Tauri 接口同步。
- `cargo test -p game-core` 11 项通过（含现金守恒、跨区最低到岸价、多族群并存与晋升储蓄迁移及满仓折价清库测试），`npm run build`、musl 版 `web-preview` 部署及公网 Playwright 验收全部通过，服务已重置回 `0001.01.01` 暂停状态。

## 第一阶段政治经济内核：中央国库、实控辖区本地市场、7 职业 Pop 与商业街/企业园区（2026-11-14）

地址：https://files.zhenxia.top/space-strategy/

- **国家 `Polity` 与实控辖区 `Sector` 三层解耦**：新增 `Polity`（中央国库、税率与月度税入/国企净利/行政俸禄账本）与 12 个陆地 `Sector` ECS 实体（`Tile` 仍保持 `<= 24` 字节）。每个辖区独立核算本地苹果供需定价与相邻辖区陆海运费调剂。
- **纯数据表驱动 `PopTypeDef / PopTypeCatalog` 与单一人口口径**：移除写死的 `PopProfession` 枚举与冗余的“抚养家属”字段，所有 Pop 阶层（薪资、自给产出、默认蓄水池标识、分红资格、晋升来源链）均由可序列化/可编辑的数据表 `PopTypeCatalog` 定义，单一 `count` 直接对应人口与人力口径。
- **`1 主导产业 + 3 次要产业/配套` 槽位规则与无等级实体产权演进**：主槽为空时锁定次槽，支持拆除与主槽产业改建；`苹果园` 放入主槽按 `2x` 主导规模运行，放入次槽按 `1x` 运行；`商业街`（次槽，依托首府/核心/园区主槽）固定托管 6 座中小私产并集中容纳资本家按股份分红（35% 注入辖区民间投资池）；同业个体私产满 3 座后可批地设立主槽 `企业园区`（固定托管 18 座、`+15%` 产出加成），支持挂牌私有化、赎买国有化及辖区“民间次槽拿地”开关。
- `cargo test -p game-core`（7 项单元测试全部通过，0 警告）、`npm run build`、静态 musl 版 `web-preview` 部署及公网 Playwright 端到端测试（验证国库与 7 阶层明细、空陆格次槽前置锁定与主槽 `2x` 建造、挂牌私有化、跨月分红再投资、满 3 座私产批建具名企业园区、1280×800 与 640×600 零溢出截图及断线重连）全部通过，线上服务已重置回 `0001.01.01` 暂停状态。

## 大区单一人力池、月度自然增长与按宜居度排队招工（2026-11-14）

地址：https://files.zhenxia.top/space-strategy/

- **大区人口组件 `PlanetPopulation` 与月度自然增长**：每个陆地大区按辖下陆地格宜居度总和与首都加成生成确定性初始人力池（远洋海域常住人口为 0），每月 1 日结算当月在岗产出后自动按宜居承载上限与核心据点加成增长人口。
- **核心优先 + 地块宜居度排队招工**：同一大区内所有已建建筑共享本区人力池；招工时 `行星首府`（需 200 人，满编 `+30/月`）与 `大区核心`（需 150 人，满编 `+20/月`）归属核心优先档率先 100% 招满，普通生产档 `苹果园`（需 100 人，满编 `+10/月`）严格按所在小格的宜居度（如沿海平原 `115%` > 丘陵 `68%` > 山脉 `15%` > 近海 `0%`）从高到低依次填人，缺人槽位按在岗比例折算实际月产。
- **界面与地图缺工可视化**：星球总览与大区卡片实时展示总人口、月增长、在岗人数与富余人力/缺工缺口；地块卡片展示宜居度百分比、每个槽位的在岗人数与折算后实际产出；地图上缺工槽位以暖橙色高亮标记。
- `cargo test -p game-core`（7 项单元测试，含首都大区超建后核心满编保护、近海低宜居果园排尾停工、跨月人口增长自动补员测试）与公网 Playwright 端到端测试（验证大区超建触发缺工与部分产出 `+64/月`、跨大区建核心、`4x` 跨月结算与人口实时增长、多尺寸截图与断线重连）全部通过，服务已重置回 `0001.01.01` 暂停状态。

## 建筑标签与两级核心唯一性规则（全星唯一行星首府 + 每大区唯一大区核心）（2026-11-14）

地址：https://files.zhenxia.top/space-strategy/

- **建筑标签位集 `BuildingTags(u32)` 与分级唯一性校验**：在 `BuildingDef` 中引入可组合建筑标签与 `unique_planet_tags` / `unique_region_tags` / `unique_tile_tags` 位掩码校验，零堆分配且不增加 `Tile`（仍保持 `<= 24` 字节）内存体积。
- **两级核心体系落地**：
  - `行星首府`（`base:planetary_capital`，`+3 苹果/月`）：仅限陆地主槽，携带 `planetary_capital` 与 `regional_core` 双标签；地球开局自动生成在起始首都格主槽（同格次槽 1 预置 1 座 `苹果园`），全星球唯一，且自动兼任首都所在大区的核心据点。
  - `大区核心`（`base:regional_core`，`+2 苹果/月`）：仅限陆地主槽，携带 `regional_core` 标签，每个陆地大区限建 1 座（首都大区因已有行星首府不再重复建造，远洋海域无陆地格自动禁用）。
- **界面与地图三层图标区分**：左上标题栏支持一键跳回行星首府格，大区面板显示本区核心据点状态并可点击定位；地图 Canvas 对行星首府（暖金外环菱形星标）、大区核心（亮白方形堡垒）与普通生产设施（青灰圆标 + 次槽卫星点）作清晰三级区分。
- `cargo test -p game-core`（6 项单元测试）与公网 Playwright 端到端测试（校验全星唯一行星首府、首都大区核心互斥、其他陆地大区 `+1 大区核心` 建后自动锁定、单格陆地主槽建核心与海格禁用拦截、`4x` 跨月结算 `+10/月`、多尺寸截图与断线重连）全部通过，线上服务已重置回 `0001.01.01` 暂停状态。

## 地块可组合 Tag 系统、“1 主 + 3 次”定长建筑槽与大区/单格双入口建造（2026-11-13）

地址：https://files.zhenxia.top/space-strategy/

- **可组合 `TileTags(u64)` 标签系统**：移除原先互斥写死的 `TerrainKind` 与独立 `coastal` 字段，所有地理与地貌属性统一为 8 字节位集 `TileTags(u64)`（序列化为可读标签数组，如 `["land", "coastal", "hills", "forest"]`）；前端底色与山峰/丘陵/树林符号均按标签动态叠加绘制，地球陆地大区收敛为 `12` 个（外加 `4` 个远洋海域）。
- **紧凑定长 `1 主 + 3 次` 建筑槽与槽位规则**：每个 `Tile` 内联存储 `TileSlots`（1 个主槽 + 3 个次槽，采用 `NonZeroU16` 紧凑建筑编号，单格总内存 `<= 24` 字节，零堆分配）。建筑表支持 `SlotRule`（`PrimaryOnly` / `SecondaryOnly` / `Any`）：内置 `城市中心`（仅限主槽，`+2 苹果/月`）与 `苹果园`（主次槽通用，`+1 苹果/月`）。
- **大区 `+1` 与单格选槽双入口**：支持在大区快捷建设面板直接点击 `+1`（后端自动按地形宜居度与空槽数挑出辖区内最优地块落子，统计容量实时来自辖下真实小格），也支持点击地图任意海陆小格在 `1 主 + 3 次` 具体槽位中手动放置。
- `cargo test -p game-core`（6 项单元测试）与公网 Playwright 端到端测试（校验可组合多标签、12+4 大区、大区 `+1` 落子、海格主槽建城市中心 + 次槽建苹果园、4x 跨月结算 `+5/月`、多尺寸截图与断线重连）全部通过，服务已重置回 `0001.01.01` 暂停状态。

## 维诺图行星地表与分级海域网格（1,000 格初筛 · 近海细深海大）（2026-11-13）

地址：https://files.zhenxia.top/space-strategy/

- `crates/game-core/src/surface.rs` 将初筛六角扰动网格调整为约 `1,000` 格（地球实测初筛 `948` 格），实现清爽的两级“近海细、深海大”重排（沿岸浅海细格 + 三轮质心松弛的远洋深海大格，合并消减 `442` 个深海格，最终地球保留 `730` 个可交互地块：`383` 个陆地格、`268` 个近海大陆架细格、`79` 个远洋深海大格），并提取单条闭合深浅海分界环。
- 大区与海域划分：陆地划分为 `18` 个地理大区，近海大陆架（`CoastalSea`）自动并入相邻沿岸陆地大区管辖，远洋深海（`DeepOcean`）独立聚类为 `4` 个远洋海域。
- 前端 `src/coastline.ts` 与 `src/PlanetSurfaceView.tsx` 去掉多余等深线圈和海面虚线干扰，仅保留浅海、深海两级配色：通过 Chaikin + `curveBasisClosed` 平滑单条大陆架外缘曲线并做轻度羽化过渡，同时对海格公共边施加微幅对称二次贝塞尔平滑，使画面干净利落。
- `cargo test -p game-core`（5 项单元测试）与公网 Playwright 自动化测试（含近海地块建造叠加 `+2/月`、跨月结算、多尺寸截图核验与断线重连）全部通过。验收后已重启服务重置回 `0001.01.01` 暂停状态。

## 星球仓库、地表地块与苹果园月度产出（2026-11-13）

地址：https://files.zhenxia.top/space-strategy/

- `game-core` 为星球实体新增 `Stockpile`（`BTreeMap<String, i64>` 有序字典）和 `PlanetTiles`（6 个 `Tile`，每格容纳 `Option<Building>`）组件；地球默认第 1 格预置“苹果园”（每月 `+1` 苹果），其余 5 格为空。
- 每满 30 天（每月 1 日）自动执行星球建筑月度产出结算，并仅在跨月结算或地块建造时向关注该星球的订阅者推送 `StreamEvent::PlanetView`，日常推进仍只发送轻量 `Tick` 帧。
- Tauri 桌面端（`build_on_tile`）与 Web 验收端（WebSocket `build_on_tile` 及 HTTP `/api/build`）统一接入地块建造与越界/重复建造校验。
- `cargo test -p game-core`：5 项单元测试通过（覆盖初始库存与地块、点击空地块叠加产出、重复建造拦截、跨月与跨年累计产出、订阅流即时推送）。
- 公网 Playwright 自动化测试通过：验证初始 `+1/月`、点击 `#02` 空地块建造第 2 座苹果园变为 `+2/月`、`4x` 跨月结算后苹果库存自动增加、`Tick` 帧不携带星球数据、窄屏无横向溢出及断线重连。验收后已重启服务重置回 `0001.01.01` 初始状态。

## 分频流式推送与视图订阅升级（2026-11-13）

地址：https://files.zhenxia.top/space-strategy/

- 彻底移除前端 HTTP 轮询，升级为按视图订阅的分频主动推送（`StreamEvent::Init`、`StreamEvent::Tick`、`StreamEvent::PlanetView`）。
- 每次日期推进仅广播轻量的 `TimeSnapshot`（不序列化全银河星球列表）；星球数据仅按客户端订阅的 `planet_id` 下发。
- Tauri 桌面端接入 `tauri::ipc::Channel<StreamEvent>`，Web 验收端接入原生 `WebSocket`（`tungstenite`，含 Origin 校验、帧大小上限与 Nginx Upgrade 反代）。
- `cargo test -p game-core`：5 项测试全部通过（含订阅者连续逐日帧验证 `[1, 2, 3, 4]` 无跳日）。
- 通过公网 HTTPS 执行 `scripts/check-web.cjs`：验证前端零 HTTP 轮询请求、`3x` 下逐日严格连续 `+1` 推送、`Tick` 帧不含星球数据、窄屏无横向溢出，以及 WebSocket 断线报错与重连恢复。验收后已重启服务使初始状态回到 `0001.01.01` 暂停状态。

## 时间系统与倍速推进更新（2026-11-13）

地址：https://files.zhenxia.top/space-strategy/

- `game-core` 实现 30 天 / 12 月 / 360 天标准历法，默认从星历 `0001.01.01` 暂停开局，禁止 0 年并支持负数起始年跨向 1 年。
- 实现 `InCombat` 按需 `1/10 天` 子步：无战斗时直接按整天推进，零额外子步开销；有战斗时在 `1x~3x` 按子步推进、战斗中途结束自动对齐当日剩余时间，`4x` 按整日批量结算战斗子步。
- 实现 `GameRuntime` 独立模拟线程 + 指令通道 + `Arc<GameSnapshot>` 快照共享，采用“少不上、多不补”调度（1x=2.0s/天、2x=1.0s/天、3x=0.25s/天、4x 无人工等待并附 60 天/秒空载防失控上限），暂停时线程零 CPU 阻塞休眠。
- `cargo test -p game-core`：5 项测试全部通过。
- `npm run build`：TypeScript 检查与 Vite 构建通过，静态 musl 版 `web-preview` 构建并部署到本机。
- 通过公网 HTTPS 执行 `scripts/check-web.cjs`：验证状态读取、非法倍速参数拦截、`4x` 实时日期推进、空格键暂停冻结与恢复、窄屏无横向溢出及错误重试恢复。验收后已重启服务使初始日期回到 `0001.01.01` 暂停状态。

## Web 实时验收部署（2026-11-07 后续更新）

地址：https://files.zhenxia.top/space-strategy/

- 新增 `web-preview` Rust HTTP 适配器，持有真实 `game-core::Game`，每次请求查询 ECS。
- 远程构建前端和静态链接的 Linux musl 后端，再部署到本机；本机无新增构建依赖。
- Rust 核心 3 项测试、TypeScript 检查和前端构建均通过。
- 后端由 `space-strategy-preview.service` 管理，监听 `127.0.0.1:18421`；Nginx 仅代理只读接口并提供 `dist`。
- 从远程机器通过公网 HTTPS 执行 `scripts/check-web.cjs`：真实 HTTP 数据、POST 被拒绝、星球显示、窄屏布局、错误提示和重试恢复均通过。
- 成功路径没有 IPC 替身或静态快照。错误分支测试主动拦截为 HTTP 503，然后恢复真实接口。
- 服务已启动，未设置开机自启。Tauri 桌面端编译限制仍未解除。

## 最初桌面原型验证

日期：2026-11-07。

源码目录：`/www/wwwroot/space-strategy`。

构建和浏览器验证均在远程测试机 `/tmp/space-strategy-check` 中执行。本机未安装 node_modules、Rust 工具链或桌面系统依赖。

## 已通过

- `cargo test -p game-core`：3 项测试通过。覆盖初始星球、组件修改后的快照、序列化稳定性。
- `npm run build`：TypeScript 检查及 Vite 生产构建通过。
- `cargo run -p game-core --example snapshot`：成功导出实际 Rust 世界快照。
- Playwright：普通浏览器显示缺少 Tauri 后端的错误，重新读取仍正确显示错误。
- Playwright：用上述 Rust 快照替代测试页中的 IPC 返回值，验证 1280×800 与 640×600 下的星球绘制、页面无横向溢出、无 JavaScript 未捕获异常。
- 已检查两个尺寸的截图。

浏览器渲染测试使用测试专用 IPC 替身。项目源码没有浏览器假数据回退；截图不代表 Tauri 桌面端已经运行。

## 未完成

`cargo check -p space-strategy` 在编译系统依赖时失败：测试机缺少 `glib-2.0` 开发库。Tauri 环境检查同时报告缺少 WebKitGTK 4.1 等依赖。

因此尚未验证：

- Tauri 应用本身完整编译。
- 真实桌面窗口中 Rust → IPC → 前端的数据联调。
- Windows/macOS/Linux 安装包和分发。

后续应在具备 Tauri 系统依赖及图形会话的机器上执行 `npm ci`、`npm run tauri dev` 和 `npm run tauri build`。
