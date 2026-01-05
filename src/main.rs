mod ascii;
mod pokemon;

use std::collections::HashSet;
use std::fmt::Write;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::fs;

use rusqlite::{Connection, OptionalExtension};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use tokio::task;

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
    name: &'static str,
    image: ascii::AsciiImage,
}

struct PokedexView {
    names: Vec<String>,
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

struct SessionState {
    width: usize,
    height: usize,
    aspect_ratio: f32,
    chars: &'static str,
    pokemon_index: usize,
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
}

impl SessionState {
    fn new(width: usize, height: usize, color_mode: ColorMode, assets: &Assets) -> Self {
        Self {
            width,
            height,
            aspect_ratio: 1.5,
            chars: " .:-=+*#%@",
            pokemon_index: pick_pokemon_index(&assets.pokemons),
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
        }
    }

    fn pokemon<'a>(&self, assets: &'a Assets) -> &'a PokemonAsset {
        &assets.pokemons[self.pokemon_index]
    }

    async fn handle_command(&mut self, cmd: &str, assets: &Assets) -> CommandAction {
        let cmd_trim = cmd.trim().to_lowercase();
        if cmd.as_bytes().contains(&3) || matches!(cmd_trim.as_str(), "q" | "quit" | "exit") {
            return CommandAction::Exit;
        }
        if cmd_trim == "__disconnect__" {
            return CommandAction::Disconnect;
        }
        self.last_cmd = cmd_trim.clone();
        match self.screen {
            Screen::Name => {
                if let Some(name) = sanitize_trainer_name(&cmd_trim) {
                    self.pokedex = load_pokedex(&name).await.unwrap_or_default();
                    self.trainer_name = Some(name);
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
                }
            }
            Screen::PokedexDetail => {
                if cmd_trim == "back" {
                    self.screen = Screen::Pokedex;
                }
            }
            Screen::Game => {
                if cmd_trim == "catch" && self.state == GameState::Idle {
                    self.state = GameState::Throwing;
                    self.frame_count = 0;
                    self.capture_recorded = false;
                }
                if cmd_trim == "pokedex" || cmd_trim == "dex" {
                    self.screen = Screen::Pokedex;
                    self.pokedex_detail = None;
                    self.pokedex_notice = None;
                    self.pokedex_notice_timer = 0;
                }
            }
        }
        CommandAction::None
    }

    async fn update(&mut self, assets: &Assets) {
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
                                if self.pokedex.insert(self.pokemon(assets).name.to_string()) {
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
                                display_pokemon_name(self.pokemon(assets).name)
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
                        self.welcome_frame = (self.welcome_frame + 1) % assets.arcanine_frames.len();
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
                    let frame = &assets.arcanine_frames[self.welcome_frame % assets.arcanine_frames.len()];
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
                if matches!(self.state, GameState::Idle | GameState::Throwing | GameState::Opening) {
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
                            + 30.0 * ooz * x_final * self.aspect_ratio) as i32;
                        let yp = (self.height as f32 / 2.0
                            + self.ball_y
                            + 18.0 * ooz * y_final) as i32;

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

                                    let mut l_idx = (shade * (self.chars.len() - 1) as f32) as usize;
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

        let prompt = match self.screen {
            Screen::Name => "trainer name + Enter",
            Screen::Pokedex => "number + Enter or 'back' + Enter",
            Screen::PokedexDetail => "type 'back' + Enter",
            Screen::Game => "type 'catch' + Enter or 'pokedex' + Enter",
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
        let reset = "\x1b[0m";
        let mut frame = String::with_capacity(self.width * self.height * 6);
        frame.push_str("\x1b[H");
        for i in 0..self.height {
            for j in 0..self.width {
                let idx = j + i * self.width;
                if buffers.output[idx] == ' ' {
                    frame.push(' ');
                } else {
                    match buffers.color_buf[idx] {
                        CellColor::None => frame.push(buffers.output[idx]),
                        CellColor::Ansi(code) => {
                            frame.push_str(code);
                            frame.push(buffers.output[idx]);
                            frame.push_str(reset);
                        }
                        CellColor::Rgb(r, g, b) => {
                            match self.color_mode {
                                ColorMode::Truecolor => {
                                    let _ = write!(
                                        frame,
                                        "\x1b[38;2;{};{};{}m{}",
                                        r,
                                        g,
                                        b,
                                        buffers.output[idx]
                                    );
                                    frame.push_str(reset);
                                }
                                ColorMode::Ansi256 => {
                                    let code = rgb_to_ansi256(r, g, b);
                                    let _ = write!(frame, "\x1b[38;5;{}m{}", code, buffers.output[idx]);
                                    frame.push_str(reset);
                                }
                                ColorMode::Mono => {
                                    frame.push(buffers.output[idx]);
                                }
                            }
                        }
                    }
                }
            }
            if i + 1 < self.height {
                frame.push('\n');
            }
        }
        let _ = write!(frame, "\x1b[{};1H\x1b[J", self.height + 1);
        frame
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
    let assets = Arc::new(Assets {
        pokemons: vec![
            PokemonAsset {
                name: "bulbasaur",
                image: pokemon::load_bulbasaur(IMG_CHARSET),
            },
            PokemonAsset {
                name: "ivysaur",
                image: pokemon::load_ivysaur(IMG_CHARSET),
            },
            PokemonAsset {
                name: "venusaur",
                image: pokemon::load_venusaur(IMG_CHARSET),
            },
            PokemonAsset {
                name: "charmander",
                image: pokemon::load_charmander(IMG_CHARSET),
            },
            PokemonAsset {
                name: "charmeleon",
                image: pokemon::load_charmeleon(IMG_CHARSET),
            },
            PokemonAsset {
                name: "charizard",
                image: pokemon::load_charizard(IMG_CHARSET),
            },
            PokemonAsset {
                name: "squirtle",
                image: pokemon::load_squirtle(IMG_CHARSET),
            },
            PokemonAsset {
                name: "wartortle",
                image: pokemon::load_wartortle(IMG_CHARSET),
            },
            PokemonAsset {
                name: "blastoise",
                image: pokemon::load_blastoise(IMG_CHARSET),
            },
            PokemonAsset {
                name: "growlithe",
                image: pokemon::load_growlithe(IMG_CHARSET),
            },
            PokemonAsset {
                name: "pikachu",
                image: pokemon::load_pikachu(IMG_CHARSET),
            },
        ],
        arcanine_frames: pokemon::load_arcanine_frames(IMG_CHARSET),
        pokedex,
    });

    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let assets = Arc::clone(&assets);
        tokio::spawn(async move {
            let _ = run_session(stream, assets).await;
        });
    }
}

async fn run_session(stream: TcpStream, assets: Arc<Assets>) -> io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    write_half
        .write_all(b"\x1b[?1049h\x1b[?7l\x1b[2J\x1b[H\x1b[?25l")
        .await?;

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
    let mut session = SessionState::new(width, height, color_mode, &assets);
    let mut buffers = RenderBuffers::new(width, height);

    write_half.write_all(b"\x1b[2J\x1b[H\x1b[?25l").await?;

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match session.handle_command(&cmd, &assets).await {
                CommandAction::Exit => {
                    reader_task.abort();
                    let _ = reader_task.await;
                    let _ = close_session(&mut write_half).await;
                    return Ok(());
                }
                CommandAction::Disconnect => {
                    reader_task.abort();
                    let _ = reader_task.await;
                    let _ = cleanup_terminal(&mut write_half).await;
                    return Ok(());
                }
                CommandAction::None => {}
            }
        }

        session.update(&assets).await;
        session.render(&assets, &mut buffers);
        let frame = session.compose_frame(&buffers);
        write_half.write_all(frame.as_bytes()).await?;

        time::sleep(Duration::from_millis(30)).await;
    }
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


fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // 16-231: 6x6x6 color cube, 232-255: grayscale
    let r = r as u16;
    let g = g as u16;
    let b = b as u16;
    let gray = (r + g + b) / 3;
    if gray > 8 && gray < 248 && (r as i16 - g as i16).abs() < 12 && (r as i16 - b as i16).abs() < 12 {
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
    let names = load_gen1_names(GEN1_CSV)?;
    Ok(PokedexView {
        names,
    })
}

fn load_gen1_names(path: &str) -> io::Result<Vec<String>> {
    let data = fs::read_to_string(path)?;
    let mut names = vec![String::new(); 151];
    for (i, line) in data.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let fields = parse_csv_line(line);
        if fields.len() < 3 {
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
        names[id - 1] = normalize_pokemon_name(name, form);
    }
    Ok(names)
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
        let conn = Connection::open(DB_PATH)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
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
        let conn = Connection::open(DB_PATH)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        let json: Option<String> = conn
            .query_row("SELECT pokedex FROM trainers WHERE name = ?1", [&name], |row| {
                row.get(0)
            })
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
    let payload = serde_json::to_string(&list)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    task::spawn_blocking(move || -> io::Result<()> {
        let conn = Connection::open(DB_PATH)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
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
