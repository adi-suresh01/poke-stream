use crate::ascii::{load_ascii_image, AsciiImage};

const GROWLITHE_PATH: &str = "assets/pokemon/growlithe.jpg";
const GROWLITHE_WIDTH: usize = 56;
const GROWLITHE_HEIGHT: usize = 34;

pub fn load_growlithe(charset: &str) -> AsciiImage {
    load_ascii_image(GROWLITHE_PATH, GROWLITHE_WIDTH, GROWLITHE_HEIGHT, charset)
}
