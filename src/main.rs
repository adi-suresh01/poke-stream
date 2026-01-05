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
    Game,
}

#[derive(Copy, Clone, PartialEq)]
enum ColorMode {
    Truecolor,
    Ansi256,
    Mono,
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
const SHAKE_FRAMES: u16 = 20;
const SHAKE_COUNT: u8 = 3;
const STAR_FRAMES: u16 = 18;
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
    let aspect_ratio = 1.5;

    let chars = " .:-=+*#%@";
    let pokemon = pick_pokemon(&assets.pokemons);
    let mut trainer_name: Option<String> = None;
    let mut pokedex: HashSet<String> = HashSet::new();
    let arcanine_frames = &assets.arcanine_frames;
    let pokedex_view = &assets.pokedex;

    let reset = "\x1b[0m";
    let red = "\x1b[91m";
    let white = "\x1b[97m";
    let black = "\x1b[30m";
    let color_mode = color_mode_from_env();

    let mut state = GameState::Idle;
    let mut frame_count = 0;
    let mut capture_recorded = false;
    let mut open_amount: f32 = 0.0;
    let mut capture_frame: u16 = 0;
    let mut shake_frame: u16 = 0;
    let mut shake_count: u8 = 0;
    let mut star_frame: u16 = 0;
    let mut star_hold: u16 = 0;
    let mut stream_particles: Vec<StreamParticle> = Vec::new();
    let mut align_start_a: f32 = 0.0;

    let floor_y: f32 = 5.0;

    let mut ball_x: f32 = -45.0;
    let mut ball_y: f32 = floor_y;
    let ball_scale: f32 = 1.0;
    let mut a: f32 = 0.0;
    let mut tilt_phase: f32 = 0.0;

    let mut last_cmd = String::new();
    let mut screen = Screen::Name;
    let mut welcome_frame = 0usize;
    let mut welcome_accum = 0u64;
    let welcome_frame_ms: u64 = 1000 / 12;

    write_half.write_all(b"\x1b[2J\x1b[H\x1b[?25l").await?;

    loop {
        let mut output: Vec<char> = vec![' '; width * height];
        let mut zbuffer: Vec<f32> = vec![-99.0; width * height];
        let mut color_buf: Vec<CellColor> = vec![CellColor::None; width * height];

        while let Ok(cmd) = cmd_rx.try_recv() {
            let cmd_trim = cmd.trim().to_lowercase();
            if cmd.as_bytes().contains(&3) || matches!(cmd_trim.as_str(), "q" | "quit" | "exit") {
                reader_task.abort();
                let _ = reader_task.await;
                let _ = close_session(&mut write_half).await;
                return Ok(());
            }
            if cmd_trim == "__disconnect__" {
                reader_task.abort();
                let _ = reader_task.await;
                let _ = cleanup_terminal(&mut write_half).await;
                return Ok(());
            }
            last_cmd = cmd_trim.clone();
            match screen {
                Screen::Name => {
                    if let Some(name) = sanitize_trainer_name(&cmd_trim) {
                        pokedex = load_pokedex(&name).await.unwrap_or_default();
                        trainer_name = Some(name);
                        screen = Screen::Game;
                    }
                }
                Screen::Pokedex => {
                    if cmd_trim == "back" {
                        screen = Screen::Game;
                    }
                }
                Screen::Game => {
                    if cmd_trim == "catch" && state == GameState::Idle {
                        state = GameState::Throwing;
                        frame_count = 0;
                        capture_recorded = false;
                    }
                    if cmd_trim == "pokedex" || cmd_trim == "dex" {
                        screen = Screen::Pokedex;
                    }
                }
            }
        }

        if let Screen::Game = screen {
            match state {
                GameState::Idle => {
                    frame_count += 1;
                    if frame_count > 60 {
                        frame_count = 0;
                    }
                }
                GameState::Throwing => {
                    ball_x += 1.5;
                    ball_y = floor_y + (ball_x * 0.5).sin() * 0.5;

                    if ball_x > 12.0 {
                        state = GameState::Opening;
                        ball_x = 15.0;
                        ball_y = floor_y;
                        capture_recorded = false;
                        capture_frame = 0;
                        open_amount = 0.0;
                        align_start_a = a;
                        let grow_start_y = 5;
                        let grow_start_x = (width / 2) - 2;
                        let ball_center_x = width as f32 / 2.0 + ball_x;
                        let ball_center_y = height as f32 / 2.0 + ball_y;
                        stream_particles = build_stream_particles(
                            pokemon,
                            grow_start_x,
                            grow_start_y,
                            ball_center_x,
                            ball_center_y,
                        );
                    }
                }
                GameState::Opening => {
                    capture_frame = capture_frame.saturating_add(1);
                    let t = (capture_frame as f32 / OPEN_FRAMES as f32).min(1.0);
                    open_amount = t;
                    a = align_start_a * (1.0 - t);
                    ball_x = 15.0;
                    ball_y = floor_y;
                    if capture_frame >= OPEN_FRAMES {
                        state = GameState::Absorbing;
                        capture_frame = 0;
                    }
                }
                GameState::Absorbing => {
                    capture_frame = capture_frame.saturating_add(1);
                    open_amount = 1.0;
                    ball_x = 15.0;
                    ball_y = floor_y;
                    if capture_frame >= ABSORB_FRAMES {
                        state = GameState::Closing;
                        capture_frame = 0;
                        if !capture_recorded {
                            if let Some(name) = trainer_name.as_ref() {
                                if pokedex.insert(pokemon.name.to_string()) {
                                    let _ = save_pokedex(name, &pokedex).await;
                                }
                            }
                            capture_recorded = true;
                        }
                    }
                }
                GameState::Closing => {
                    capture_frame = capture_frame.saturating_add(1);
                    let t = capture_frame as f32 / CLOSE_FRAMES as f32;
                    open_amount = (1.0 - t).max(0.0);
                    ball_x = 15.0;
                    ball_y = floor_y;
                    if capture_frame >= CLOSE_FRAMES {
                        state = GameState::Shaking;
                        shake_frame = 0;
                        shake_count = 0;
                        open_amount = 0.0;
                    }
                }
                GameState::Shaking => {
                    shake_frame = shake_frame.saturating_add(1);
                    let phase = (shake_frame as f32 / SHAKE_FRAMES as f32) * std::f32::consts::PI * 2.0;
                    let wobble = phase.sin() * 1.3;
                    ball_x = 15.0 + wobble;
                    ball_y = floor_y;
                    if shake_frame >= SHAKE_FRAMES {
                        shake_frame = 0;
                        shake_count = shake_count.saturating_add(1);
                        if shake_count >= SHAKE_COUNT {
                            star_frame = STAR_FRAMES;
                            star_hold = 30;
                            state = GameState::StarHold;
                        }
                    }
                }
                GameState::StarHold => {
                    ball_x = 15.0;
                    ball_y = floor_y;
                    if star_hold > 0 {
                        star_hold = star_hold.saturating_sub(1);
                    } else {
                        state = GameState::Idle;
                        ball_x = -45.0;
                        ball_y = floor_y;
                        frame_count = 0;
                        open_amount = 0.0;
                        stream_particles.clear();
                    }
                }
            }
        }

        match screen {
            Screen::Name => {
                if !arcanine_frames.is_empty() {
                    let frame = &arcanine_frames[welcome_frame % arcanine_frames.len()];
                    let start_x = (width.saturating_sub(frame.width)) / 2;
                    let start_y = (height.saturating_sub(frame.height)) / 2;

                    for y in 0..frame.height {
                        for x in 0..frame.width {
                            let target_y = start_y + y;
                            let target_x = start_x + x;
                            if target_y < height && target_x < width {
                                let src_idx = x + y * frame.width;
                                let ch = frame.chars[src_idx];
                                if ch != ' ' {
                                    let idx = target_x + target_y * width;
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
                    pokedex_view,
                    &pokedex,
                    &mut output,
                    &mut color_buf,
                    &mut zbuffer,
                    width,
                    height,
                );
            }
            Screen::Game => {
                if matches!(state, GameState::Idle | GameState::Throwing | GameState::Opening) {
                    let grow_start_y = 5;
                    let grow_start_x = (width / 2) - 2;

                    for y in 0..pokemon.image.height {
                        for x in 0..pokemon.image.width {
                            let target_y = grow_start_y + y;
                            let target_x = grow_start_x + x;
                            if target_y < height && target_x < width {
                                let src_idx = x + y * pokemon.image.width;
                                let ch = pokemon.image.chars[src_idx];
                                if ch != ' ' {
                                    let idx = target_x + target_y * width;
                                    output[idx] = ch;
                                    let (r, g, b) = pokemon.image.colors[src_idx];
                                    color_buf[idx] = CellColor::Rgb(r, g, b);
                                    zbuffer[idx] = 0.4;
                                }
                            }
                        }
                    }
                }
                if state == GameState::Absorbing {
                    render_stream(
                        &stream_particles,
                        capture_frame,
                        &mut output,
                        &mut color_buf,
                        &mut zbuffer,
                        width,
                        height,
                    );
                }

                let cos_a = a.cos();
                let sin_a = a.sin();
                let tilt = 0.25 + 0.1 * tilt_phase.sin();
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

                        let r = ball_scale;
                        let x = (ox * cos_a - oy * sin_a) * r;
                        let y = (ox * sin_a + oy * cos_a) * r;
                        let z = oz * r;

                        let mut y_final = y * cos_b - z * sin_b;
                        let z_final = y * sin_b + z * cos_b;
                        let x_final = x;
                        if open_amount > 0.0 && oy > 0.02 {
                            y_final += open_amount * 0.6;
                        }
                        if open_amount > 0.0 && oy.abs() < 0.03 {
                            theta += 0.03;
                            continue;
                        }

                        let camera_dist = 3.0;
                        let ooz = 1.0 / (z_final + camera_dist);

                        let xp =
                            (width as f32 / 2.0 + ball_x + 30.0 * ooz * x_final * aspect_ratio)
                                as i32;
                        let yp = (height as f32 / 2.0 + ball_y + 18.0 * ooz * y_final) as i32;

                        if xp >= 0 && xp < width as i32 && yp >= 0 && yp < height as i32 {
                            let idx = (xp + yp * width as i32) as usize;
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

                                    let mut l_idx = (shade * (chars.len() - 1) as f32) as usize;
                                    if l_idx >= chars.len() {
                                        l_idx = chars.len() - 1;
                                    }
                                    output[idx] = chars.chars().nth(l_idx).unwrap();
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

        let prompt = match screen {
            Screen::Name => "trainer name + Enter",
            Screen::Pokedex => "type 'back' + Enter",
            Screen::Game => "type 'catch' + Enter",
        };
        let prompt_line = format!("command: {} ({})", last_cmd, prompt);
        let prompt_row = height.saturating_sub(1);
        for x in 0..width {
            let idx = x + prompt_row * width;
            output[idx] = ' ';
            color_buf[idx] = CellColor::None;
        }
        for (x, ch) in prompt_line.chars().take(width).enumerate() {
            let idx = x + prompt_row * width;
            output[idx] = ch;
        }

        if let Screen::Game = screen {
            if star_frame > 0 {
                render_starburst(
                    width as i32,
                    height as i32,
                    ball_x,
                    ball_y,
                    &mut output,
                    &mut color_buf,
                    &mut zbuffer,
                    star_frame,
                );
                star_frame = star_frame.saturating_sub(1);
            }
        }

        let mut frame = String::with_capacity(width * height * 6);
        frame.push_str("\x1b[H");
        for i in 0..height {
            for j in 0..width {
                let idx = j + i * width;
                if output[idx] == ' ' {
                    frame.push(' ');
                } else {
                    match color_buf[idx] {
                        CellColor::None => frame.push(output[idx]),
                        CellColor::Ansi(code) => {
                            frame.push_str(code);
                            frame.push(output[idx]);
                            frame.push_str(reset);
                        }
                        CellColor::Rgb(r, g, b) => {
                            match color_mode {
                                ColorMode::Truecolor => {
                                    let _ = write!(
                                        frame,
                                        "\x1b[38;2;{};{};{}m{}",
                                        r,
                                        g,
                                        b,
                                        output[idx]
                                    );
                                    frame.push_str(reset);
                                }
                                ColorMode::Ansi256 => {
                                    let code = rgb_to_ansi256(r, g, b);
                                    let _ = write!(frame, "\x1b[38;5;{}m{}", code, output[idx]);
                                    frame.push_str(reset);
                                }
                                ColorMode::Mono => {
                                    frame.push(output[idx]);
                                }
                            }
                        }
                    }
                }
            }
            if i + 1 < height {
                frame.push('\n');
            }
        }
        let _ = write!(frame, "\x1b[{};1H\x1b[J", height + 1);
        write_half.write_all(frame.as_bytes()).await?;

        match screen {
            Screen::Name => {
                welcome_accum += 30;
                if welcome_accum >= welcome_frame_ms {
                    welcome_accum = 0;
                    if !arcanine_frames.is_empty() {
                        welcome_frame = (welcome_frame + 1) % arcanine_frames.len();
                    }
                }
            }
            Screen::Pokedex => {}
            Screen::Game => {
                if state == GameState::Throwing {
                    a -= 0.2;
                } else if state == GameState::Idle {
                    a -= 0.05;
                }
                tilt_phase += 0.04;
            }
        }

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

fn pick_pokemon(pokemons: &[PokemonAsset]) -> &PokemonAsset {
    if pokemons.is_empty() {
        panic!("no pokemon assets loaded");
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let idx = (nanos % pokemons.len() as u128) as usize;
    &pokemons[idx]
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
