// src/main.rs
use std::{thread, time};

fn main() {
    // 1. Configuration
    let width = 80;  
    let height = 40; 
    let chars = ".,-~:;=!*#$@";
    
    // Rotation State
    let mut a: f32 = 0.0; // Angle Z (Rolling)
    
    // Clear the screen initially
    print!("\x1b[2J");

    println!("Starting Pokeball Engine...");
    
    // 2. The Animation Loop
    loop {
        // Buffers
        let mut zbuffer: Vec<f32> = vec![0.0; width * height];
        let mut output: Vec<char> = vec![' '; width * height];
        let mut colors: Vec<&str> = vec!["\x1b[0m"; width * height];

        // Math Constants
        let cos_a = a.cos();
        let sin_a = a.sin();
        let tilt: f32 = 0.5;
        let cos_b = tilt.cos();
        let sin_b = tilt.sin();

        // 3. Scan the Sphere (The Math Kernel)
        let mut phi: f32 = 0.0;
        while phi < 6.28 {
            let mut theta: f32 = 0.0;
            while theta < 3.14 {
                // Sphere Geometry
                let ox = theta.sin() * phi.cos();
                let oy = theta.cos();
                let oz = theta.sin() * phi.sin();

                // Texture Mapping (The Art)
                let pixel_color = if oy > -0.1 && oy < 0.1 {
                    "\x1b[90m" // Band (Dark Gray)
                } else if oy > -0.15 && oy < 0.15 && oz > 0.8 {
                    "\x1b[97m" // Button (White)
                } else if oy > 0.0 {
                    "\x1b[91m" // Top (Red)
                } else {
                    "\x1b[97m" // Bottom (White)
                };

                // Rotation
                let x = ox * cos_a - oy * sin_a;
                let y = ox * sin_a + oy * cos_a;
                let z = oz;

                // 3D Tilt
                let y_final = y * cos_b - z * sin_b;
                let z_final = y * sin_b + z * cos_b;
                let x_final = x;

                // Projection
                let ooz = 1.0 / (z_final + 4.0);
                let xp = (width as f32 / 2.0 + 30.0 * ooz * x_final * 2.0) as i32;
                let yp = (height as f32 / 2.0 + 15.0 * ooz * y_final) as i32;

                // Lighting
                let l = x_final * 0.0 + y_final * 1.0 + z_final * -1.0;

                if l > 0.0 {
                    let idx = (xp + yp * width as i32) as usize;
                    if idx < width * height {
                        if ooz > zbuffer[idx] {
                            zbuffer[idx] = ooz;
                            let mut l_idx = (l * 8.0) as usize;
                            if l_idx > 11 { l_idx = 11; }
                            output[idx] = chars.chars().nth(l_idx).unwrap();
                            colors[idx] = pixel_color;
                        }
                    }
                }
                theta += 0.03; // Lower number = higher quality
            }
            phi += 0.03;
        }

        // 4. Draw to Screen
        print!("\x1b[H"); // Reset cursor to top-left
        let mut frame = String::with_capacity(width * height * 10);
        
        for i in 0..height {
            for j in 0..width {
                let idx = j + i * width;
                if output[idx] == ' ' {
                    frame.push(' ');
                } else {
                    frame.push_str(colors[idx]);
                    frame.push(output[idx]);
                    frame.push_str("\x1b[0m");
                }
            }
            frame.push('\n');
        }
        println!("{}", frame);

        a += 0.08;
        thread::sleep(time::Duration::from_millis(33));
    }
}