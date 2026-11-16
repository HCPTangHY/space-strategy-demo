use game_core::{
    GameRuntime, GameSnapshot, GameSpeed, PlanetSnapshot, PlanetSurfaceSnapshot, StreamEvent,
    TileSlotKey,
};
use std::thread;

#[tauri::command]
fn get_planets(runtime: tauri::State<'_, GameRuntime>) -> Result<Vec<PlanetSnapshot>, String> {
    Ok(runtime.snapshot()?.planets.clone())
}

#[tauri::command]
fn get_planet_surface(
    planet_id: String,
    runtime: tauri::State<'_, GameRuntime>,
) -> Result<Option<PlanetSurfaceSnapshot>, String> {
    runtime.planet_surface(planet_id)
}

#[tauri::command]
fn get_game_state(runtime: tauri::State<'_, GameRuntime>) -> Result<GameSnapshot, String> {
    Ok((*runtime.snapshot()?).clone())
}

#[tauri::command]
fn set_game_speed(speed: u8, runtime: tauri::State<'_, GameRuntime>) -> Result<GameSnapshot, String> {
    let parsed = GameSpeed::try_from(speed)?;
    Ok((*runtime.set_speed(parsed)?).clone())
}

#[tauri::command]
fn build_on_tile(
    planet_id: String,
    tile_index: usize,
    building_id: String,
    slot: Option<TileSlotKey>,
    runtime: tauri::State<'_, GameRuntime>,
) -> Result<PlanetSnapshot, String> {
    runtime.build_on_tile(planet_id, tile_index, building_id, slot)
}

#[tauri::command]
fn build_in_region(
    planet_id: String,
    region_id: u16,
    building_id: String,
    runtime: tauri::State<'_, GameRuntime>,
) -> Result<PlanetSnapshot, String> {
    runtime.build_in_region(planet_id, region_id, building_id)
}

#[tauri::command]
fn subscribe_stream(
    planet_id: Option<String>,
    channel: tauri::ipc::Channel<StreamEvent>,
    app: tauri::AppHandle,
    runtime: tauri::State<'_, GameRuntime>,
) -> Result<u64, String> {
    use tauri::Manager;
    let (sub_id, rx) = runtime.subscribe(planet_id)?;
    thread::Builder::new()
        .name(format!("tauri-stream-{sub_id}"))
        .spawn(move || {
            while let Ok(event) = rx.recv() {
                if channel.send(event).is_err() {
                    break;
                }
            }
            if let Some(rt) = app.try_state::<GameRuntime>() {
                rt.unsubscribe(sub_id);
            }
        })
        .map_err(|_| "创建推送通道线程失败。".to_string())?;
    Ok(sub_id)
}

#[tauri::command]
fn watch_planet(
    sub_id: u64,
    planet_id: Option<String>,
    runtime: tauri::State<'_, GameRuntime>,
) -> Result<(), String> {
    runtime.watch_planet(sub_id, planet_id)
}

#[tauri::command]
fn unsubscribe_stream(sub_id: u64, runtime: tauri::State<'_, GameRuntime>) {
    runtime.unsubscribe(sub_id);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(GameRuntime::default())
        .invoke_handler(tauri::generate_handler![
            get_planets,
            get_planet_surface,
            get_game_state,
            set_game_speed,
            build_on_tile,
            build_in_region,
            subscribe_stream,
            watch_planet,
            unsubscribe_stream
        ])
        .run(tauri::generate_context!())
        .expect("启动桌面应用失败");
}
