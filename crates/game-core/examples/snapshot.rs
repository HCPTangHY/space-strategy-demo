fn main() {
    let game = game_core::Game::default();
    println!(
        "{}",
        serde_json::to_string(&game.snapshot()).expect("序列化游戏快照失败")
    );
}
