use crate::ascii::{load_ascii_animation, load_ascii_image, AsciiImage};

const GROWLITHE_PATH: &str = "assets/pokemon/growlithe.jpg";
const GROWLITHE_WIDTH: usize = 56;
const GROWLITHE_HEIGHT: usize = 34;

const PIKACHU_PATH: &str = "assets/pokemon/pikachu.png";
const PIKACHU_WIDTH: usize = 78;
const PIKACHU_HEIGHT: usize = 34;

const ARCANINE_PATH: &str = "assets/pokemon/arcanine.gif";
const ARCANINE_WIDTH: usize = 96;
const ARCANINE_HEIGHT: usize = 24;

pub fn load_growlithe(charset: &str) -> AsciiImage {
    load_ascii_image(GROWLITHE_PATH, GROWLITHE_WIDTH, GROWLITHE_HEIGHT, charset)
}

pub fn load_pikachu(charset: &str) -> AsciiImage {
    load_ascii_image(PIKACHU_PATH, PIKACHU_WIDTH, PIKACHU_HEIGHT, charset)
}

pub fn load_arcanine_frames(charset: &str) -> Vec<AsciiImage> {
    load_ascii_animation(ARCANINE_PATH, ARCANINE_WIDTH, ARCANINE_HEIGHT, charset)
}
