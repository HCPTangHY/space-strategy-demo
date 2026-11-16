# Space Strategy

最小桌面原型：Tauri 2 + Rust/hecs + React/TypeScript + Canvas 2D。

## 当前功能

启动桌面应用或打开验收页后，Rust 创建初始世界与模拟时钟（30 天 / 12 月 / 360 天标准历法，默认从星历 `0001.01.01` 暂停开局，无 0 年）。前端默认展示由种子确定性生成的**维诺图行星地表版图**（支持滚轮缩放、拖拽平移、大区染色/自然地形切换、平滑分形海岸线以及与二维轨道视图切换），顶部提供日期栏以及暂停与 `1x ~ 4x` 倍速控制（支持空格键暂停/继续、数字键 `1~4` 切换倍速）。

## 结构

```text
crates/game-core/  独立 Rust 核心，拥有 hecs World
src-tauri/        桌面窗口、状态托管、IPC 命令
src/              React 界面、TypeScript 通信、Canvas 绘制
```

数据流：`GameRuntime (独立模拟线程 + 指令队列 + 视图订阅表 + hecs World) → StreamEvent 分频推送 → Tauri Channel / WebSocket → TypeScript → Canvas / React`。

- `Planet` 是 Rust 星球组件，包含稳定内容 ID、名称、半径。
- `Stockpile`（`BTreeMap<String, i64>` 有序字典仓库）与 `PlanetTiles`（由 `surface.rs` 确定性生成的两级“近海细、深海大”海陆一体维诺网格，地球共 730 个可交互海陆格：383 陆地格 + 268 近海大陆架细格 + 79 远洋深海大格，划分为 12 个陆地大区与 4 个远洋海域）作为独立 ECS 组件挂载在星球实体上。
- `Polity` 是国家主权实体组件（持有中央国库、基础税率与月度财政收支明细）；每个受控陆地片区生成独立的 `Sector`（实控辖区）ECS 实体，政治上服从所属国家，内部独立挂载 `SectorPops`（由可编辑纯数据表 `PopTypeCatalog / PopTypeDef` 驱动的动态 `PopGroup` 列表，采用单一人口口径，不含冗余家属拆分）、`SectorMarket`（本地商品市场占位）以及 `SectorCapital`（民间投资池与分红再投资账本）。
- 每个地块采用统一的紧凑位集标签 `TileTags(u64)`（可自由组合 `land`、`sea`、`coastal`、`hills`、`forest`、`mountains` 等标签，后续矿藏直接追加标签位即可，无需增改结构体字段）与定长 `1 主导产业槽 + 3 次要产业/配套槽`（整个 `Tile` 在 ECS 中保持 `<= 24` 字节）：主槽为空时锁定次槽，须先确立地块主业后方可开发次槽；主槽支持产业改建替换，每个槽位独立记录 `BuildingOwner`（`国有 / 民间私产 / 具名企业 / 预留托拉斯`）。
- 建筑系统支持紧凑标签位集 `BuildingTags(u32)`、地形前置标签 `required_tags`、主次槽规则 `SlotRule`、多职业岗位配比以及无等级固定物理承载容量：
  - `行星首府`（`base:planetary_capital`）：全星球限 1 座，仅限陆地主槽，开局预置在首都格主槽，兼任首都辖区核心据点。
  - `大区核心`（`base:regional_core`）：每个大区限 1 座，仅限陆地主槽。
  - `苹果园`（`base:apple_orchard`）：主次槽通用，放入次槽按 `1x` 次要产业规模运行，放入主槽自动按 `2x` 主导产业规模运行；支持国有建筑挂牌私有化与私产赎买国有化。
  - `商业街`（`base:commercial_street`）：次槽专用，须依托行星首府、大区核心或企业园区主槽兴建，单座固定托管上限 6 座中小私营建筑，集中容纳资本家与文员按股份分红。
  - `企业园区`（`base:corporate_campus`）：主槽专用，同辖区同行业个体私产满 3 座后可批地落成具名企业总部，单座固定托管上限 18 座旗下产业建筑并提供 `+15%` 规模产出加成，腾出商业街孵化额度。
- `Position2d`、`Appearance` 和 `InCombat` 是独立组件；内部实体句柄不传给前端。
- 时间系统采用 `30/12/360` 制，内部仅记录从 `0` 起算的 `1/10 天` 子步数，显示日期由独立的 `CalendarConfig` 换算（支持不同剧本自定义纪年与起始年，跳过 0 年）。
- `1/10 天` 子步仅在世界中存在 `InCombat` 实体时按需展开；平时无战斗时直接按整天推进，零额外子步开销。
- 倍速调度采用“少不上、多不补”：1x（2.0s/天）、2x（1.0s/天）、3x（0.25s/天）、4x（无人工延迟推进，空载设 60 天/秒防失控上限）。暂停时后台线程阻塞休眠，不占 CPU。
- 状态同步彻底取消前端轮询，采用三层按视图分频推送：每日推进仅推轻量的 `StreamEvent::Tick`；打开或切换星球时按需生成并仅下发一次 `StreamEvent::PlanetSurface`（维诺多边形与海岸闭环）；建造建筑或跨月结算时仅下发稀疏的 `StreamEvent::PlanetView`（库存与已占用地块列表，不重复传输静态地图几何）。

## 环境准备

需要 Node.js 22.12+（或当前受支持的更新版本）、npm、稳定版 Rust，以及目标平台的 Tauri 系统依赖。

- Windows：Microsoft C++ Build Tools 与 WebView2。
- Linux：GTK 3、WebKitGTK 4.1 等开发包，以及运行桌面应用所需的图形会话。
- macOS：Xcode Command Line Tools。

完整安装说明：https://v2.tauri.app/start/prerequisites/

Tauri 不只是 Node.js 加 Rust；缺少系统开发库时不能编译桌面端。

## 运行

```sh
npm ci
npm run tauri dev
```

桌面启动命令会自动启动 Vite，请勿另行占用 1420 端口。

```sh
npm run build              # TypeScript 检查与前端构建
cargo test -p game-core     # ECS 核心测试，无需 GTK 图形库
cargo check -p space-strategy # 桌面 Rust 编译检查，需要系统开发库
npm run tauri build        # 构建桌面可执行文件；当前关闭安装包生成
```

## Web 开发验收

正式目标仍为 Tauri 桌面应用。浏览器验收增加一个轻量 HTTP 适配器，复用同一个 `game-core::GameRuntime`，不使用 WASM 或前端假数据。

```sh
cargo run -p web-preview   # 终端一：HTTP 18421 + WebSocket 18422，真实 Rust ECS 状态流
npm run dev               # 终端二：Vite 自动代理 /api 与 /api/ws
```

浏览器访问 `http://127.0.0.1:1420`。前端自动选择 Tauri `Channel` 或 `WebSocket` 持久推送流；连接断开时明确提示错误并提供重连，不回退为假数据。

服务器验收地址：https://files.zhenxia.top/space-strategy/

生产前端由 Nginx 提供，`/space-strategy/api/ws` 通过 WebSocket Upgrade 代理到本机 Rust 进程（同时保留 `/space-strategy/api/state` 供命令行诊断）。部署配置见 `deploy/`。

```sh
systemctl status space-strategy-preview
systemctl stop space-strategy-preview
```

当前服务已启动，未设置开机自启。浏览器联调脚本为 `scripts/check-web.cjs`，需要在测试环境安装 Playwright。

## 当前边界

目前已完成星球内第一阶段政治经济闭环（国家国库、实控辖区本地市场与邻区贸易、7 职业 Pop、公私产权转换、次槽商业街与主槽企业园区），尚未实装多商品工业产业链、跨星球托拉斯与特许状、战斗、存档、模组加载器、WASM 或多人联机。原版星球目前在核心初始化时定义，尚未实现“本体即模组”。

纯 Rust 核心不依赖 Tauri；当前 HTTP 适配仅用于只读开发验收，不是正式多人服务端。多人游戏仍需身份验证、命令校验、可见性过滤、同步和断线重连，并非直接替换通信函数即可。

已提供程序生成的基础窗口图标。安装包、签名和各平台发布配置尚未设置。当前 `bundle.active` 为 `false`。
