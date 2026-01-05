# Poke-Stream

![Arcanine animation](arcanine.gif)

Terminal-hosted Pokemon game with real-time ASCII animation. Players connect over Telnet to play and catch Pokemon while a 3D, ray-cast Pokeball animates over 2D Pokemon art, all rendered as colored ASCII characters. The server is written in Rust and uses Tokio to run one game loop per connection.

## Features
- Telnet-playable game loop with a welcome screen and capture sequence.
- Hybrid renderer: 2D ASCII sprites plus a 3D shaded sphere with z-buffering.
- Color-aware ASCII rendering with truecolor, ANSI 256, or monochrome output.
- Async TCP server that spawns a session task per client.
- Asset pipeline that converts PNG/JPG/GIF into colored ASCII frames.

## Play (Public Server)
From any terminal with Telnet installed:

```bash
telnet why-ntsc.gl.at.ply.gg 62201
```

If your terminal supports 24-bit color, set:

```bash
POKESTREAM_COLOR=truecolor
```

Recommended terminal size is at least 140x40.

## Controls
- On connect: enter a unique trainer name
- In game: type `catch` to throw the Pokeball
- In game: type `pokedex` or `dex` to view your Pokedex
- Exit the game: type `q`, `quit`, or `exit`

## Exiting Telnet
After the game closes, exit your Telnet client:

```text
Press Ctrl + ]
Type quit
```

![Capture sequence](Screenshot%202026-01-04%20at%2016.34.54.png)

## Architecture Summary
- **Server loop**: `src/main.rs` binds on `0.0.0.0:8080` and spawns a Tokio task per connection.
- **Game states**: `Idle`, `Throwing`, `Opening`, `Absorbing`, `Closing`, `Shaking`, `StarHold`.
- **Renderer**:
  - **2D layer**: ASCII Pokemon sprites with per-character color.
  - **3D layer**: Ray-cast sphere with shading, rim band, and button.
  - **Buffers**: character output, color buffer, and z-buffer.
- **Assets**: `src/pokemon.rs` loads image assets and converts them to ASCII using `src/ascii.rs`.

## Configuration
Color mode selection (optional):
- `POKESTREAM_COLOR=truecolor` or `24bit`
- `POKESTREAM_COLOR=ansi256` or `256`
- `POKESTREAM_COLOR=mono` or `none`

If unset, the server auto-detects color support via `COLORTERM` and `TERM`.

## Tech Stack
- Rust (edition 2024)
- Tokio for async networking
- Image crate for asset decoding and GIF frames

## Project Context
This project is a terminal-native, distributed-systems style experiment: a single server process accepts multiple Telnet sessions and runs an independent animation loop per client. The goal is to support many concurrent users with low latency rendering while keeping everything in ASCII.

## Roadmap
- Add "three shakes" wobble animation before capture confirmation.
- Improve the Pokemon art assets and add more species.
- Externalize session state (planned Redis-backed pokedex/session store).
- Explore higher concurrency targets with profiling and load testing.
