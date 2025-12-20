mod ascii;
mod pokemon;

use std::{
    fmt::Write,
    io::{self, BufRead},
    os::fd::AsRawFd,
    sync::mpsc,
    thread,
    time,
};
use termios::{tcsetattr, Termios, ECHO, TCSANOW};

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

struct EchoGuard {
    original: Termios,
}

impl EchoGuard {
    fn new() -> Self {
        let fd = io::stdin().as_raw_fd();
        let mut term = Termios::from_fd(fd).expect("failed to read termios");
        let original = term.clone();
        term.c_lflag &= !ECHO;
        tcsetattr(fd, TCSANOW, &term).expect("failed to disable echo");
        Self { original }
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        let fd = io::stdin().as_raw_fd();
        let _ = tcsetattr(fd, TCSANOW, &self.original);
    }
}

fn main() {
    let width = 140;
    let height = 40;
    
    // FIX 1: Aspect Ratio set to 1.5 as requested
    let aspect_ratio = 1.5;
    
    let chars = " .:-=+*#%@";
    let img_charset =
        ".'`^\",:;Il!i><~+_-?][}{1)(|\\/*tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$Ñ";
    let growlithe = pokemon::load_growlithe(img_charset);

    // --- ANSI COLORS ---
    let reset = "\x1b[0m";
    let red = "\x1b[91m";
    let white = "\x1b[97m";
    let black = "\x1b[30m";

    // --- STATE ---
    let mut state = GameState::Idle;
    let mut frame_count = 0;
    let mut caught_timer = 0; // To track how long we stay in "Caught" state

    // POSITIONS
    // We align them based on the "Floor" (approx row 25)
    let floor_y: f32 = 5.0; 
    
    let mut ball_x: f32 = -45.0; // Left side
    let mut ball_y: f32 = floor_y;   
    let ball_scale: f32 = 1.0;
    let mut a: f32 = 0.0;   
    let mut tilt_phase: f32 = 0.0;

    let _echo_guard = EchoGuard::new();

    let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines().flatten() {
            if cmd_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut last_cmd = String::new();

    print!("\x1b[2J"); 

    loop {
        let mut output: Vec<char> = vec![' '; width * height];
        let mut zbuffer: Vec<f32> = vec![-99.0; width * height]; 
        let mut color_buf: Vec<CellColor> = vec![CellColor::None; width * height];

        if let Ok(cmd) = cmd_rx.try_recv() {
            let cmd_trim = cmd.trim().to_lowercase();
            last_cmd = cmd_trim.clone();
            if cmd_trim == "catch" && state == GameState::Idle {
                state = GameState::Throwing;
                frame_count = 0;
            }
        }

        match state {
            GameState::Idle => {
                frame_count += 1;
                // Wait 60 frames, then throw
                if frame_count > 60 {
                    frame_count = 0;
                }
            }
            GameState::Throwing => {
                // FIX 2: Horizontal Throw (No Arc)
                // Moves straight towards Pikachu
                ball_x += 1.5; 
                
                // Add a tiny bit of "Roll" bobble just for realism (Sine wave)
                ball_y = floor_y + (ball_x * 0.5).sin() * 0.5;

                // Hit detection (Pikachu is around x=15)
                if ball_x > 12.0 {
                    state = GameState::Caught;
                    ball_x = 15.0; // Snap to center of Pikachu
                    ball_y = floor_y;
                    caught_timer = 0;
                }
            }
            GameState::Caught => {
                // FIX 3: Reset Loop
                caught_timer += 1;
                ball_x = 15.0;
                
                // Stay caught for 50 frames (approx 1.5 seconds), then reset
                if caught_timer > 50 {
                    state = GameState::Idle;
                    ball_x = -45.0; // Reset to start
                    ball_y = floor_y;
                    frame_count = 0;
                }
            }
        }

        // --- RENDER GROWLITHE (IMAGE -> ASCII, COLOR) ---
        if state != GameState::Caught {
            let grow_start_y = 5;
            let grow_start_x = (width / 2) + 2;

            for y in 0..growlithe.height {
                for x in 0..growlithe.width {
                    let target_y = grow_start_y + y;
                    let target_x = grow_start_x + x;
                    if target_y < height && target_x < width {
                        let src_idx = x + y * growlithe.width;
                        let ch = growlithe.chars[src_idx];
                        if ch != ' ' {
                            let idx = target_x + target_y * width;
                            output[idx] = ch;
                            let (r, g, b) = growlithe.colors[src_idx];
                            color_buf[idx] = CellColor::Rgb(r, g, b);
                            zbuffer[idx] = 0.4;
                        }
                    }
                }
            }
        }

        // --- RENDER POKEBALL ---
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

                // Texture
                let mut pixel_char = '.';
                let pixel_color;
                let dist_to_button = ox*ox + oy*oy + (oz-1.0)*(oz-1.0);

                if dist_to_button < 0.12 { pixel_color = white; pixel_char = '@'; } 
                else if dist_to_button < 0.18 { pixel_color = black; pixel_char = '#'; } 
                else if oy > -0.06 && oy < 0.06 { pixel_color = black; pixel_char = '#'; } 
                else if oy > 0.0 { pixel_color = red; } 
                else { pixel_color = white; }

                let r = ball_scale;
                let x = (ox * cos_a - oy * sin_a) * r;
                let y = (ox * sin_a + oy * cos_a) * r;
                let z = oz * r;

                let y_final = y * cos_b - z * sin_b;
                let z_final = y * sin_b + z * cos_b;
                let x_final = x;

                let camera_dist = 3.0;
                let ooz = 1.0 / (z_final + camera_dist);
                
                // Apply offsets
                let xp = (width as f32 / 2.0 + ball_x + 30.0 * ooz * x_final * aspect_ratio) as i32;
                // FIX 5: Adjusted Y offset (+18) to match Pikachu's feet
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

        // Render
        print!("\x1b[H");
        let mut frame = String::with_capacity(width * height * 15);
        for i in 0..height {
            for j in 0..width {
                let idx = j + i * width;
                if output[idx] == ' ' {
                    frame.push(' ');
                } else {
                    match color_buf[idx] {
                        CellColor::None => {
                            frame.push(output[idx]);
                        }
                        CellColor::Ansi(code) => {
                            frame.push_str(code);
                            frame.push(output[idx]);
                            frame.push_str(reset);
                        }
                        CellColor::Rgb(r, g, b) => {
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
                    }
                }
            }
            frame.push('\n');
        }
        let _ = write!(frame, "command: {} (type 'catch' + Enter)\n", last_cmd);
        println!("{}", frame);

        // Spin Logic
        if state == GameState::Throwing {
            // Spin fast when throwing
            a -= 0.2; 
        } else if state == GameState::Idle {
            // Spin slow when idle
            a -= 0.05;
        }
        // If caught, stop spinning (a stays same)
        tilt_phase += 0.04;

        thread::sleep(time::Duration::from_millis(30));
    }
}
