//! Development preview adapter sharing `GameRuntime` via WebSocket push + HTTP diagnostics.
//! Not a multiplayer server.
use game_core::{ClientCommand, GameRuntime, GameSpeed, PolicyChange, TileSlotKey};
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Response, Server, StatusCode};
use tungstenite::handshake::server::{ErrorResponse, Request, Response as WsResponse};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{accept_hdr_with_config, Error as WsError, Message};

const MAX_WS_CLIENTS: usize = 32;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let http_server = Server::http("127.0.0.1:18421")?;
    let ws_listener = TcpListener::bind("127.0.0.1:18422")?;
    let runtime = Arc::new(GameRuntime::default());

    let ws_runtime = Arc::clone(&runtime);
    thread::Builder::new()
        .name("web-preview-ws-accept".into())
        .spawn(move || run_ws_listener(ws_listener, ws_runtime))?;

    eprintln!(
        "Rust ECS preview listening: HTTP http://127.0.0.1:18421 | WS ws://127.0.0.1:18422/api/ws"
    );
    run_http_server(http_server, runtime)
}

fn run_ws_listener(listener: TcpListener, runtime: Arc<GameRuntime>) {
    let active_clients = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        if active_clients.fetch_add(1, Ordering::AcqRel) >= MAX_WS_CLIENTS {
            active_clients.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let client_runtime = Arc::clone(&runtime);
        let client_counter = Arc::clone(&active_clients);
        let _ = thread::Builder::new()
            .name("web-preview-ws-client".into())
            .spawn(move || {
                let _ = handle_ws_client(stream, &client_runtime);
                client_counter.fetch_sub(1, Ordering::AcqRel);
            });
    }
}

struct SubGuard<'a> {
    runtime: &'a GameRuntime,
    sub_id: u64,
}

impl Drop for SubGuard<'_> {
    fn drop(&mut self) {
        self.runtime.unsubscribe(self.sub_id);
    }
}

fn is_allowed_origin(origin: &str) -> bool {
    origin == "https://files.zhenxia.top"
        || origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:")
}

fn handle_ws_client(stream: TcpStream, runtime: &GameRuntime) -> Result<(), ()> {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mode_handle = stream.try_clone().map_err(|_| ())?;

    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(512 * 1024);
    config.max_frame_size = Some(512 * 1024);

    let handshake_check = |req: &Request, response: WsResponse| -> Result<WsResponse, ErrorResponse> {
        if let Some(origin_hdr) = req.headers().get("Origin") {
            let ok = origin_hdr
                .to_str()
                .map(is_allowed_origin)
                .unwrap_or(false);
            if !ok {
                let forbidden = tungstenite::http::Response::builder()
                    .status(403)
                    .body(Some("Forbidden origin".to_string()))
                    .unwrap();
                return Err(forbidden);
            }
        }
        Ok(response)
    };

    let mut ws = accept_hdr_with_config(stream, handshake_check, Some(config)).map_err(|_| ())?;
    let (sub_id, rx) = runtime.subscribe(None).map_err(|_| ())?;
    let _guard = SubGuard { runtime, sub_id };
    let mut last_ping = Instant::now();

    loop {
        // 1. Drain any pending simulation push events immediately in blocking write mode.
        while let Ok(event) = rx.try_recv() {
            let text = serde_json::to_string(&event).map_err(|_| ())?;
            ws.send(Message::text(text)).map_err(|_| ())?;
        }

        // 2. Non-blocking read of incoming client commands (`set_speed`, `watch_planet`, Ping/Close).
        mode_handle.set_nonblocking(true).map_err(|_| ())?;
        let read_status = drain_client_messages(&mut ws, runtime, sub_id);
        mode_handle.set_nonblocking(false).map_err(|_| ())?;
        read_status?;
        ws.flush().map_err(|_| ())?;

        // 3. Keep proxy connections alive while paused.
        if last_ping.elapsed() >= Duration::from_secs(20) {
            ws.send(Message::Ping(Vec::new().into())).map_err(|_| ())?;
            last_ping = Instant::now();
        }

        // 4. Wait up to 10ms on simulation push channel: 0ms latency on server push, <=10ms on client input.
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(event) => {
                let text = serde_json::to_string(&event).map_err(|_| ())?;
                ws.send(Message::text(text)).map_err(|_| ())?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn drain_client_messages(
    ws: &mut tungstenite::WebSocket<TcpStream>,
    runtime: &GameRuntime,
    sub_id: u64,
) -> Result<(), ()> {
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                if text.len() > 4096 {
                    return Err(());
                }
                if let Ok(cmd) = serde_json::from_str::<ClientCommand>(text.as_str()) {
                    match cmd {
                        ClientCommand::SetSpeed { speed } => {
                            let _ = runtime.set_speed_async(speed);
                        }
                        ClientCommand::WatchPlanet { planet_id } => {
                            let _ = runtime.watch_planet(sub_id, planet_id);
                        }
                        ClientCommand::BuildOnTile {
                            planet_id,
                            tile_index,
                            building_id,
                            slot,
                        } => {
                            let _ = runtime.build_on_tile_async(
                                sub_id,
                                planet_id,
                                tile_index,
                                building_id,
                                slot,
                            );
                        }
                        ClientCommand::BuildInRegion {
                            planet_id,
                            region_id,
                            building_id,
                        } => {
                            let _ = runtime.build_in_region_async(
                                sub_id,
                                planet_id,
                                region_id,
                                building_id,
                            );
                        }
                        ClientCommand::DemolishOnTile {
                            planet_id,
                            tile_index,
                            slot,
                        } => {
                            let _ = runtime.demolish_on_tile_async(
                                sub_id,
                                planet_id,
                                tile_index,
                                slot,
                            );
                        }
                        ClientCommand::PrivatizeSlot {
                            planet_id,
                            tile_index,
                            slot,
                        } => {
                            let _ = runtime.privatize_slot_async(
                                sub_id,
                                planet_id,
                                tile_index,
                                slot,
                            );
                        }
                        ClientCommand::NationalizeSlot {
                            planet_id,
                            tile_index,
                            slot,
                        } => {
                            let _ = runtime.nationalize_slot_async(
                                sub_id,
                                planet_id,
                                tile_index,
                                slot,
                            );
                        }
                        ClientCommand::TogglePrivateLandUse {
                            planet_id,
                            sector_id,
                            allow,
                        } => {
                            let _ = runtime.toggle_private_land_use_async(
                                sub_id,
                                planet_id,
                                sector_id,
                                allow,
                            );
                        }
                        ClientCommand::SetMonetaryPolicy { planet_id, policy } => {
                            let _ = runtime.set_policy_async(
                                sub_id,
                                planet_id,
                                PolicyChange::Monetary(policy),
                            );
                        }
                        ClientCommand::SetDepotPolicy {
                            planet_id,
                            sector_id,
                            policy,
                        } => {
                            let _ = runtime.set_policy_async(
                                sub_id,
                                planet_id,
                                PolicyChange::Depot { sector_id, policy },
                            );
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => return Err(()),
            Ok(_) => {}
            Err(WsError::Io(ref err)) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(_) => return Err(()),
        }
    }
}

fn run_http_server(
    server: Server,
    runtime: Arc<GameRuntime>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for mut request in server.incoming_requests() {
        let mut allow: Option<&'static str> = None;
        let (status, body) = match (request.method(), request.url()) {
            (&Method::Get, "/api/state") => match runtime.snapshot() {
                Ok(snapshot) => serialize_json(&*snapshot),
                Err(err) => (500, json_error(&err)),
            },
            (&Method::Get, "/api/planets") => match runtime.snapshot() {
                Ok(snapshot) => serialize_json(&snapshot.planets),
                Err(err) => (500, json_error(&err)),
            },
            (&Method::Get, url) if url == "/api/surface" || url.starts_with("/api/surface?") => {
                let planet_id = url
                    .split_once('?')
                    .and_then(|(_, q)| {
                        q.split('&').find_map(|kv| kv.strip_prefix("planet_id="))
                    })
                    .unwrap_or("base:earth")
                    .to_string();
                match runtime.planet_surface(planet_id) {
                    Ok(Some(surface)) => serialize_json(&surface),
                    Ok(None) => (404, json_error("找不到指定的星球地表。")),
                    Err(err) => (500, json_error(&err)),
                }
            }
            (&Method::Post, "/api/speed") => {
                let mut raw = Vec::with_capacity(64);
                if request.as_reader().take(257).read_to_end(&mut raw).is_err() || raw.len() > 256 {
                    (400, json_error("请求体过大或无法读取。"))
                } else {
                    match parse_speed_payload(&raw) {
                        Ok(speed) => match runtime.set_speed(speed) {
                            Ok(snapshot) => serialize_json(&*snapshot),
                            Err(err) => (500, json_error(&err)),
                        },
                        Err(err) => (400, json_error(&err)),
                    }
                }
            }
            (&Method::Post, "/api/build") => {
                let mut raw = Vec::with_capacity(128);
                if request.as_reader().take(513).read_to_end(&mut raw).is_err() || raw.len() > 512 {
                    (400, json_error("请求体过大或无法读取。"))
                } else {
                    match parse_build_payload(&raw) {
                        Ok((planet_id, tile_index, building_id, slot)) => {
                            match runtime.build_on_tile(planet_id, tile_index, building_id, slot) {
                                Ok(planet) => serialize_json(&planet),
                                Err(err) => (400, json_error(&err)),
                            }
                        }
                        Err(err) => (400, json_error(&err)),
                    }
                }
            }
            (_, "/api/state") | (_, "/api/planets") => {
                allow = Some("GET");
                (405, json_error("Only GET is supported"))
            }
            (_, "/api/speed") | (_, "/api/build") => {
                allow = Some("POST");
                (405, json_error("Only POST is supported"))
            }
            _ => (404, json_error("Not found")),
        };

        let mut response = Response::from_string(body)
            .with_status_code(StatusCode(status))
            .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
            .with_header(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap());
        if let Some(allowed) = allow {
            response.add_header(Header::from_bytes("Allow", allowed).unwrap());
        }
        if let Err(error) = request.respond(response) {
            eprintln!("HTTP response failed: {error}");
        }
    }
    Ok(())
}

fn parse_speed_payload(raw: &[u8]) -> Result<GameSpeed, String> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).map_err(|_| "无效的 JSON 请求体。".to_string())?;
    let speed_val = value
        .get("speed")
        .ok_or_else(|| "缺少 speed 字段。".to_string())?
        .clone();
    serde_json::from_value::<GameSpeed>(speed_val)
        .map_err(|_| "speed 必须是 0 到 4 之间的整数。".to_string())
}

fn parse_build_payload(raw: &[u8]) -> Result<(String, usize, String, Option<TileSlotKey>), String> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).map_err(|_| "无效的 JSON 请求体。".to_string())?;
    let planet_id = value
        .get("planet_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少有效的 planet_id 字段。".to_string())?
        .to_string();
    let tile_index = value
        .get("tile_index")
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| "缺少有效的 tile_index 字段。".to_string())?;
    let building_id = value
        .get("building_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少有效的 building_id 字段。".to_string())?
        .to_string();
    let slot = match value.get("slot") {
        None | Some(serde_json::Value::Null) => None,
        Some(s) => Some(
            serde_json::from_value::<TileSlotKey>(s.clone())
                .map_err(|_| "无效的 slot 槽位标识。".to_string())?,
        ),
    };
    Ok((planet_id, tile_index, building_id, slot))
}

fn serialize_json<T: serde::Serialize>(value: &T) -> (u16, String) {
    match serde_json::to_string(value) {
        Ok(body) => (200, body),
        Err(error) => {
            eprintln!("Snapshot serialization failed: {error}");
            (500, json_error("Unable to serialize game state"))
        }
    }
}

fn json_error(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}
