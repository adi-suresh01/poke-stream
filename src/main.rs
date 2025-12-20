use std::{thread, time};

fn main() {
    let width = 80;   
    let height = 40;  
    
    // 1. ASPECT RATIO
    let aspect_ratio = 1.5; 

    // 2. SHADING
    // Standard set: Index 0 is space, Index 1 is dot, Index 12 is @
    let chars = " ..-,;:!+*#$@"; 

    let mut a: f32 = 0.0; 

    print!("\x1b[2J"); 

    loop {
        let mut zbuffer: Vec<f32> = vec![0.0; width * height];
        let mut output: Vec<char> = vec![' '; width * height];
        let mut colors: Vec<&str> = vec!["\x1b[0m"; width * height];

        let cos_a = a.cos();
        let sin_a = a.sin();
        
        let tilt: f32 = 0.3; 
        let cos_b = tilt.cos();
        let sin_b = tilt.sin();

        // High Density
        let mut phi: f32 = 0.0;
        while phi < 6.28 {
            let mut theta: f32 = 0.0;
            while theta < 3.14 {
                
                let ox = theta.sin() * phi.cos();
                let oy = theta.cos();
                let oz = theta.sin() * phi.sin();

                // --- TEXTURE MAPPING ---
                let mut pixel_char = '.'; 
                let pixel_color;

                let dist_to_button = ox*ox + oy*oy + (oz-1.0)*(oz-1.0);

                if dist_to_button < 0.12 {       
                     pixel_color = "\x1b[97m"; 
                     pixel_char = '@';         
                } else if dist_to_button < 0.18 { 
                     pixel_color = "\x1b[30m"; 
                     pixel_char = '#';
                } else if oy > -0.06 && oy < 0.06 { 
                    pixel_color = "\x1b[30m"; 
                    pixel_char = '#';         
                } else if oy > 0.0 {               
                    pixel_color = "\x1b[91m";
                } else {                           
                    pixel_color = "\x1b[97m";
                }

                // ROTATION
                let x = ox * cos_a - oy * sin_a;
                let y = ox * sin_a + oy * cos_a;
                let z = oz;

                // TILT
                let y_final = y * cos_b - z * sin_b;
                let z_final = y * sin_b + z * cos_b;
                let x_final = x;

                // PERSPECTIVE DISTORTION (Your preferred 2.2 setting)
                let camera_dist = 2.2;
                let ooz = 1.0 / (z_final + camera_dist); 
                
                let xp = (width as f32 / 2.0 + 25.0 * ooz * x_final * aspect_ratio) as i32;
                let yp = (height as f32 / 2.0 + 12.0 * ooz * y_final) as i32;

                // LIGHTING
                let l = x_final * -0.5 + y_final * 0.5 + z_final * -1.0;

                if l > 0.0 {
                    let idx = (xp + yp * width as i32) as usize;
                    if idx < width * height {
                        if ooz > zbuffer[idx] {
                            zbuffer[idx] = ooz;
                            
                            // === THE NEW 3D DEPTH LOGIC ===
                            
                            // 1. Calculate Max Allowed "Weight" based on Depth
                            // If z is 1.0 (Front), we allow full range (index 12).
                            // If z is -1.0 (Back), we allow ONLY small chars (index 3).
                            // This effectively blurs/fades the back.
                            
                            let depth_limit = if z_final > 0.0 {
                                12 // Front? Allow everything up to @
                            } else if z_final > -0.4 {
                                8  // Mid-back? Allow up to +
                            } else {
                                2  // Far back? Allow only . or ,
                            };

                            if pixel_char == '@' || pixel_char == '#' {
                                // Important features (Button) stay solid
                                output[idx] = pixel_char;
                            } else {
                                // Calculate Standard Lighting
                                let mut l_idx = (l * 8.0) as usize;
                                
                                // FORCE THE LIMIT
                                // If lighting says "Draw a #" but depth says "Only allowed .", use "."
                                if l_idx > depth_limit {
                                    l_idx = depth_limit;
                                }
                                
                                output[idx] = chars.chars().nth(l_idx).unwrap();
                            }
                            colors[idx] = pixel_color;
                        }
                    }
                }
                theta += 0.015; 
            }
            phi += 0.015; 
        }

        // Render
        print!("\x1b[H");
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

        a -= 0.08; 
        thread::sleep(time::Duration::from_millis(30));
    }
}