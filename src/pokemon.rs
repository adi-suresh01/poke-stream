use crate::ascii::{load_ascii_animation, load_ascii_image, AsciiImage};

const GROWLITHE_PATH: &str = "assets/pokemon/growlithe.jpg";
const GROWLITHE_WIDTH: usize = 56;
const GROWLITHE_HEIGHT: usize = 34;

const BULBASAUR_PATH: &str = "assets/pokemon/bulbasaur.jpg";
const BULBASAUR_WIDTH: usize = 72;
const BULBASAUR_HEIGHT: usize = 34;

const IVYSAUR_PATH: &str = "assets/pokemon/ivysaur.jpg";
const IVYSAUR_WIDTH: usize = 72;
const IVYSAUR_HEIGHT: usize = 34;

const VENUSAUR_PATH: &str = "assets/pokemon/venusaur.jpg";
const VENUSAUR_WIDTH: usize = 72;
const VENUSAUR_HEIGHT: usize = 34;

const CHARMANDER_PATH: &str = "assets/pokemon/charmander.jpg";
const CHARMANDER_WIDTH: usize = 68;
const CHARMANDER_HEIGHT: usize = 34;

const CHARMELEON_PATH: &str = "assets/pokemon/charmeleon.jpg";
const CHARMELEON_WIDTH: usize = 72;
const CHARMELEON_HEIGHT: usize = 34;

const CHARIZARD_PATH: &str = "assets/pokemon/charizard.jpg";
const CHARIZARD_WIDTH: usize = 76;
const CHARIZARD_HEIGHT: usize = 34;

const SQUIRTLE_PATH: &str = "assets/pokemon/squirtle.jpg";
const SQUIRTLE_WIDTH: usize = 64;
const SQUIRTLE_HEIGHT: usize = 34;

const WARTORTLE_PATH: &str = "assets/pokemon/wartortle.jpg";
const WARTORTLE_WIDTH: usize = 72;
const WARTORTLE_HEIGHT: usize = 34;

const BLASTOISE_PATH: &str = "assets/pokemon/blastoise.jpg";
const BLASTOISE_WIDTH: usize = 74;
const BLASTOISE_HEIGHT: usize = 34;

const PIKACHU_PATH: &str = "assets/pokemon/pikachu.png";
const PIKACHU_WIDTH: usize = 78;
const PIKACHU_HEIGHT: usize = 34;

const ARCANINE_PATH: &str = "assets/pokemon/arcanine.gif";
const ARCANINE_WIDTH: usize = 96;
const ARCANINE_HEIGHT: usize = 24;

pub fn load_growlithe(charset: &str) -> AsciiImage {
    load_ascii_image(GROWLITHE_PATH, GROWLITHE_WIDTH, GROWLITHE_HEIGHT, charset)
}

pub fn load_bulbasaur(charset: &str) -> AsciiImage {
    load_ascii_image(BULBASAUR_PATH, BULBASAUR_WIDTH, BULBASAUR_HEIGHT, charset)
}

pub fn load_ivysaur(charset: &str) -> AsciiImage {
    load_ascii_image(IVYSAUR_PATH, IVYSAUR_WIDTH, IVYSAUR_HEIGHT, charset)
}

pub fn load_venusaur(charset: &str) -> AsciiImage {
    load_ascii_image(VENUSAUR_PATH, VENUSAUR_WIDTH, VENUSAUR_HEIGHT, charset)
}

pub fn load_charmander(charset: &str) -> AsciiImage {
    load_ascii_image(CHARMANDER_PATH, CHARMANDER_WIDTH, CHARMANDER_HEIGHT, charset)
}

pub fn load_charmeleon(charset: &str) -> AsciiImage {
    load_ascii_image(CHARMELEON_PATH, CHARMELEON_WIDTH, CHARMELEON_HEIGHT, charset)
}

pub fn load_charizard(charset: &str) -> AsciiImage {
    load_ascii_image(CHARIZARD_PATH, CHARIZARD_WIDTH, CHARIZARD_HEIGHT, charset)
}

pub fn load_squirtle(charset: &str) -> AsciiImage {
    load_ascii_image(SQUIRTLE_PATH, SQUIRTLE_WIDTH, SQUIRTLE_HEIGHT, charset)
}

pub fn load_wartortle(charset: &str) -> AsciiImage {
    load_ascii_image(WARTORTLE_PATH, WARTORTLE_WIDTH, WARTORTLE_HEIGHT, charset)
}

pub fn load_blastoise(charset: &str) -> AsciiImage {
    load_ascii_image(BLASTOISE_PATH, BLASTOISE_WIDTH, BLASTOISE_HEIGHT, charset)
}

pub fn load_pikachu(charset: &str) -> AsciiImage {
    load_ascii_image(PIKACHU_PATH, PIKACHU_WIDTH, PIKACHU_HEIGHT, charset)
}

pub fn load_arcanine_frames(charset: &str) -> Vec<AsciiImage> {
    load_ascii_animation(ARCANINE_PATH, ARCANINE_WIDTH, ARCANINE_HEIGHT, charset)
}
