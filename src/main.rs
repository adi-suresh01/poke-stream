mod ascii;
mod pokemon;

use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Write;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task;
use tokio::time::{self, Duration, MissedTickBehavior};

#[derive(PartialEq)]
enum GameState {
    Idle,
    Throwing,
    Opening,
    Absorbing,
    Closing,
    Shaking,
    StarHold,
}

#[derive(Copy, Clone)]
enum CellColor {
    None,
    Ansi(&'static str),
    Rgb(u8, u8, u8),
}

struct Assets {
    pokemons: Vec<PokemonAsset>,
    arcanine_frames: Vec<ascii::AsciiImage>,
    pokedex: PokedexView,
}

enum Screen {
    Name,
    Pokedex,
    PokedexDetail,
    Game,
}

#[derive(Copy, Clone, PartialEq)]
enum SelectionMode {
    DailyWeighted,
    RandomPerSession,
}

#[derive(Copy, Clone, PartialEq)]
enum ColorMode {
    Truecolor,
    Ansi256,
    Mono,
}

struct RenderBuffers {
    output: Vec<char>,
    zbuffer: Vec<f32>,
    color_buf: Vec<CellColor>,
}

impl RenderBuffers {
    fn new(width: usize, height: usize) -> Self {
        let len = width * height;
        Self {
            output: vec![' '; len],
            zbuffer: vec![-99.0; len],
            color_buf: vec![CellColor::None; len],
        }
    }

    fn clear(&mut self) {
        self.output.fill(' ');
        self.zbuffer.fill(-99.0);
        self.color_buf.fill(CellColor::None);
    }
}

struct PokemonAsset {
    name: String,
    image: ascii::AsciiImage,
}

struct PokedexView {
    names: Vec<String>,
    totals_by_name: HashMap<String, u16>,
}

struct StreamParticle {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    ch: char,
    color: (u8, u8, u8),
    start_frame: u16,
}

fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client")
    })
}

fn pokemon_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const IMG_CHARSET: &str =
    ".'`^\",:;Il!i><~+_-?][}{1)(|\\/*tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$Ñ";

const DB_PATH: &str = "pokedex.db";
const OPEN_FRAMES: u16 = 10;
const ABSORB_FRAMES: u16 = 22;
const CLOSE_FRAMES: u16 = 10;
const SHAKE_FRAMES: u16 = 28;
const SHAKE_COUNT: u8 = 3;
const STAR_FRAMES: u16 = 26;

enum CommandAction {
    None,
    Exit,
    Disconnect,
}

enum OutputMessage {
    Bytes(Vec<u8>),
    Close { send_bye: bool },
}

struct SessionState {
    width: usize,
    height: usize,
    aspect_ratio: f32,
    chars: &'static str,
    pokemon_index: usize,
    selection_mode: SelectionMode,
    trainer_name: Option<String>,
    pokedex: HashSet<String>,
    screen: Screen,
    pokedex_detail: Option<usize>,
    state: GameState,
    frame_count: u32,
    capture_recorded: bool,
    open_amount: f32,
    capture_frame: u16,
    shake_frame: u16,
    shake_count: u8,
    stream_particles: Vec<StreamParticle>,
    align_start_a: f32,
    star_frame: u16,
    star_hold: u16,
    caught_message: Option<String>,
    caught_message_timer: u16,
    pokedex_notice: Option<String>,
    pokedex_notice_timer: u16,
    agent_lines: Vec<String>,
    agent_message_timer: u16,
    floor_y: f32,
    ball_x: f32,
    ball_y: f32,
    ball_scale: f32,
    a: f32,
    tilt_phase: f32,
    welcome_frame: usize,
    welcome_accum: u64,
    welcome_frame_ms: u64,
    last_cmd: String,
    color_mode: ColorMode,
    daily_key: i64,
}

impl SessionState {
    fn new(
        width: usize,
        height: usize,
        color_mode: ColorMode,
        selection_mode: SelectionMode,
        assets: &Assets,
    ) -> Self {
        let pokemon_index = match selection_mode {
            SelectionMode::RandomPerSession => pick_pokemon_index(&assets.pokemons),
            SelectionMode::DailyWeighted => 0,
        };
        Self {
            width,
            height,
            aspect_ratio: 1.5,
            chars: " .:-=+*#%@",
            pokemon_index,
            selection_mode,
            trainer_name: None,
            pokedex: HashSet::new(),
            screen: Screen::Name,
            pokedex_detail: None,
            state: GameState::Idle,
            frame_count: 0,
            capture_recorded: false,
            open_amount: 0.0,
            capture_frame: 0,
            shake_frame: 0,
            shake_count: 0,
            stream_particles: Vec::new(),
            align_start_a: 0.0,
            star_frame: 0,
            star_hold: 0,
            caught_message: None,
            caught_message_timer: 0,
            pokedex_notice: None,
            pokedex_notice_timer: 0,
            agent_lines: Vec::new(),
            agent_message_timer: 0,
            floor_y: 5.0,
            ball_x: -45.0,
            ball_y: 5.0,
            ball_scale: 1.0,
            a: 0.0,
            tilt_phase: 0.0,
            welcome_frame: 0,
            welcome_accum: 0,
            welcome_frame_ms: 1000 / 12,
            last_cmd: String::new(),
            color_mode,
            daily_key: -1,
        }
    }

    fn pokemon<'a>(&self, assets: &'a Assets) -> &'a PokemonAsset {
        &assets.pokemons[self.pokemon_index]
    }

    fn set_agent_message(&mut self, message: String) {
        let normalized = normalize_whitespace(&message);
        let max_w = self.width.saturating_sub(6).max(20);
        self.agent_lines = word_wrap(&normalized, max_w);
        self.agent_lines.truncate(4);
        self.agent_message_timer = 220;
    }

    fn screen_label(&self) -> &'static str {
        match self.screen {
            Screen::Name => "name",
            Screen::Pokedex => "pokedex",
            Screen::PokedexDetail => "pokedex_detail",
            Screen::Game => "game",
        }
    }

    fn dex_progress(&self, assets: &Assets) -> (usize, usize, usize) {
        let total = assets
            .pokedex
            .names
            .iter()
            .filter(|name| !name.is_empty())
            .count();
        let caught = assets
            .pokedex
            .names
            .iter()
            .filter(|name| !name.is_empty() && self.pokedex.contains(*name))
            .count();
        let left = total.saturating_sub(caught);
        (caught, total, left)
    }

    async fn answer_query_with_agent(&mut self, user_input: &str, assets: &Assets) -> bool {
        let normalized = normalize_whitespace(user_input);
        if normalized.is_empty() || !looks_like_agent_query_candidate(&normalized) {
            return false;
        }

        let trainer_name = self.trainer_name.clone();
        let current_pokemon = if matches!(self.screen, Screen::Game) {
            Some(self.pokemon(assets).name.clone())
        } else {
            None
        };
        let screen = self.screen_label().to_string();
        let (caught_count, total_count, left_count) = self.dex_progress(assets);

        let query = normalized.to_lowercase();

        let response = if is_agent_stats_query(&query) {
            if let Some(pokemon_name) = current_pokemon.as_deref() {
                fetch_pokemon_stats(pokemon_name).await.unwrap_or_else(|| {
                    format!(
                        "Agent: current pokemon is {}.",
                        display_pokemon_name(pokemon_name)
                    )
                })
            } else {
                "Agent: I can show stats from the catch screen.".to_string()
            }
        } else if is_agent_pokemon_query(&query) {
            if let Some(pokemon_name) = current_pokemon.as_deref() {
                fetch_pokemon_brief(pokemon_name).await.unwrap_or_else(|| {
                    format!(
                        "Agent: current pokemon is {}.",
                        display_pokemon_name(pokemon_name)
                    )
                })
            } else {
                "Agent: I can identify pokemon from the catch screen.".to_string()
            }
        } else if is_agent_left_to_catch_query(&query) {
            format!(
                "Agent: you caught {caught_count}/{total_count}. {left_count} pokemon left to catch."
            )
        } else if is_agent_caught_count_query(&query) {
            format!("Agent: you currently have {caught_count} out of {total_count} pokemon.")
        } else if is_agent_missing_list_query(&query) {
            let missing = missing_pokemon_preview(&self.pokedex, assets, 8);
            if missing.is_empty() {
                "Agent: your Gen 1 dex is complete. Legendaries are unlocked.".to_string()
            } else {
                format!(
                    "Agent: you are missing {}. Next few: {}.",
                    left_count,
                    missing.join(", ")
                )
            }
        } else if query == "help" || query.contains("what can you do") {
            "Agent: ask things like 'what is this pokemon?', 'how many pokemon do i have left?', or 'which pokemon am i missing?'.".to_string()
        } else {
            ask_llm_brief(
                &normalized,
                &screen,
                trainer_name.as_deref(),
                current_pokemon.as_deref(),
                caught_count,
                total_count,
                left_count,
            )
            .await
            .unwrap_or_else(|| {
                "Agent: I can answer pokemon identification and dex progress questions.".to_string()
            })
        };

        self.set_agent_message(response);
        true
    }

    async fn handle_command(&mut self, cmd: &str, assets: &Assets) -> CommandAction {
        let raw_cmd = cmd.trim().to_lowercase();
        let cmd_trim = raw_cmd.clone();

        if cmd.as_bytes().contains(&3) || matches!(cmd_trim.as_str(), "q" | "quit" | "exit") {
            return CommandAction::Exit;
        }
        if cmd_trim == "__disconnect__" {
            return CommandAction::Disconnect;
        }
        self.last_cmd = raw_cmd;
        match self.screen {
            Screen::Name => {
                if let Some(name) = sanitize_trainer_name(&cmd_trim) {
                    self.pokedex = load_pokedex(&name).await.unwrap_or_default();
                    self.trainer_name = Some(name);
                    match self.selection_mode {
                        SelectionMode::DailyWeighted => self.refresh_daily_pokemon(assets),
                        SelectionMode::RandomPerSession => {
                            self.pokemon_index = pick_pokemon_index(&assets.pokemons);
                        }
                    }
                    self.screen = Screen::Game;
                }
            }
            Screen::Pokedex => {
                if cmd_trim == "back" {
                    self.screen = Screen::Game;
                    self.pokedex_detail = None;
                    self.pokedex_notice = None;
                    self.pokedex_notice_timer = 0;
                } else if let Ok(id) = cmd_trim.parse::<usize>() {
                    if id >= 1 && id <= 151 {
                        if let Some(name) = assets.pokedex.names.get(id - 1) {
                            if !name.is_empty() && self.pokedex.contains(name) {
                                self.pokedex_detail = Some(id - 1);
                                self.screen = Screen::PokedexDetail;
                                self.pokedex_notice = None;
                                self.pokedex_notice_timer = 0;
                            } else {
                                self.pokedex_notice = Some("POKEMON NOT CAUGHT YET".to_string());
                                self.pokedex_notice_timer = 45;
                            }
                        }
                    }
                } else if self.answer_query_with_agent(&cmd_trim, assets).await {
                    return CommandAction::None;
                }
            }
            Screen::PokedexDetail => {
                if cmd_trim == "back" {
                    self.screen = Screen::Pokedex;
                } else if self.answer_query_with_agent(&cmd_trim, assets).await {
                    return CommandAction::None;
                }
            }
            Screen::Game => {
                if cmd_trim == "catch" && self.state == GameState::Idle {
                    self.state = GameState::Throwing;
                    self.frame_count = 0;
                    self.capture_recorded = false;
                } else if cmd_trim == "pokedex" || cmd_trim == "dex" {
                    self.screen = Screen::Pokedex;
                    self.pokedex_detail = None;
                    self.pokedex_notice = None;
                    self.pokedex_notice_timer = 0;
                } else if self.answer_query_with_agent(&cmd_trim, assets).await {
                    return CommandAction::None;
                }
            }
        }
        CommandAction::None
    }

    async fn update(&mut self, assets: &Assets) {
        if self.agent_message_timer > 0 {
            self.agent_message_timer = self.agent_message_timer.saturating_sub(1);
            if self.agent_message_timer == 0 {
                self.agent_lines.clear();
            }
        }

        if self.selection_mode == SelectionMode::DailyWeighted
            && self.trainer_name.is_some()
            && self.state == GameState::Idle
        {
            self.refresh_daily_pokemon(assets);
        }

        if let Screen::Game = self.screen {
            match self.state {
                GameState::Idle => {
                    self.frame_count = self.frame_count.saturating_add(1);
                    if self.frame_count > 60 {
                        self.frame_count = 0;
                    }
                }
                GameState::Throwing => {
                    self.ball_x += 1.5;
                    self.ball_y = self.floor_y + (self.ball_x * 0.5).sin() * 0.5;

                    if self.ball_x > 12.0 {
                        self.state = GameState::Opening;
                        self.ball_x = 15.0;
                        self.ball_y = self.floor_y;
                        self.capture_recorded = false;
                        self.capture_frame = 0;
                        self.open_amount = 0.0;
                        self.align_start_a = self.a;
                        let grow_start_y = 5;
                        let grow_start_x = (self.width / 2) - 2;
                        let ball_center_x = self.width as f32 / 2.0 + self.ball_x;
                        let ball_center_y = self.height as f32 / 2.0 + self.ball_y;
                        self.stream_particles = build_stream_particles(
                            self.pokemon(assets),
                            grow_start_x,
                            grow_start_y,
                            ball_center_x,
                            ball_center_y,
                        );
                    }
                }
                GameState::Opening => {
                    self.capture_frame = self.capture_frame.saturating_add(1);
                    let t = (self.capture_frame as f32 / OPEN_FRAMES as f32).min(1.0);
                    self.open_amount = t;
                    self.a = self.align_start_a * (1.0 - t);
                    self.ball_x = 15.0;
                    self.ball_y = self.floor_y;
                    if self.capture_frame >= OPEN_FRAMES {
                        self.state = GameState::Absorbing;
                        self.capture_frame = 0;
                    }
                }
                GameState::Absorbing => {
                    self.capture_frame = self.capture_frame.saturating_add(1);
                    self.open_amount = 1.0;
                    self.ball_x = 15.0;
                    self.ball_y = self.floor_y;
                    if self.capture_frame >= ABSORB_FRAMES {
                        self.state = GameState::Closing;
                        self.capture_frame = 0;
                        if !self.capture_recorded {
                            if let Some(name) = self.trainer_name.as_ref() {
                                if self.pokedex.insert(self.pokemon(assets).name.clone()) {
                                    let _ = save_pokedex(name, &self.pokedex).await;
                                }
                            }
                            self.capture_recorded = true;
                        }
                    }
                }
                GameState::Closing => {
                    self.capture_frame = self.capture_frame.saturating_add(1);
                    let t = self.capture_frame as f32 / CLOSE_FRAMES as f32;
                    self.open_amount = (1.0 - t).max(0.0);
                    self.ball_x = 15.0;
                    self.ball_y = self.floor_y;
                    if self.capture_frame >= CLOSE_FRAMES {
                        self.state = GameState::Shaking;
                        self.shake_frame = 0;
                        self.shake_count = 0;
                        self.open_amount = 0.0;
                    }
                }
                GameState::Shaking => {
                    self.shake_frame = self.shake_frame.saturating_add(1);
                    let phase = (self.shake_frame as f32 / SHAKE_FRAMES as f32)
                        * std::f32::consts::PI
                        * 2.0;
                    let wobble = phase.sin() * 1.3;
                    self.ball_x = 15.0 + wobble;
                    self.ball_y = self.floor_y;
                    if self.shake_frame >= SHAKE_FRAMES {
                        self.shake_frame = 0;
                        self.shake_count = self.shake_count.saturating_add(1);
                        if self.shake_count >= SHAKE_COUNT {
                            self.star_frame = STAR_FRAMES;
                            self.star_hold = 45;
                            self.state = GameState::StarHold;
                            self.caught_message = Some(format!(
                                "{} Caught!",
                                display_pokemon_name(&self.pokemon(assets).name)
                            ));
                            self.caught_message_timer = 45;
                        }
                    }
                }
                GameState::StarHold => {
                    self.ball_x = 15.0;
                    self.ball_y = self.floor_y;
                    if self.star_hold > 0 {
                        self.star_hold = self.star_hold.saturating_sub(1);
                        if self.caught_message_timer > 0 {
                            self.caught_message_timer = self.caught_message_timer.saturating_sub(1);
                        }
                    } else {
                        self.state = GameState::Idle;
                        self.ball_x = -45.0;
                        self.ball_y = self.floor_y;
                        self.frame_count = 0;
                        self.open_amount = 0.0;
                        self.stream_particles.clear();
                        self.caught_message = None;
                        self.caught_message_timer = 0;
                    }
                }
            }
        }

        match self.screen {
            Screen::Name => {
                self.welcome_accum += 30;
                if self.welcome_accum >= self.welcome_frame_ms {
                    self.welcome_accum = 0;
                    if !assets.arcanine_frames.is_empty() {
                        self.welcome_frame =
                            (self.welcome_frame + 1) % assets.arcanine_frames.len();
                    }
                }
            }
            Screen::Pokedex => {
                if self.pokedex_notice_timer > 0 {
                    self.pokedex_notice_timer = self.pokedex_notice_timer.saturating_sub(1);
                    if self.pokedex_notice_timer == 0 {
                        self.pokedex_notice = None;
                    }
                }
            }
            Screen::PokedexDetail => {}
            Screen::Game => {
                if self.state == GameState::Throwing {
                    self.a -= 0.2;
                } else if self.state == GameState::Idle {
                    self.a -= 0.05;
                }
                self.tilt_phase += 0.04;
            }
        }
    }

    fn render(&mut self, assets: &Assets, buffers: &mut RenderBuffers) {
        buffers.clear();
        let output = &mut buffers.output;
        let zbuffer = &mut buffers.zbuffer;
        let color_buf = &mut buffers.color_buf;
        let reset = "\x1b[0m";
        let red = "\x1b[91m";
        let white = "\x1b[97m";
        let black = "\x1b[30m";

        match self.screen {
            Screen::Name => {
                if !assets.arcanine_frames.is_empty() {
                    let frame =
                        &assets.arcanine_frames[self.welcome_frame % assets.arcanine_frames.len()];
                    let start_x = (self.width.saturating_sub(frame.width)) / 2;
                    let start_y = (self.height.saturating_sub(frame.height)) / 2;

                    for y in 0..frame.height {
                        for x in 0..frame.width {
                            let target_y = start_y + y;
                            let target_x = start_x + x;
                            if target_y < self.height && target_x < self.width {
                                let src_idx = x + y * frame.width;
                                let ch = frame.chars[src_idx];
                                if ch != ' ' {
                                    let idx = target_x + target_y * self.width;
                                    output[idx] = ch;
                                    let (r, g, b) = frame.colors[src_idx];
                                    color_buf[idx] = CellColor::Rgb(r, g, b);
                                    zbuffer[idx] = 0.2;
                                }
                            }
                        }
                    }
                }
            }
            Screen::Pokedex => {
                render_pokedex(
                    &assets.pokedex,
                    &self.pokedex,
                    output,
                    color_buf,
                    zbuffer,
                    self.width,
                    self.height,
                );
            }
            Screen::PokedexDetail => {
                if let Some(detail) = self.pokedex_detail {
                    render_pokedex_detail(
                        assets,
                        detail,
                        output,
                        color_buf,
                        zbuffer,
                        self.width,
                        self.height,
                    );
                }
            }
            Screen::Game => {
                let pokemon = self.pokemon(assets);
                if matches!(
                    self.state,
                    GameState::Idle | GameState::Throwing | GameState::Opening
                ) {
                    let grow_start_y = 5;
                    let grow_start_x = (self.width / 2) - 2;

                    for y in 0..pokemon.image.height {
                        for x in 0..pokemon.image.width {
                            let target_y = grow_start_y + y;
                            let target_x = grow_start_x + x;
                            if target_y < self.height && target_x < self.width {
                                let src_idx = x + y * pokemon.image.width;
                                let ch = pokemon.image.chars[src_idx];
                                if ch != ' ' {
                                    let idx = target_x + target_y * self.width;
                                    output[idx] = ch;
                                    let (r, g, b) = pokemon.image.colors[src_idx];
                                    color_buf[idx] = CellColor::Rgb(r, g, b);
                                    zbuffer[idx] = 0.4;
                                }
                            }
                        }
                    }
                }

                if self.state == GameState::Absorbing {
                    render_stream(
                        &self.stream_particles,
                        self.capture_frame,
                        output,
                        color_buf,
                        zbuffer,
                        self.width,
                        self.height,
                    );
                }

                let cos_a = self.a.cos();
                let sin_a = self.a.sin();
                let tilt = 0.25 + 0.1 * self.tilt_phase.sin();
                let cos_b = tilt.cos();
                let sin_b = tilt.sin();
                let (mut lx, mut ly, mut lz) = (-0.6_f32, 0.4_f32, -1.0_f32);
                let l_len = (lx * lx + ly * ly + lz * lz).sqrt();
                lx /= l_len;
                ly /= l_len;
                lz /= l_len;

                let mut phi: f32 = 0.0;
                while phi < 6.28 {
                    let mut theta: f32 = 0.0;
                    while theta < 3.14 {
                        let ox = theta.sin() * phi.cos();
                        let oy = theta.cos();
                        let oz = theta.sin() * phi.sin();

                        let mut pixel_char = '.';
                        let pixel_color;
                        let dist_to_button = ox * ox + oy * oy + (oz - 1.0) * (oz - 1.0);
                        let band = oy.abs() < 0.06;

                        if dist_to_button < 0.10 {
                            pixel_color = black;
                            pixel_char = '#';
                        } else if dist_to_button < 0.18 {
                            pixel_color = white;
                            pixel_char = '@';
                        } else if band {
                            pixel_color = black;
                            pixel_char = '#';
                        } else if oy < 0.0 {
                            pixel_color = red;
                        } else {
                            pixel_color = white;
                        }

                        let r = self.ball_scale;
                        let x = (ox * cos_a - oy * sin_a) * r;
                        let y = (ox * sin_a + oy * cos_a) * r;
                        let z = oz * r;

                        let mut y_final = y * cos_b - z * sin_b;
                        let z_final = y * sin_b + z * cos_b;
                        let x_final = x;
                        if self.open_amount > 0.0 && oy > 0.02 {
                            y_final += self.open_amount * 0.6;
                        }
                        if self.open_amount > 0.0 && oy.abs() < 0.03 {
                            theta += 0.03;
                            continue;
                        }

                        let camera_dist = 3.0;
                        let ooz = 1.0 / (z_final + camera_dist);

                        let xp = (self.width as f32 / 2.0
                            + self.ball_x
                            + 30.0 * ooz * x_final * self.aspect_ratio)
                            as i32;
                        let yp =
                            (self.height as f32 / 2.0 + self.ball_y + 18.0 * ooz * y_final) as i32;

                        if xp >= 0 && xp < self.width as i32 && yp >= 0 && yp < self.height as i32 {
                            let idx = (xp + yp * self.width as i32) as usize;
                            if ooz > zbuffer[idx] {
                                zbuffer[idx] = ooz;

                                if pixel_char == '@' || pixel_char == '#' {
                                    output[idx] = pixel_char;
                                } else {
                                    let dot = x_final * lx + y_final * ly + z_final * lz;
                                    let diffuse = dot.max(0.0);
                                    let rz = 2.0 * dot * z_final - lz;
                                    let spec = (rz * -1.0).max(0.0).powf(16.0);
                                    let shade = (0.12 + diffuse * 0.9 + spec * 0.6).min(1.0);

                                    let mut l_idx =
                                        (shade * (self.chars.len() - 1) as f32) as usize;
                                    if l_idx >= self.chars.len() {
                                        l_idx = self.chars.len() - 1;
                                    }
                                    output[idx] = self.chars.chars().nth(l_idx).unwrap();
                                }
                                color_buf[idx] = CellColor::Ansi(pixel_color);
                            }
                        }
                        theta += 0.03;
                    }
                    phi += 0.03;
                }
            }
        }

        if let Screen::Game = self.screen {
            if self.star_frame > 0 {
                render_starburst(
                    self.width as i32,
                    self.height as i32,
                    self.ball_x,
                    self.ball_y,
                    output,
                    color_buf,
                    zbuffer,
                    self.star_frame,
                );
                self.star_frame = self.star_frame.saturating_sub(1);
            }
        }

        if let Screen::Pokedex = self.screen {
            if let Some(notice) = self.pokedex_notice.as_ref() {
                if self.pokedex_notice_timer > 0 {
                    let row = self.height.saturating_sub(3);
                    let start_x = (self.width.saturating_sub(notice.len())) / 2;
                    for (i, ch) in notice.chars().enumerate() {
                        let x = start_x + i;
                        if x >= self.width || row >= self.height {
                            continue;
                        }
                        let idx = x + row * self.width;
                        output[idx] = ch;
                        color_buf[idx] = CellColor::Ansi("\x1b[91m");
                        zbuffer[idx] = 0.4;
                    }
                }
            }
        }

        if let Screen::Game = self.screen {
            if let Some(message) = self.caught_message.as_ref() {
                if self.caught_message_timer > 0 {
                    let row = self.height.saturating_sub(3);
                    let start_x = (self.width.saturating_sub(message.len())) / 2;
                    for (i, ch) in message.chars().enumerate() {
                        let x = start_x + i;
                        if x >= self.width || row >= self.height {
                            continue;
                        }
                        let idx = x + row * self.width;
                        output[idx] = ch;
                        color_buf[idx] = CellColor::Ansi("\x1b[92m");
                        zbuffer[idx] = 0.5;
                    }
                }
            }
        }

        if let Screen::Game = self.screen {
            if !self.agent_lines.is_empty() && self.agent_message_timer > 0 {
                let num_lines = self.agent_lines.len();
                let base_row = self.height.saturating_sub(2 + num_lines);
                let start_x = 3;
                for (li, line) in self.agent_lines.iter().enumerate() {
                    let row = base_row + li;
                    if row >= self.height {
                        continue;
                    }
                    for (i, ch) in line.chars().enumerate() {
                        let x = start_x + i;
                        if x >= self.width {
                            break;
                        }
                        let idx = x + row * self.width;
                        output[idx] = ch;
                        color_buf[idx] = CellColor::Ansi("\x1b[96m");
                        zbuffer[idx] = 0.9;
                    }
                }
            }
        }

        let prompt = match self.screen {
            Screen::Name => "enter a unique trainer name to begin catching (q to quit)",
            Screen::Pokedex => "type a caught number (1-151), or 'back' to return (q to quit)",
            Screen::PokedexDetail => "type 'back' to return to the pokedex (q to quit)",
            Screen::Game => {
                "type 'catch'/'pokedex' or ask a question like 'what is this pokemon?' (q to quit)"
            }
        };
        let prompt_line = format!("command: {} ({})", self.last_cmd, prompt);
        let prompt_row = self.height.saturating_sub(1);
        for x in 0..self.width {
            let idx = x + prompt_row * self.width;
            output[idx] = ' ';
            color_buf[idx] = CellColor::None;
        }
        for (x, ch) in prompt_line.chars().take(self.width).enumerate() {
            let idx = x + prompt_row * self.width;
            output[idx] = ch;
        }

        let _ = reset;
    }

    fn compose_frame(&self, buffers: &RenderBuffers) -> String {
        #[derive(Copy, Clone, PartialEq)]
        enum ActiveColor {
            None,
            Ansi(&'static str),
            Rgb(u8, u8, u8),
            Ansi256(u8),
        }

        let reset = "\x1b[0m";
        let mut frame = String::with_capacity(self.width * self.height * 4);
        frame.push_str("\x1b[H");
        let mut active = ActiveColor::None;

        for i in 0..self.height {
            for j in 0..self.width {
                let idx = j + i * self.width;
                let ch = buffers.output[idx];
                if ch == ' ' {
                    frame.push(' ');
                    continue;
                }

                match buffers.color_buf[idx] {
                    CellColor::None => {
                        if active != ActiveColor::None {
                            frame.push_str(reset);
                            active = ActiveColor::None;
                        }
                        frame.push(ch);
                    }
                    CellColor::Ansi(code) => {
                        if active != ActiveColor::Ansi(code) {
                            frame.push_str(code);
                            active = ActiveColor::Ansi(code);
                        }
                        frame.push(ch);
                    }
                    CellColor::Rgb(r, g, b) => match self.color_mode {
                        ColorMode::Truecolor => {
                            let desired = ActiveColor::Rgb(r, g, b);
                            if active != desired {
                                let _ = write!(frame, "\x1b[38;2;{};{};{}m", r, g, b);
                                active = desired;
                            }
                            frame.push(ch);
                        }
                        ColorMode::Ansi256 => {
                            let code = rgb_to_ansi256(r, g, b);
                            let desired = ActiveColor::Ansi256(code);
                            if active != desired {
                                let _ = write!(frame, "\x1b[38;5;{}m", code);
                                active = desired;
                            }
                            frame.push(ch);
                        }
                        ColorMode::Mono => {
                            if active != ActiveColor::None {
                                frame.push_str(reset);
                                active = ActiveColor::None;
                            }
                            frame.push(ch);
                        }
                    },
                }
            }
            if active != ActiveColor::None {
                frame.push_str(reset);
                active = ActiveColor::None;
            }
            if i + 1 < self.height {
                frame.push('\n');
            }
        }
        let _ = write!(frame, "\x1b[{};1H\x1b[J", self.height + 1);
        frame
    }

    fn refresh_daily_pokemon(&mut self, assets: &Assets) {
        if self.selection_mode != SelectionMode::DailyWeighted {
            return;
        }
        let Some(name) = self.trainer_name.as_ref() else {
            return;
        };
        let day = current_day_index_est();
        if day == self.daily_key {
            return;
        }
        self.daily_key = day;
        let legendary_unlocked = legendaries_unlocked(&self.pokedex, &assets.pokedex.names);
        let seed = daily_seed(name, day);
        self.pokemon_index =
            pick_weighted_pokemon(&assets.pokemons, &assets.pokedex, seed, legendary_unlocked);
        self.stream_particles.clear();
        self.caught_message = None;
        self.caught_message_timer = 0;
    }
}
const GEN1_CSV: &str = "sample_images/gen01.csv";
const POKEDEX_COLS: usize = 15;
const POKEDEX_ROWS: usize = 11;
const POKEDEX_CELL_W: usize = 9;
const POKEDEX_CELL_H: usize = 3;

#[tokio::main]
async fn main() -> io::Result<()> {
    init_db().await?;
    let pokedex = load_pokedex_view().unwrap_or_else(|err| {
        panic!("failed to load pokedex assets: {err}");
    });
    let pokemons = load_pokemon_assets(&pokedex.names, IMG_CHARSET);
    let assets = Arc::new(Assets {
        pokemons,
        arcanine_frames: pokemon::load_arcanine_frames(IMG_CHARSET),
        pokedex,
    });

    let selection_mode = selection_mode_from_env();
    let port = server_port_from_env(selection_mode);
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        let assets = Arc::clone(&assets);
        tokio::spawn(async move {
            let _ = run_session(stream, assets).await;
        });
    }
}

async fn run_session(stream: TcpStream, assets: Arc<Assets>) -> io::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    let reader_task = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    let _ = cmd_tx.send(String::from("__disconnect__"));
                    break;
                }
                Ok(_) => {
                    let _ = cmd_tx.send(line.clone());
                }
                Err(_) => {
                    let _ = cmd_tx.send(String::from("__disconnect__"));
                    break;
                }
            }
        }
    });

    let width = env_usize("POKESTREAM_WIDTH").unwrap_or(140);
    let height = env_usize("POKESTREAM_HEIGHT").unwrap_or(40);
    let color_mode = color_mode_from_env();
    let selection_mode = selection_mode_from_env();
    let mut session = SessionState::new(width, height, color_mode, selection_mode, &assets);
    let mut buffers = RenderBuffers::new(width, height);

    let (out_tx, out_rx) = mpsc::channel::<OutputMessage>(2);
    let writer_task = tokio::spawn(async move {
        let mut write_half = write_half;
        let mut out_rx = out_rx;
        while let Some(msg) = out_rx.recv().await {
            match msg {
                OutputMessage::Bytes(bytes) => {
                    if write_half.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                OutputMessage::Close { send_bye } => {
                    if send_bye {
                        let _ = close_session(&mut write_half).await;
                    } else {
                        let _ = cleanup_terminal(&mut write_half).await;
                        let _ = write_half.shutdown().await;
                    }
                    break;
                }
            }
        }
    });

    let _ = out_tx
        .send(OutputMessage::Bytes(
            b"\x1b[?1049h\x1b[?7l\x1b[2J\x1b[H\x1b[?25l".to_vec(),
        ))
        .await;
    let _ = out_tx
        .send(OutputMessage::Bytes(b"\x1b[2J\x1b[H\x1b[?25l".to_vec()))
        .await;

    let frame_interval = frame_interval_from_env();
    let mut ticker = time::interval(frame_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_frame_hash: u64 = 0;

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => {
                let cmd = match maybe_cmd {
                    Some(cmd) => cmd,
                    None => String::from("__disconnect__"),
                };
                match session.handle_command(&cmd, &assets).await {
                    CommandAction::Exit => {
                        reader_task.abort();
                        let _ = reader_task.await;
                        let _ = out_tx.send(OutputMessage::Close { send_bye: true }).await;
                        break;
                    }
                    CommandAction::Disconnect => {
                        reader_task.abort();
                        let _ = reader_task.await;
                        let _ = out_tx.send(OutputMessage::Close { send_bye: false }).await;
                        break;
                    }
                    CommandAction::None => {}
                }

                if out_tx.capacity() > 0 {
                    session.render(&assets, &mut buffers);
                    let frame = session.compose_frame(&buffers);
                    let hash = fast_hash(frame.as_bytes());
                    if hash != last_frame_hash {
                        last_frame_hash = hash;
                        let _ = out_tx.try_send(OutputMessage::Bytes(frame.into_bytes()));
                    }
                }
            }
            _ = ticker.tick() => {
                if out_tx.capacity() == 0 {
                    continue;
                }
                session.update(&assets).await;
                session.render(&assets, &mut buffers);
                let frame = session.compose_frame(&buffers);
                let hash = fast_hash(frame.as_bytes());
                if hash != last_frame_hash {
                    last_frame_hash = hash;
                    let _ = out_tx.try_send(OutputMessage::Bytes(frame.into_bytes()));
                }
            }
        }
    }

    let _ = writer_task.await;
    Ok(())
}

async fn cleanup_terminal(write_half: &mut OwnedWriteHalf) -> io::Result<()> {
    write_half
        .write_all(b"\x1b[0m\x1b[?25h\x1b[?7h\x1b[?1049l")
        .await
}

async fn close_session(write_half: &mut OwnedWriteHalf) -> io::Result<()> {
    cleanup_terminal(write_half).await?;
    write_half.write_all(b"bye\r\n").await?;
    write_half.shutdown().await
}

fn build_stream_particles(
    pokemon: &PokemonAsset,
    start_x: usize,
    start_y: usize,
    target_x: f32,
    target_y: f32,
) -> Vec<StreamParticle> {
    let mut particles = Vec::new();
    let mut idx: u16 = 0;
    for y in 0..pokemon.image.height {
        for x in 0..pokemon.image.width {
            let src_idx = x + y * pokemon.image.width;
            let ch = pokemon.image.chars[src_idx];
            if ch == ' ' {
                continue;
            }
            let color = pokemon.image.colors[src_idx];
            particles.push(StreamParticle {
                x0: (start_x + x) as f32,
                y0: (start_y + y) as f32,
                x1: target_x,
                y1: target_y,
                ch,
                color,
                start_frame: idx % 12,
            });
            idx = idx.wrapping_add(1);
        }
    }
    particles
}

fn render_stream(
    particles: &[StreamParticle],
    frame: u16,
    output: &mut [char],
    color_buf: &mut [CellColor],
    zbuffer: &mut [f32],
    width: usize,
    height: usize,
) {
    for particle in particles {
        if frame < particle.start_frame {
            continue;
        }
        let t = (frame - particle.start_frame) as f32 / ABSORB_FRAMES as f32;
        let t = t.min(1.0);
        let x = particle.x0 + (particle.x1 - particle.x0) * t;
        let y = particle.y0 + (particle.y1 - particle.y0) * t;
        let xi = x.round() as i32;
        let yi = y.round() as i32;
        if xi < 0 || yi < 0 || xi >= width as i32 || yi >= height as i32 {
            continue;
        }
        let idx = (xi + yi * width as i32) as usize;
        if 0.6 > zbuffer[idx] {
            zbuffer[idx] = 0.6;
            output[idx] = particle.ch;
            let (r, g, b) = particle.color;
            color_buf[idx] = CellColor::Rgb(r, g, b);
        }
    }
}

fn render_starburst(
    width: i32,
    height: i32,
    ball_x: f32,
    ball_y: f32,
    output: &mut [char],
    color_buf: &mut [CellColor],
    zbuffer: &mut [f32],
    frame: u16,
) {
    let center_x = (width as f32 / 2.0 + ball_x).round() as i32;
    let center_y = (height as f32 / 2.0 + ball_y).round() as i32 - 10;
    let t = frame as f32 / STAR_FRAMES as f32;
    let spread = (1.0 - t) * 11.0;
    let stars = [
        (0.0, -1.0),
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.8, -0.6),
        (-0.8, -0.6),
        (0.8, 0.6),
        (-0.8, 0.6),
        (1.2, -0.2),
        (-1.2, -0.2),
        (1.2, 0.2),
        (-1.2, 0.2),
        (0.0, -1.6),
        (0.0, -2.0),
        (0.0, -2.4),
        (0.4, -1.8),
        (-0.4, -1.8),
    ];
    for (dx, dy) in stars {
        let x = center_x + (dx * spread) as i32;
        let y = center_y + (dy * spread) as i32;
        if x < 0 || y < 0 || x >= width || y >= height {
            continue;
        }
        let idx = (x + y * width) as usize;
        output[idx] = '*';
        color_buf[idx] = CellColor::Ansi("\x1b[93m");
        zbuffer[idx] = 0.9;
    }
}

fn color_mode_from_env() -> ColorMode {
    if let Ok(mode) = env::var("POKESTREAM_COLOR") {
        let mode = mode.to_lowercase();
        if mode == "truecolor" || mode == "24bit" {
            return ColorMode::Truecolor;
        }
        if mode == "ansi256" || mode == "256" {
            return ColorMode::Ansi256;
        }
        if mode == "mono" || mode == "none" {
            return ColorMode::Mono;
        }
    }
    if let Ok(ct) = env::var("COLORTERM") {
        let ct = ct.to_lowercase();
        if ct.contains("truecolor") || ct.contains("24bit") {
            return ColorMode::Truecolor;
        }
    }
    if let Ok(term) = env::var("TERM") {
        let term = term.to_lowercase();
        if term.contains("truecolor") || term.contains("24bit") || term.contains("direct") {
            return ColorMode::Truecolor;
        }
        if term.contains("256color") {
            return ColorMode::Ansi256;
        }
    }
    ColorMode::Ansi256
}

fn env_usize(key: &str) -> Option<usize> {
    env::var(key).ok().and_then(|val| val.parse::<usize>().ok())
}

fn selection_mode_from_env() -> SelectionMode {
    if let Ok(mode) = env::var("POKESTREAM_MODE") {
        let mode = mode.to_lowercase();
        if matches!(
            mode.as_str(),
            "random" | "dev_random" | "random_per_session"
        ) {
            return SelectionMode::RandomPerSession;
        }
    }
    if let Ok(raw) = env::var("POKESTREAM_RANDOM") {
        let raw = raw.to_lowercase();
        if matches!(raw.as_str(), "1" | "true" | "yes") {
            return SelectionMode::RandomPerSession;
        }
    }
    SelectionMode::DailyWeighted
}

fn server_port_from_env(selection_mode: SelectionMode) -> u16 {
    if let Ok(raw) = env::var("POKESTREAM_PORT") {
        if let Ok(port) = raw.parse::<u16>() {
            return port;
        }
    }
    match selection_mode {
        SelectionMode::RandomPerSession => 8081,
        SelectionMode::DailyWeighted => 8080,
    }
}

fn frame_interval_from_env() -> Duration {
    if let Ok(raw) = env::var("POKESTREAM_FPS") {
        if let Ok(fps) = raw.parse::<u64>() {
            if fps > 0 {
                let ms = (1000 / fps).max(10);
                return Duration::from_millis(ms);
            }
        }
    }
    if let Ok(raw) = env::var("POKESTREAM_FRAME_MS") {
        if let Ok(ms) = raw.parse::<u64>() {
            if ms > 0 {
                return Duration::from_millis(ms);
            }
        }
    }
    Duration::from_millis(30)
}

fn current_day_index_est() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let shifted = secs - 5 * 3600;
    shifted.div_euclid(86_400)
}

fn daily_seed(trainer: &str, day_index: i64) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in trainer.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for b in day_index.to_le_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

fn is_legendary(name: &str) -> bool {
    matches!(name, "articuno" | "zapdos" | "moltres" | "mewtwo" | "mew")
}

fn is_starter(name: &str) -> bool {
    matches!(
        name,
        "bulbasaur" | "charmander" | "squirtle" | "pikachu" | "eevee"
    )
}

fn legendaries_unlocked(pokedex: &HashSet<String>, names: &[String]) -> bool {
    for name in names {
        if name.is_empty() || is_legendary(name) {
            continue;
        }
        if !pokedex.contains(name) {
            return false;
        }
    }
    true
}

fn pokemon_weight(name: &str, total: Option<u16>, legendary_unlocked: bool) -> u32 {
    if is_legendary(name) && !legendary_unlocked {
        return 0;
    }
    let total = total.unwrap_or(350) as i32;
    let mut weight = (600 - total).clamp(20, 500) as f32;

    if total >= 500 {
        weight *= 0.35;
    } else if total >= 450 {
        weight *= 0.55;
    } else if total >= 400 {
        weight *= 0.75;
    } else if total <= 300 {
        weight *= 1.25;
    }

    if is_starter(name) {
        weight *= 0.8;
    }

    if is_legendary(name) {
        weight *= 0.05;
    }

    weight.max(1.0) as u32
}

fn pick_weighted_pokemon(
    pokemons: &[PokemonAsset],
    pokedex_view: &PokedexView,
    seed: u64,
    legendary_unlocked: bool,
) -> usize {
    let mut weights = Vec::with_capacity(pokemons.len());
    for (idx, pokemon) in pokemons.iter().enumerate() {
        let total = pokedex_view.totals_by_name.get(&pokemon.name).copied();
        let weight = pokemon_weight(&pokemon.name, total, legendary_unlocked);
        if weight > 0 {
            weights.push((idx, weight));
        }
    }
    if weights.is_empty() {
        return 0;
    }

    let total_weight: u64 = weights.iter().map(|(_, w)| *w as u64).sum();
    let mut rng = seed;
    let roll = next_u64(&mut rng) % total_weight;
    let mut acc = 0u64;
    for (idx, weight) in weights {
        acc += weight as u64;
        if roll < acc {
            return idx;
        }
    }
    0
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // 16-231: 6x6x6 color cube, 232-255: grayscale
    let r = r as u16;
    let g = g as u16;
    let b = b as u16;
    let gray = (r + g + b) / 3;
    if gray > 8
        && gray < 248
        && (r as i16 - g as i16).abs() < 12
        && (r as i16 - b as i16).abs() < 12
    {
        let gray_index = ((gray - 8) * 24 / 247) as u8;
        return 232 + gray_index;
    }
    let rc = (r * 5 / 255) as u8;
    let gc = (g * 5 / 255) as u8;
    let bc = (b * 5 / 255) as u8;
    16 + 36 * rc + 6 * gc + bc
}

fn pick_pokemon_index(pokemons: &[PokemonAsset]) -> usize {
    if pokemons.is_empty() {
        panic!("no pokemon assets loaded");
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (nanos % pokemons.len() as u128) as usize
}

fn load_pokedex_view() -> io::Result<PokedexView> {
    let (names, totals_by_name) = load_gen1_data(GEN1_CSV)?;
    Ok(PokedexView {
        names,
        totals_by_name,
    })
}

fn load_pokemon_assets(names: &[String], charset: &str) -> Vec<PokemonAsset> {
    let mut assets = Vec::new();
    for name in names {
        if name.is_empty() {
            continue;
        }
        let image = pokemon::load_named_pokemon(name, charset);
        assets.push(PokemonAsset {
            name: name.clone(),
            image,
        });
    }
    assets
}

fn load_gen1_data(path: &str) -> io::Result<(Vec<String>, HashMap<String, u16>)> {
    let data = fs::read_to_string(path)?;
    let mut names = vec![String::new(); 151];
    let mut totals_by_name = HashMap::new();
    for (i, line) in data.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let fields = parse_csv_line(line);
        if fields.len() < 6 {
            continue;
        }
        let id: usize = match fields[0].trim().parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if id == 0 || id > 151 {
            continue;
        }
        let name = fields[1].trim();
        let form = fields[2].trim();
        let total: u16 = fields[5].trim().parse().unwrap_or(0);
        let normalized = normalize_pokemon_name(name, form);
        names[id - 1] = normalized.clone();
        if total > 0 {
            totals_by_name.insert(normalized, total);
        }
    }
    Ok((names, totals_by_name))
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                out.push(buf.clone());
                buf.clear();
            }
            _ => buf.push(ch),
        }
    }
    out.push(buf);
    out
}

fn normalize_pokemon_name(name: &str, form: &str) -> String {
    let mut base = name.trim().to_lowercase();
    base = base.replace('.', "");
    base = base.replace('\'', "");
    base = base.replace(' ', "-");
    let form = form.trim();
    if !form.is_empty() && form != " " {
        let form = form.to_lowercase().replace(' ', "-");
        if base == "nidoran" {
            if form == "female" {
                return "nidoran-f".to_string();
            }
            if form == "male" {
                return "nidoran-m".to_string();
            }
        }
        return format!("{}-{}", base, form);
    }
    base
}

fn display_pokemon_name(name: &str) -> String {
    let mut out = String::new();
    for (i, part) in name.split('-').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
}

fn is_agent_stats_query(phrase: &str) -> bool {
    let phrase = phrase.trim();
    phrase.contains("stats")
        || phrase.contains("base stat")
        || (phrase.contains("hp") && phrase.contains("atk"))
        || phrase.contains("how strong")
        || phrase.contains("what are its stats")
        || phrase.contains("show me stats")
}

fn is_agent_pokemon_query(phrase: &str) -> bool {
    let phrase = phrase.trim();
    phrase.contains("what pokemon is this")
        || phrase.contains("who is this pokemon")
        || phrase.contains("what is this creature")
        || phrase.contains("what is this pokemon")
        || phrase.contains("identify this")
        || phrase.contains("tell me about this")
        || phrase.contains("what the hell is this")
}

fn is_agent_left_to_catch_query(phrase: &str) -> bool {
    let phrase = phrase.trim();
    phrase.contains("how many")
        && (phrase.contains("left")
            || phrase.contains("remaining")
            || phrase.contains("to catch")
            || phrase.contains("missing"))
}

fn is_agent_caught_count_query(phrase: &str) -> bool {
    let phrase = phrase.trim();
    (phrase.contains("how many") || phrase.contains("count"))
        && (phrase.contains("caught") || phrase.contains("have i"))
}

fn is_agent_missing_list_query(phrase: &str) -> bool {
    let phrase = phrase.trim();
    phrase.contains("which pokemon am i missing")
        || phrase.contains("what am i missing")
        || (phrase.contains("show") && phrase.contains("missing"))
}

fn looks_like_agent_query_candidate(text: &str) -> bool {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return false;
    }
    if matches!(
        text.as_str(),
        "catch" | "pokedex" | "dex" | "back" | "q" | "quit" | "exit"
    ) {
        return false;
    }
    if text.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    text.ends_with('?')
        || text.contains(' ')
        || matches!(text.as_str(), "help" | "status" | "progress" | "missing")
}

fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.len() + 1 + word.len() <= max_width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn fast_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_text(text: &str, max_chars: usize) -> String {
    let len = text.chars().count();
    if len <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }
    let mut out = String::new();
    for ch in text.chars().take(max_chars - 3) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn missing_pokemon_preview(caught: &HashSet<String>, assets: &Assets, limit: usize) -> Vec<String> {
    assets
        .pokedex
        .names
        .iter()
        .filter(|name| !name.is_empty() && !caught.contains(*name))
        .take(limit)
        .map(|name| display_pokemon_name(name))
        .collect()
}

fn ollama_url() -> String {
    env::var("OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string())
}

fn ollama_model() -> String {
    env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5:1.5b".to_string())
}

async fn ask_llm_brief(
    user_query: &str,
    screen: &str,
    trainer_name: Option<&str>,
    current_pokemon: Option<&str>,
    caught_count: usize,
    total_count: usize,
    left_count: usize,
) -> Option<String> {
    let prompt = format!(
        "You are Pokestream Agent inside a telnet Pokemon game. \
        Keep responses concise, under 200 characters. Only answer in-game questions. \
        Context: screen={screen}, trainer={}, pokemon={}, caught={caught_count}/{total_count}, left={left_count}.\n\
        User: {user_query}",
        trainer_name.unwrap_or("unknown"),
        current_pokemon.unwrap_or("unknown"),
    );

    let payload = serde_json::json!({
        "model": ollama_model(),
        "prompt": prompt,
        "stream": false,
        "keep_alive": "10m",
        "options": {
            "temperature": 0.3,
            "num_predict": 40,
        }
    });

    let url = format!("{}/api/generate", ollama_url());
    let client = shared_http_client();
    let response = match tokio::time::timeout(
        Duration::from_secs(8),
        client.post(&url).json(&payload).send(),
    )
    .await
    {
        Ok(Ok(resp)) => resp.error_for_status().ok()?,
        _ => return None,
    };
    let body: serde_json::Value = response.json().await.ok()?;

    let text = body.get("response").and_then(|v| v.as_str())?;
    let cleaned = normalize_whitespace(text);
    if cleaned.is_empty() {
        return None;
    }

    let answer = format!("Agent: {}", cleaned);
    Some(clip_text(&normalize_whitespace(&answer), 220))
}

async fn fetch_pokemon_brief(name: &str) -> Option<String> {
    if let Ok(cache) = pokemon_cache().lock() {
        if let Some(cached) = cache.get(name) {
            return Some(cached.clone());
        }
    }

    let client = shared_http_client();
    let url = format!("https://pokeapi.co/api/v2/pokemon/{name}");
    let pokemon: serde_json::Value = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    let mut type_slots: Vec<(i64, String)> = Vec::new();
    if let Some(types) = pokemon.get("types").and_then(|v| v.as_array()) {
        for entry in types {
            let slot = entry.get("slot").and_then(|v| v.as_i64()).unwrap_or(99);
            if let Some(type_name) = entry
                .get("type")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                type_slots.push((slot, type_name.to_string()));
            }
        }
    }
    type_slots.sort_by_key(|(slot, _)| *slot);
    let type_text = if type_slots.is_empty() {
        "unknown".to_string()
    } else {
        type_slots
            .into_iter()
            .map(|(_, ty)| ty)
            .collect::<Vec<_>>()
            .join("/")
    };

    let mut hp = None;
    let mut atk = None;
    let mut def = None;
    if let Some(stats) = pokemon.get("stats").and_then(|v| v.as_array()) {
        for stat in stats {
            let Some(stat_name) = stat
                .get("stat")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let Some(value) = stat.get("base_stat").and_then(|v| v.as_i64()) else {
                continue;
            };
            match stat_name {
                "hp" => hp = Some(value),
                "attack" => atk = Some(value),
                "defense" => def = Some(value),
                _ => {}
            }
        }
    }

    let flavor = if let Some(species_url) = pokemon
        .get("species")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
    {
        fetch_species_flavor(&client, species_url).await
    } else {
        None
    };

    let message = if let Some(flavor) = flavor {
        format!(
            "Agent: {} is a {}-type Pokemon. {}",
            display_pokemon_name(name),
            type_text,
            flavor
        )
    } else {
        format!(
            "Agent: {} is a {}-type Pokemon.",
            display_pokemon_name(name),
            type_text
        )
    };
    let result = normalize_whitespace(&message);
    if let Ok(mut cache) = pokemon_cache().lock() {
        cache.insert(name.to_string(), result.clone());
    }
    Some(result)
}

async fn fetch_pokemon_stats(name: &str) -> Option<String> {
    let client = shared_http_client();
    let url = format!("https://pokeapi.co/api/v2/pokemon/{name}");
    let pokemon: serde_json::Value = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    let mut stats_list = Vec::new();
    if let Some(stats) = pokemon.get("stats").and_then(|v| v.as_array()) {
        for stat in stats {
            let stat_name = stat
                .get("stat")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let value = stat.get("base_stat").and_then(|v| v.as_i64()).unwrap_or(0);
            let label = match stat_name {
                "hp" => "HP",
                "attack" => "ATK",
                "defense" => "DEF",
                "special-attack" => "SP.ATK",
                "special-defense" => "SP.DEF",
                "speed" => "SPD",
                other => other,
            };
            stats_list.push(format!("{label} {value}"));
        }
    }
    let message = format!(
        "Agent: {} stats: {}.",
        display_pokemon_name(name),
        stats_list.join(", ")
    );
    Some(clip_text(&normalize_whitespace(&message), 400))
}

async fn fetch_species_flavor(client: &reqwest::Client, species_url: &str) -> Option<String> {
    let species: serde_json::Value = client
        .get(species_url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let entries = species.get("flavor_text_entries")?.as_array()?;
    for entry in entries {
        if entry
            .get("language")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            != Some("en")
        {
            continue;
        }
        let Some(raw) = entry.get("flavor_text").and_then(|v| v.as_str()) else {
            continue;
        };
        let cleaned = raw.replace(['\n', '\r', '\u{000c}'], " ");
        let cleaned = normalize_whitespace(&cleaned);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    None
}

fn find_pokemon_asset<'a>(assets: &'a Assets, name: &str) -> Option<&'a PokemonAsset> {
    assets.pokemons.iter().find(|asset| asset.name == name)
}

fn render_pokedex(
    view: &PokedexView,
    caught: &HashSet<String>,
    output: &mut [char],
    color_buf: &mut [CellColor],
    zbuffer: &mut [f32],
    width: usize,
    height: usize,
) {
    let grid_w = POKEDEX_COLS * POKEDEX_CELL_W;
    let grid_h = POKEDEX_ROWS * POKEDEX_CELL_H;
    let start_x = (width.saturating_sub(grid_w)) / 2;
    let start_y = (height.saturating_sub(grid_h)) / 2;

    for idx in 0..151 {
        let row = idx / POKEDEX_COLS;
        let col = idx % POKEDEX_COLS;
        let base_x = start_x + col * POKEDEX_CELL_W;
        let base_y = start_y + row * POKEDEX_CELL_H;
        let number = idx + 1;
        let digits: Vec<char> = number.to_string().chars().collect();
        let number_w = digits.len();
        let offset_x = base_x + (POKEDEX_CELL_W.saturating_sub(number_w)) / 2;
        let offset_y = base_y;

        let name = view.names.get(idx).map(|s| s.as_str()).unwrap_or("");
        let caught_entry = !name.is_empty() && caught.contains(name);
        let main = if caught_entry { "\x1b[91m" } else { "\x1b[97m" };

        for (d, digit) in digits.iter().enumerate() {
            let target_x = offset_x + d;
            let target_y = offset_y;
            if target_x >= width || target_y >= height {
                continue;
            }
            let idx = target_x + target_y * width;
            output[idx] = *digit;
            color_buf[idx] = CellColor::Ansi(main);
            zbuffer[idx] = 0.35;
        }
    }
}

fn render_pokedex_detail(
    assets: &Assets,
    idx: usize,
    output: &mut [char],
    color_buf: &mut [CellColor],
    zbuffer: &mut [f32],
    width: usize,
    height: usize,
) {
    let name = assets
        .pokedex
        .names
        .get(idx)
        .map(|s| s.as_str())
        .unwrap_or("");
    let display_name = if name.is_empty() {
        "Unknown Pokemon".to_string()
    } else {
        display_pokemon_name(name)
    };

    if let Some(asset) = find_pokemon_asset(assets, name) {
        let image = &asset.image;
        let start_x = (width.saturating_sub(image.width)) / 2;
        let start_y = (height.saturating_sub(image.height + 2)) / 2;

        for y in 0..image.height {
            for x in 0..image.width {
                let target_y = start_y + y;
                let target_x = start_x + x;
                if target_y >= height || target_x >= width {
                    continue;
                }
                let src_idx = x + y * image.width;
                let ch = image.chars[src_idx];
                if ch == ' ' {
                    continue;
                }
                let idx = target_x + target_y * width;
                output[idx] = ch;
                let (r, g, b) = image.colors[src_idx];
                color_buf[idx] = CellColor::Rgb(r, g, b);
                zbuffer[idx] = 0.4;
            }
        }

        let name_row = (start_y + image.height + 1).min(height.saturating_sub(2));
        let name_start = (width.saturating_sub(display_name.len())) / 2;
        for (i, ch) in display_name.chars().enumerate() {
            let x = name_start + i;
            if x >= width || name_row >= height {
                continue;
            }
            let idx = x + name_row * width;
            output[idx] = ch;
            color_buf[idx] = CellColor::Ansi("\x1b[92m");
            zbuffer[idx] = 0.4;
        }
    } else {
        let notice = "Sprite not available yet";
        let notice_row = height.saturating_sub(3);
        let notice_start = (width.saturating_sub(notice.len())) / 2;
        for (i, ch) in notice.chars().enumerate() {
            let x = notice_start + i;
            if x >= width || notice_row >= height {
                continue;
            }
            let idx = x + notice_row * width;
            output[idx] = ch;
            color_buf[idx] = CellColor::Ansi("\x1b[97m");
            zbuffer[idx] = 0.4;
        }

        let name_row = notice_row.saturating_sub(2);
        let name_start = (width.saturating_sub(display_name.len())) / 2;
        for (i, ch) in display_name.chars().enumerate() {
            let x = name_start + i;
            if x >= width || name_row >= height {
                continue;
            }
            let idx = x + name_row * width;
            output[idx] = ch;
            color_buf[idx] = CellColor::Ansi("\x1b[92m");
            zbuffer[idx] = 0.4;
        }
    }
}

fn sanitize_trainer_name(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > 16 {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(trimmed.to_lowercase())
}

async fn init_db() -> io::Result<()> {
    task::spawn_blocking(|| -> io::Result<()> {
        let conn =
            Connection::open(DB_PATH).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS trainers (
                name TEXT PRIMARY KEY,
                pokedex TEXT NOT NULL
            )",
            [],
        )
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        Ok(())
    })
    .await
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
}

async fn load_pokedex(name: &str) -> io::Result<HashSet<String>> {
    let name = name.to_string();
    task::spawn_blocking(move || -> io::Result<HashSet<String>> {
        let conn =
            Connection::open(DB_PATH).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        let json: Option<String> = conn
            .query_row(
                "SELECT pokedex FROM trainers WHERE name = ?1",
                [&name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        let list: Vec<String> = match json {
            Some(text) => serde_json::from_str(&text)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?,
            None => Vec::new(),
        };
        Ok(list.into_iter().collect())
    })
    .await
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
}

async fn save_pokedex(name: &str, pokedex: &HashSet<String>) -> io::Result<()> {
    let name = name.to_string();
    let mut list: Vec<String> = pokedex.iter().cloned().collect();
    list.sort();
    let payload =
        serde_json::to_string(&list).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    task::spawn_blocking(move || -> io::Result<()> {
        let conn =
            Connection::open(DB_PATH).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        conn.execute(
            "INSERT INTO trainers (name, pokedex)
             VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET pokedex = excluded.pokedex",
            (&name, &payload),
        )
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        Ok(())
    })
    .await
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
}
