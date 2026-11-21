//! Background Simulation Worker Thread & Pub/Sub Stream Shell (`后台模拟线程与流式订阅外壳`).

use crate::building::TileSlotKey;
use crate::calendar::GameSpeed;
use crate::protocol::{
    GameSnapshot, PlanetSnapshot, PolicyChange, ScenarioPreset, StreamEvent, TimeSnapshot,
};
use crate::surface::{self, PlanetSurfaceSnapshot};
use crate::Game;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Subscriber {
    id: u64,
    tx: mpsc::SyncSender<StreamEvent>,
    watched_planet: Option<String>,
}

enum RuntimeCommand {
    SetSpeed(GameSpeed, Option<mpsc::SyncSender<Arc<GameSnapshot>>>),
    BuildOnTile {
        planet_id: String,
        tile_index: usize,
        building_id: String,
        slot: Option<TileSlotKey>,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    BuildInRegion {
        planet_id: String,
        region_id: u16,
        building_id: String,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    DemolishOnTile {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    PrivatizeSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    NationalizeSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    AcquireBankruptSlot {
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        buyer_polity: Option<crate::economy::PolityId>,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    ResolvePendingEvent {
        instance_id: u64,
        option_index: usize,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<Arc<GameSnapshot>, String>>>,
    },
    TogglePrivateLandUse {
        planet_id: String,
        sector_id: u16,
        allow: bool,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    SetPolicy {
        planet_id: String,
        change: PolicyChange,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    Subscribe {
        id: u64,
        watched_planet: Option<String>,
        tx: mpsc::SyncSender<StreamEvent>,
    },
    WatchPlanet {
        id: u64,
        planet_id: Option<String>,
    },
    GetPlanetSurface {
        planet_id: String,
        reply: mpsc::SyncSender<Option<PlanetSurfaceSnapshot>>,
    },
    SetEarthGridPreset {
        dense: bool,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    SetScenarioPreset {
        scenario: ScenarioPreset,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    ClaimTile {
        planet_id: String,
        tile_index: usize,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    ClaimRegionFrontier {
        planet_id: String,
        region_id: u16,
        max_count: usize,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<PlanetSnapshot, String>>>,
    },
    ReloadData {
        restart_world: bool,
        origin_sub_id: Option<u64>,
        reply: Option<mpsc::SyncSender<Result<String, String>>>,
    },
    Unsubscribe {
        id: u64,
    },
    Shutdown,
}

const STREAM_BUFFER_CAPACITY: usize = 128;

pub struct GameRuntime {
    snapshot: Arc<Mutex<Arc<GameSnapshot>>>,
    cmd_tx: mpsc::Sender<RuntimeCommand>,
    next_sub_id: AtomicU64,
    worker: Option<JoinHandle<()>>,
}

impl Default for GameRuntime {
    fn default() -> Self {
        Self::new(Game::default())
    }
}

impl GameRuntime {
    pub fn new(mut game: Game) -> Self {
        let initial = Arc::new(game.snapshot());
        let snapshot = Arc::new(Mutex::new(initial));
        let worker_snapshot = Arc::clone(&snapshot);
        let (cmd_tx, cmd_rx) = mpsc::channel::<RuntimeCommand>();

        let worker = thread::Builder::new()
            .name("space-strategy-sim".into())
            .spawn(move || {
                let mut last_step = Instant::now();
                let mut subscribers: Vec<Subscriber> = Vec::new();

                let publish_snapshot = |game: &Game| -> Arc<GameSnapshot> {
                    let next = Arc::new(game.snapshot());
                    if let Ok(mut slot) = worker_snapshot.lock() {
                        *slot = Arc::clone(&next);
                    }
                    next
                };

                let broadcast_tick = |subscribers: &mut Vec<Subscriber>, time: TimeSnapshot| {
                    subscribers.retain(|sub| {
                        match sub.tx.try_send(StreamEvent::Tick { time: time.clone() }) {
                            Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
                            Err(mpsc::TrySendError::Disconnected(_)) => false,
                        }
                    });
                };

                let broadcast_watched_planets =
                    |subscribers: &mut Vec<Subscriber>, game: &Game, filter_id: Option<&str>| {
                        subscribers.retain(|sub| {
                            let Some(watched_id) = sub.watched_planet.as_deref() else {
                                return true;
                            };
                            if let Some(only_id) = filter_id {
                                if watched_id != only_id {
                                    return true;
                                }
                            }
                            let event = StreamEvent::PlanetView {
                                planet: game.planet_by_id(watched_id),
                            };
                            match sub.tx.try_send(event) {
                                Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
                                Err(mpsc::TrySendError::Disconnected(_)) => false,
                            }
                        });
                    };

                let notify_mutation_outcome =
                    |result: &Result<PlanetSnapshot, String>,
                     planet_id: &str,
                     origin_sub_id: Option<u64>,
                     game: &Game,
                     subscribers: &mut Vec<Subscriber>| {
                        match result {
                            Ok(_) => {
                                publish_snapshot(game);
                                broadcast_watched_planets(subscribers, game, Some(planet_id));
                            }
                            Err(err) => {
                                if let Some(target_sub) = origin_sub_id {
                                    for sub in subscribers.iter() {
                                        if sub.id == target_sub {
                                            let _ = sub.tx.try_send(StreamEvent::CommandError {
                                                message: err.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    };

                let handle_cmd = |cmd: RuntimeCommand,
                                  game: &mut Game,
                                  last_step: &mut Instant,
                                  subscribers: &mut Vec<Subscriber>|
                 -> bool {
                    match cmd {
                        RuntimeCommand::SetSpeed(next_speed, reply) => {
                            let was_paused = game.speed() == GameSpeed::Paused;
                            game.set_speed(next_speed);
                            if was_paused && next_speed != GameSpeed::Paused {
                                *last_step = Instant::now();
                            }
                            let snap = publish_snapshot(game);
                            broadcast_tick(subscribers, snap.time.clone());
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(snap);
                            }
                            true
                        }
                        RuntimeCommand::BuildOnTile {
                            planet_id,
                            tile_index,
                            building_id,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result =
                                game.build_on_tile(&planet_id, tile_index, &building_id, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::BuildInRegion {
                            planet_id,
                            region_id,
                            building_id,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.build_in_region(&planet_id, region_id, &building_id);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::DemolishOnTile {
                            planet_id,
                            tile_index,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.demolish_on_tile(&planet_id, tile_index, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::PrivatizeSlot {
                            planet_id,
                            tile_index,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.privatize_slot(&planet_id, tile_index, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::NationalizeSlot {
                            planet_id,
                            tile_index,
                            slot,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.nationalize_slot(&planet_id, tile_index, slot);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::AcquireBankruptSlot {
                            planet_id,
                            tile_index,
                            slot,
                            buyer_polity,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.acquire_bankrupt_slot(
                                &planet_id,
                                tile_index,
                                slot,
                                buyer_polity,
                            );
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::ResolvePendingEvent {
                            instance_id,
                            option_index,
                            origin_sub_id,
                            reply,
                        } => {
                            let res = game.resolve_pending_event(instance_id, option_index);
                            let out = match res {
                                Ok(()) => {
                                    let snap = publish_snapshot(game);
                                    broadcast_watched_planets(subscribers, game, None);
                                    Ok(snap)
                                }
                                Err(err) => {
                                    if let Some(target_sub) = origin_sub_id {
                                        for sub in subscribers.iter() {
                                            if sub.id == target_sub {
                                                let _ =
                                                    sub.tx.try_send(StreamEvent::CommandError {
                                                        message: err.clone(),
                                                    });
                                            }
                                        }
                                    }
                                    Err(err)
                                }
                            };
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(out);
                            }
                            true
                        }
                        RuntimeCommand::TogglePrivateLandUse {
                            planet_id,
                            sector_id,
                            allow,
                            origin_sub_id,
                            reply,
                        } => {
                            let result =
                                game.toggle_private_land_use(&planet_id, sector_id, allow);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::SetPolicy {
                            planet_id,
                            change,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = match change {
                                PolicyChange::Monetary(p) => game.set_monetary_policy(&planet_id, p),
                                PolicyChange::Depot { sector_id, policy } => {
                                    game.set_depot_policy(&planet_id, sector_id, policy)
                                }
                            };
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::Subscribe {
                            id,
                            watched_planet,
                            tx,
                        } => {
                            let default_planet_id = game.default_planet_id();
                            let target = watched_planet.or_else(|| default_planet_id.clone());
                            let init = StreamEvent::Init {
                                time: game.time(),
                                default_planet_id,
                            };
                            if tx.try_send(init).is_ok() {
                                let surface = target
                                    .as_deref()
                                    .and_then(|pid| game.planet_surface_by_id(pid));
                                let planet =
                                    target.as_deref().and_then(|pid| game.planet_by_id(pid));
                                if tx.try_send(StreamEvent::PlanetSurface { surface }).is_ok()
                                    && tx.try_send(StreamEvent::PlanetView { planet }).is_ok()
                                {
                                    subscribers.push(Subscriber {
                                        id,
                                        tx,
                                        watched_planet: target,
                                    });
                                }
                            }
                            true
                        }
                        RuntimeCommand::WatchPlanet { id, planet_id } => {
                            let surface = planet_id
                                .as_deref()
                                .and_then(|pid| game.planet_surface_by_id(pid));
                            let planet =
                                planet_id.as_deref().and_then(|pid| game.planet_by_id(pid));
                            subscribers.retain_mut(|sub| {
                                if sub.id != id {
                                    return true;
                                }
                                sub.watched_planet = planet_id.clone();
                                !matches!(
                                    sub.tx.try_send(StreamEvent::PlanetSurface {
                                        surface: surface.clone()
                                    }),
                                    Err(mpsc::TrySendError::Disconnected(_))
                                ) && !matches!(
                                    sub.tx.try_send(StreamEvent::PlanetView {
                                        planet: planet.clone()
                                    }),
                                    Err(mpsc::TrySendError::Disconnected(_))
                                )
                            });
                            true
                        }
                        RuntimeCommand::GetPlanetSurface { planet_id, reply } => {
                            let _ = reply.send(game.planet_surface_by_id(&planet_id));
                            true
                        }
                        RuntimeCommand::SetEarthGridPreset { dense, reply } => {
                            let target_cells = if dense {
                                surface::DETAILED_GRID_CELLS
                            } else {
                                surface::DEFAULT_GRID_CELLS
                            };
                            let cur_scen = game.scenario();
                            let res = Game::with_scenario_and_cells(
                                game.calendar.clone(),
                                cur_scen,
                                target_cells,
                            )
                            .and_then(|next_game| {
                                *game = next_game;
                                *last_step = Instant::now();
                                let pid = game
                                    .default_planet_id()
                                    .unwrap_or_else(|| "base:earth".to_string());
                                game.planet_by_id(&pid)
                                    .ok_or_else(|| "无法生成星球快照。".to_string())
                            });
                            if res.is_ok() {
                                let snap = publish_snapshot(game);
                                let default_pid = game.default_planet_id();
                                subscribers.retain_mut(|sub| {
                                    let init_ev = StreamEvent::Init {
                                        time: snap.time.clone(),
                                        default_planet_id: default_pid.clone(),
                                    };
                                    if matches!(
                                        sub.tx.try_send(init_ev),
                                        Err(mpsc::TrySendError::Disconnected(_))
                                    ) {
                                        return false;
                                    }
                                    if sub
                                        .watched_planet
                                        .as_deref()
                                        .and_then(|id| game.planet_by_id(id))
                                        .is_none()
                                    {
                                        sub.watched_planet = default_pid.clone();
                                    }
                                    if let Some(watched_id) = sub.watched_planet.as_deref() {
                                        let surf = game.planet_surface_by_id(watched_id);
                                        let pview = game.planet_by_id(watched_id);
                                        if matches!(
                                            sub.tx.try_send(StreamEvent::PlanetSurface {
                                                surface: surf
                                            }),
                                            Err(mpsc::TrySendError::Disconnected(_))
                                        ) {
                                            return false;
                                        }
                                        if matches!(
                                            sub.tx.try_send(StreamEvent::PlanetView {
                                                planet: pview
                                            }),
                                            Err(mpsc::TrySendError::Disconnected(_))
                                        ) {
                                            return false;
                                        }
                                    }
                                    true
                                });
                            }
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(res);
                            }
                            true
                        }
                        RuntimeCommand::SetScenarioPreset {
                            scenario,
                            origin_sub_id,
                            reply,
                        } => {
                            let res =
                                Game::with_scenario_preset(game.calendar.clone(), scenario)
                                    .and_then(|next_game| {
                                        *game = next_game;
                                        *last_step = Instant::now();
                                        let pid = game
                                            .default_planet_id()
                                            .unwrap_or_else(|| "base:earth".to_string());
                                        game.planet_by_id(&pid)
                                            .ok_or_else(|| "无法生成开局星球快照。".to_string())
                                    });
                            match &res {
                                Ok(_) => {
                                    let snap = publish_snapshot(game);
                                    let default_pid = game.default_planet_id();
                                    let label = match scenario {
                                        ScenarioPreset::Earth => "地球开局",
                                        ScenarioPreset::Colony => "殖民地开局",
                                    };
                                    subscribers.retain_mut(|sub| {
                                        let init_ev = StreamEvent::Init {
                                            time: snap.time.clone(),
                                            default_planet_id: default_pid.clone(),
                                        };
                                        if matches!(
                                            sub.tx.try_send(init_ev),
                                            Err(mpsc::TrySendError::Disconnected(_))
                                        ) {
                                            return false;
                                        }
                                        sub.watched_planet = default_pid.clone();
                                        if let Some(watched_id) = sub.watched_planet.as_deref() {
                                            let surf = game.planet_surface_by_id(watched_id);
                                            let pview = game.planet_by_id(watched_id);
                                            let _ = sub
                                                .tx
                                                .try_send(StreamEvent::PlanetSurface { surface: surf });
                                            let _ = sub
                                                .tx
                                                .try_send(StreamEvent::PlanetView { planet: pview });
                                        }
                                        if origin_sub_id.is_none_or(|id| id == sub.id) {
                                            let _ = sub.tx.try_send(StreamEvent::CommandNotice {
                                                message: format!("已切换至「{label}」并开启新对局。"),
                                            });
                                        }
                                        true
                                    });
                                }
                                Err(err) => {
                                    for sub in subscribers.iter() {
                                        if origin_sub_id.is_none_or(|id| id == sub.id) {
                                            let _ = sub.tx.try_send(StreamEvent::CommandError {
                                                message: err.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(res);
                            }
                            true
                        }
                        RuntimeCommand::ClaimTile {
                            planet_id,
                            tile_index,
                            origin_sub_id,
                            reply,
                        } => {
                            let result = game.claim_tile(&planet_id, tile_index);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::ClaimRegionFrontier {
                            planet_id,
                            region_id,
                            max_count,
                            origin_sub_id,
                            reply,
                        } => {
                            let result =
                                game.claim_region_frontier(&planet_id, region_id, max_count);
                            notify_mutation_outcome(
                                &result,
                                &planet_id,
                                origin_sub_id,
                                game,
                                subscribers,
                            );
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(result);
                            }
                            true
                        }
                        RuntimeCommand::ReloadData {
                            restart_world,
                            origin_sub_id,
                            reply,
                        } => {
                            let res = if restart_world {
                                game.reload_and_restart()
                            } else {
                                game.reload_data_in_place()
                            };
                            match &res {
                                Ok(msg) => {
                                    let snap = publish_snapshot(game);
                                    let default_pid = game.default_planet_id();
                                    if restart_world {
                                        *last_step = Instant::now();
                                        subscribers.retain_mut(|sub| {
                                            let init_ev = StreamEvent::Init {
                                                time: snap.time.clone(),
                                                default_planet_id: default_pid.clone(),
                                            };
                                            if matches!(
                                                sub.tx.try_send(init_ev),
                                                Err(mpsc::TrySendError::Disconnected(_))
                                            ) {
                                                return false;
                                            }
                                            if sub
                                                .watched_planet
                                                .as_deref()
                                                .and_then(|id| game.planet_by_id(id))
                                                .is_none()
                                            {
                                                sub.watched_planet = default_pid.clone();
                                            }
                                            if let Some(watched_id) = sub.watched_planet.as_deref() {
                                                let surf = game.planet_surface_by_id(watched_id);
                                                let pview = game.planet_by_id(watched_id);
                                                let _ = sub.tx.try_send(StreamEvent::PlanetSurface {
                                                    surface: surf,
                                                });
                                                let _ = sub
                                                    .tx
                                                    .try_send(StreamEvent::PlanetView { planet: pview });
                                            }
                                            true
                                        });
                                    } else {
                                        broadcast_watched_planets(subscribers, game, None);
                                    }
                                    for sub in subscribers.iter() {
                                        if origin_sub_id.is_none_or(|id| id == sub.id) {
                                            let _ = sub.tx.try_send(StreamEvent::CommandNotice {
                                                message: msg.clone(),
                                            });
                                        }
                                    }
                                }
                                Err(err) => {
                                    for sub in subscribers.iter() {
                                        if origin_sub_id.is_none_or(|id| id == sub.id) {
                                            let _ = sub.tx.try_send(StreamEvent::CommandError {
                                                message: err.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                            if let Some(reply_tx) = reply {
                                let _ = reply_tx.send(res);
                            }
                            true
                        }
                        RuntimeCommand::Unsubscribe { id } => {
                            subscribers.retain(|sub| sub.id != id);
                            true
                        }
                        RuntimeCommand::Shutdown => false,
                    }
                };

                let auto_reload_dir = std::env::var("SPACE_STRATEGY_AUTO_RELOAD")
                    .ok()
                    .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .then(|| crate::data_bundle::resolve_gamedata_dir().ok())
                    .flatten();
                let mut last_mtime = auto_reload_dir
                    .as_deref()
                    .and_then(crate::data_bundle::gamedata_latest_mtime);
                let mut last_watch_poll = Instant::now();

                loop {
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        if !handle_cmd(cmd, &mut game, &mut last_step, &mut subscribers) {
                            return;
                        }
                    }

                    if let Some(ref watch_dir) = auto_reload_dir {
                        if last_watch_poll.elapsed() >= Duration::from_millis(1500) {
                            last_watch_poll = Instant::now();
                            let cur_m = crate::data_bundle::gamedata_latest_mtime(watch_dir);
                            if cur_m.is_some() && cur_m != last_mtime {
                                last_mtime = cur_m;
                                let _ = handle_cmd(
                                    RuntimeCommand::ReloadData {
                                        restart_world: false,
                                        origin_sub_id: None,
                                        reply: None,
                                    },
                                    &mut game,
                                    &mut last_step,
                                    &mut subscribers,
                                );
                            }
                        }
                    }

                    let Some(interval) = game.next_step_interval() else {
                        let poll_wait = if auto_reload_dir.is_some() {
                            Duration::from_millis(1500)
                        } else {
                            Duration::from_secs(3600)
                        };
                        match cmd_rx.recv_timeout(poll_wait) {
                            Ok(cmd) => {
                                if !handle_cmd(cmd, &mut game, &mut last_step, &mut subscribers) {
                                    return;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                        continue;
                    };

                    let elapsed = Instant::now().saturating_duration_since(last_step);
                    if elapsed >= interval {
                        let outcome = game.step_once();
                        last_step = Instant::now();
                        let snap = publish_snapshot(&game);
                        broadcast_tick(&mut subscribers, snap.time.clone());
                        if outcome.month_advanced {
                            broadcast_watched_planets(&mut subscribers, &game, None);
                        }
                    } else {
                        let wait = interval - elapsed;
                        match cmd_rx.recv_timeout(wait) {
                            Ok(cmd) => {
                                if !handle_cmd(cmd, &mut game, &mut last_step, &mut subscribers) {
                                    return;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
            })
            .expect("启动模拟线程失败");

        Self {
            snapshot,
            cmd_tx,
            next_sub_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    pub fn snapshot(&self) -> Result<Arc<GameSnapshot>, String> {
        let slot = self
            .snapshot
            .lock()
            .map_err(|_| "游戏状态不可用，请重新启动。".to_string())?;
        Ok(Arc::clone(&slot))
    }

    pub fn set_speed(&self, speed: GameSpeed) -> Result<Arc<GameSnapshot>, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::SetSpeed(speed, Some(reply_tx)))
            .map_err(|_| "模拟线程已停止，无法修改游戏速度。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应速度变更超时。".to_string())
    }

    pub fn set_speed_async(&self, speed: GameSpeed) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::SetSpeed(speed, None))
            .map_err(|_| "模拟线程已停止，无法修改游戏速度。".to_string())
    }

    pub fn set_earth_grid_preset(&self, dense: bool) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::SetEarthGridPreset {
                dense,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法切换地球网格密度。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "等待模拟线程切换地球网格超时。".to_string())?
    }

    pub fn set_earth_grid_preset_async(&self, dense: bool) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::SetEarthGridPreset { dense, reply: None })
            .map_err(|_| "模拟线程已停止，无法切换地球网格密度。".to_string())
    }

    pub fn set_scenario_preset(&self, scenario: ScenarioPreset) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::SetScenarioPreset {
                scenario,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法切换开局剧本。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(4))
            .map_err(|_| "等待模拟线程切换开局剧本超时。".to_string())?
    }

    pub fn set_scenario_preset_async(
        &self,
        origin_sub_id: Option<u64>,
        scenario: ScenarioPreset,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::SetScenarioPreset {
                scenario,
                origin_sub_id,
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法切换开局剧本。".to_string())
    }

    pub fn claim_tile(
        &self,
        planet_id: String,
        tile_index: usize,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::ClaimTile {
                planet_id,
                tile_index,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法框入地块。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程框入地块超时。".to_string())?
    }

    pub fn claim_tile_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::ClaimTile {
                planet_id,
                tile_index,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法框入地块。".to_string())
    }

    pub fn claim_region_frontier(
        &self,
        planet_id: String,
        region_id: u16,
        max_count: usize,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::ClaimRegionFrontier {
                planet_id,
                region_id,
                max_count,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法批量框入荒野地块。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程批量框入地块超时。".to_string())?
    }

    pub fn claim_region_frontier_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        region_id: u16,
        max_count: usize,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::ClaimRegionFrontier {
                planet_id,
                region_id,
                max_count,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法批量框入荒野地块。".to_string())
    }

    pub fn reload_data(&self, restart_world: bool) -> Result<String, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::ReloadData {
                restart_world,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法热重载外部数据。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "等待模拟线程热重载外部数据超时。".to_string())?
    }

    pub fn reload_data_async(
        &self,
        origin_sub_id: Option<u64>,
        restart_world: bool,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::ReloadData {
                restart_world,
                origin_sub_id,
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法热重载外部数据。".to_string())
    }

    pub fn build_on_tile(
        &self,
        planet_id: String,
        tile_index: usize,
        building_id: String,
        slot: Option<TileSlotKey>,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::BuildOnTile {
                planet_id,
                tile_index,
                building_id,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法建造建筑。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应建造请求超时。".to_string())?
    }

    pub fn build_on_tile_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        building_id: String,
        slot: Option<TileSlotKey>,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::BuildOnTile {
                planet_id,
                tile_index,
                building_id,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法建造建筑。".to_string())
    }

    pub fn build_in_region(
        &self,
        planet_id: String,
        region_id: u16,
        building_id: String,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::BuildInRegion {
                planet_id,
                region_id,
                building_id,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法在大区建造建筑。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应大区建造请求超时。".to_string())?
    }

    pub fn build_in_region_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        region_id: u16,
        building_id: String,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::BuildInRegion {
                planet_id,
                region_id,
                building_id,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法在大区建造建筑。".to_string())
    }

    pub fn demolish_on_tile(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::DemolishOnTile {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法拆除建筑。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应拆除请求超时。".to_string())?
    }

    pub fn demolish_on_tile_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::DemolishOnTile {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法拆除建筑。".to_string())
    }

    pub fn privatize_slot(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::PrivatizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法执行私有化。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应私有化请求超时。".to_string())?
    }

    pub fn privatize_slot_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::PrivatizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法执行私有化。".to_string())
    }

    pub fn nationalize_slot(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::NationalizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法执行国有化。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应国有化请求超时。".to_string())?
    }

    pub fn nationalize_slot_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::NationalizeSlot {
                planet_id,
                tile_index,
                slot,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法执行国有化。".to_string())
    }

    pub fn acquire_bankrupt_slot(
        &self,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        buyer_polity: Option<crate::economy::PolityId>,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::AcquireBankruptSlot {
                planet_id,
                tile_index,
                slot,
                buyer_polity,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法收购破产工场。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应破产收购请求超时。".to_string())?
    }

    pub fn acquire_bankrupt_slot_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        tile_index: usize,
        slot: TileSlotKey,
        buyer_polity: Option<crate::economy::PolityId>,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::AcquireBankruptSlot {
                planet_id,
                tile_index,
                slot,
                buyer_polity,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法收购破产工场。".to_string())
    }

    pub fn resolve_pending_event(
        &self,
        instance_id: u64,
        option_index: usize,
    ) -> Result<Arc<GameSnapshot>, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::ResolvePendingEvent {
                instance_id,
                option_index,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法处理待决事件。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程处理待决事件超时。".to_string())?
    }

    pub fn resolve_pending_event_async(
        &self,
        origin_sub_id: u64,
        instance_id: u64,
        option_index: usize,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::ResolvePendingEvent {
                instance_id,
                option_index,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法处理待决事件。".to_string())
    }

    pub fn toggle_private_land_use(
        &self,
        planet_id: String,
        sector_id: u16,
        allow: bool,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::TogglePrivateLandUse {
                planet_id,
                sector_id,
                allow,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法切换用地政策。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应用地政策变更超时。".to_string())?
    }

    pub fn set_policy(
        &self,
        planet_id: String,
        change: PolicyChange,
    ) -> Result<PlanetSnapshot, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::SetPolicy {
                planet_id,
                change,
                origin_sub_id: None,
                reply: Some(reply_tx),
            })
            .map_err(|_| "模拟线程已停止，无法调整政策。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程响应政策变更超时。".to_string())?
    }

    pub fn set_policy_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        change: PolicyChange,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::SetPolicy {
                planet_id,
                change,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法调整政策。".to_string())
    }

    pub fn toggle_private_land_use_async(
        &self,
        origin_sub_id: u64,
        planet_id: String,
        sector_id: u16,
        allow: bool,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::TogglePrivateLandUse {
                planet_id,
                sector_id,
                allow,
                origin_sub_id: Some(origin_sub_id),
                reply: None,
            })
            .map_err(|_| "模拟线程已停止，无法切换用地政策。".to_string())
    }

    pub fn planet_surface(&self, planet_id: String) -> Result<Option<PlanetSurfaceSnapshot>, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(RuntimeCommand::GetPlanetSurface {
                planet_id,
                reply: reply_tx,
            })
            .map_err(|_| "模拟线程已停止，无法查询星球地表。".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "等待模拟线程生成地表数据超时。".to_string())
    }

    pub fn subscribe(
        &self,
        watched_planet: Option<String>,
    ) -> Result<(u64, mpsc::Receiver<StreamEvent>), String> {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(STREAM_BUFFER_CAPACITY);
        self.cmd_tx
            .send(RuntimeCommand::Subscribe {
                id,
                watched_planet,
                tx,
            })
            .map_err(|_| "模拟线程已停止，无法订阅状态流。".to_string())?;
        Ok((id, rx))
    }

    pub fn watch_planet(&self, id: u64, planet_id: Option<String>) -> Result<(), String> {
        self.cmd_tx
            .send(RuntimeCommand::WatchPlanet { id, planet_id })
            .map_err(|_| "模拟线程已停止，无法切换观察目标。".to_string())
    }

    pub fn unsubscribe(&self, id: u64) {
        let _ = self.cmd_tx.send(RuntimeCommand::Unsubscribe { id });
    }
}

impl Drop for GameRuntime {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(RuntimeCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
