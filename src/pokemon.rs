use crate::ascii::{load_ascii_animation, load_ascii_image, AsciiImage};
use std::path::Path;

const ARCANINE_PATH: &str = "assets/pokemon/arcanine.gif";
const ARCANINE_WIDTH: usize = 96;
const ARCANINE_HEIGHT: usize = 24;

const POKEMON_WIDTH: usize = 72;
const POKEMON_HEIGHT: usize = 34;

pub fn load_named_pokemon(name: &str, charset: &str) -> AsciiImage {
    let jpg_path = format!("assets/pokemon/{name}.jpg");
    if Path::new(&jpg_path).exists() {
        return load_ascii_image(&jpg_path, POKEMON_WIDTH, POKEMON_HEIGHT, charset);
    }
    let png_path = format!("assets/pokemon/{name}.png");
    if Path::new(&png_path).exists() {
        return load_ascii_image(&png_path, POKEMON_WIDTH, POKEMON_HEIGHT, charset);
    }
    panic!("missing pokemon image for {name}");
}

pub fn load_arcanine_frames(charset: &str) -> Vec<AsciiImage> {
    load_ascii_animation(ARCANINE_PATH, ARCANINE_WIDTH, ARCANINE_HEIGHT, charset)
}
