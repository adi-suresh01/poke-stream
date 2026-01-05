mod ascii;
mod pokemon;

use std::fmt::Write;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{self, Duration};

#[derive(PartialEq)]
enum GameState {
    Idle,
    Throwing,
    Caught,
}

#[derive(Copy, Clone)]
enum CellColor {
    None,
    Ansi(&'static str),
    Rgb(u8, u8, u8),
}

struct Assets {
    pokemons: Vec<ascii::AsciiImage>,
    arcanine_frames: Vec<ascii::AsciiImage>,
}

enum Screen {
    Welcome,
    Game,
}

#[derive(Copy, Clone, PartialEq)]
enum ColorMode {
    Truecolor,
    Ansi256,
    Mono,
}

const IMG_CHARSET: &str =
    ".'`^\",:;Il!i><~+_-?][}{1)(|\\/*tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$Ñ";

#[tokio::main]
async fn main() -> io::Result<()> {
    let assets = Arc::new(Assets {
        pokemons: vec![
            pokemon::load_growlithe(IMG_CHARSET),
            pokemon::load_pikachu(IMG_CHARSET),
        ],
        arcanine_frames: pokemon::load_arcanine_frames(IMG_CHARSET),
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
    let arcanine_frames = &assets.arcanine_frames;

    let reset = "\x1b[0m";
    let red = "\x1b[91m";
    let white = "\x1b[97m";
    let black = "\x1b[30m";
    let color_mode = color_mode_from_env();

    let mut state = GameState::Idle;
    let mut frame_count = 0;
    let mut caught_timer = 0;

    let floor_y: f32 = 5.0;

    let mut ball_x: f32 = -45.0;
    let mut ball_y: f32 = floor_y;
    let ball_scale: f32 = 1.0;
    let mut a: f32 = 0.0;
    let mut tilt_phase: f32 = 0.0;

    let mut last_cmd = String::new();
    let mut screen = Screen::Welcome;
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
                Screen::Welcome => {
                    if cmd_trim == "start" || cmd_trim == "play" || cmd_trim == "catch" {
                        screen = Screen::Game;
                        if cmd_trim == "catch" && state == GameState::Idle {
                            state = GameState::Throwing;
                            frame_count = 0;
                        }
                    }
                }
                Screen::Game => {
                    if cmd_trim == "catch" && state == GameState::Idle {
                        state = GameState::Throwing;
                        frame_count = 0;
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
                        state = GameState::Caught;
                        ball_x = 15.0;
                        ball_y = floor_y;
                        caught_timer = 0;
                    }
                }
                GameState::Caught => {
                    caught_timer += 1;
                    ball_x = 15.0;

                    if caught_timer > 50 {
                        state = GameState::Idle;
                        ball_x = -45.0;
                        ball_y = floor_y;
                        frame_count = 0;
                    }
                }
            }
        }

        match screen {
            Screen::Welcome => {
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
            Screen::Game => {
                if state != GameState::Caught {
                    let grow_start_y = 5;
                    let grow_start_x = (width / 2) + 2;

                    for y in 0..pokemon.height {
                        for x in 0..pokemon.width {
                            let target_y = grow_start_y + y;
                            let target_x = grow_start_x + x;
                            if target_y < height && target_x < width {
                                let src_idx = x + y * pokemon.width;
                                let ch = pokemon.chars[src_idx];
                                if ch != ' ' {
                                    let idx = target_x + target_y * width;
                                    output[idx] = ch;
                                    let (r, g, b) = pokemon.colors[src_idx];
                                    color_buf[idx] = CellColor::Rgb(r, g, b);
                                    zbuffer[idx] = 0.4;
                                }
                            }
                        }
                    }
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

                        if dist_to_button < 0.12 {
                            pixel_color = white;
                            pixel_char = '@';
                        } else if dist_to_button < 0.18 {
                            pixel_color = black;
                            pixel_char = '#';
                        } else if oy > -0.06 && oy < 0.06 {
                            pixel_color = black;
                            pixel_char = '#';
                        } else if oy > 0.0 {
                            pixel_color = red;
                        } else {
                            pixel_color = white;
                        }

                        let r = ball_scale;
                        let x = (ox * cos_a - oy * sin_a) * r;
                        let y = (ox * sin_a + oy * cos_a) * r;
                        let z = oz * r;

                        let y_final = y * cos_b - z * sin_b;
                        let z_final = y * sin_b + z * cos_b;
                        let x_final = x;

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
            Screen::Welcome => "type 'start' + Enter",
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
            Screen::Welcome => {
                welcome_accum += 30;
                if welcome_accum >= welcome_frame_ms {
                    welcome_accum = 0;
                    if !arcanine_frames.is_empty() {
                        welcome_frame = (welcome_frame + 1) % arcanine_frames.len();
                    }
                }
            }
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

fn pick_pokemon(pokemons: &[ascii::AsciiImage]) -> &ascii::AsciiImage {
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
