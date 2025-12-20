Markdown

# Project Context: 3D ASCII Pokeball Streamer

## 1. Project Overview
**Goal:** Build a terminal-based application that streams high-fidelity ASCII animations to users via Telnet/SSH.
**Core Feature:** A 3D, mathematically rendered Pokeball that "catches" a 2D ASCII Pokemon in real-time.
**Target Audience:** Users connecting via terminal (e.g., `telnet pokestream.com`).
**Current Status:** Phase 1 (Local Rendering Engine) is functional. Moving towards Phase 2 (Networking).

---

## 2. Technical Architecture

### A. Development Environment
* **Machine:** Ubuntu Server (Headless).
* **Workflow:** Remote Development via VS Code (SSH Extension).
* **Version Control:** Git (GitHub Repo: `poke-stream`).

### B. The Stack
* **Language:** Rust (for high-performance ASCII rendering and eventually TCP networking).
* **Rendering Method:** Hybrid 2D/3D Engine.
    * **Layer 1 (Background):** Static 2D ASCII Art (The Pokemon).
    * **Layer 2 (Foreground):** Dynamic 3D Ray-casted Sphere (The Pokeball) with Z-buffering.
* **Future Backend (Planned):**
    * **Networking:** Tokio (Rust async runtime) for handling multiple Telnet connections.
    * **State Management:** Redis (for user session/pokedex storage).
    * **Logic:** Potential Python worker for complex game logic (TBD).

---

## 3. Current Codebase State

### File Structure
```text
poke-stream/
├── Cargo.toml      # Rust dependencies
├── context.md      # This file
└── src/
    ├── main.rs     # The Core Engine (Render Loop, Physics, State Machine)
    └── art.rs      # The Asset File (Strings for Pokemon ASCII)
Core Logic (src/main.rs)
The engine runs a mostly infinite loop with the following components:

State Machine:

GameState::Idle: Ball spins in place (Left side), Pokemon stands (Right side).

GameState::Throwing: Ball moves horizontally (x += speed) towards Pokemon.

GameState::Caught: Ball snaps to Pokemon position, Animation pauses, Pokemon layer is hidden (to simulate being inside).

Rendering Pipeline:

Buffers: Uses output (char), color_buf (ANSI strings), and zbuffer (depth) arrays.

3D Math Kernel:

Standard Sphere Equation with Texture Mapping for the Pokeball features (Button, Band, Top Red, Bottom White).

Rotation: Z-axis spin ("Rolling tire") + X-axis Tilt (Hubcap view).

Perspective: Weak perspective projection (1.0 / (z + camera_dist)).

Physics & Constants:

Aspect Ratio: 1.5 (Crucial for fixing "tall" terminal characters).

Throw Style: Horizontal linear movement (no gravity arc) to ensure alignment.

Alignment: Pokemon and Ball are aligned on a virtual "floor" at y = 8.0.

Assets (src/art.rs)
Contains pub const PIKACHU: &str.

Art Style: "Chunky" Gen 1 aesthetic. Denser characters preferred over thin line art to match the 3D ball's visual weight.

4. Design Decisions & Evolution (Why we are here)
Visual Style:

Decision: We abandoned "Deep Perspective" (Fisheye) because it distorted the ASCII characters too much.

Current: We use a "Clean" orthographic-style projection which preserves the round shape of the ball.

Shading: We use a specific char set (..-,;:!+*#$@) and High Density scanning (0.02 steps) to make the ball look solid, not wireframe.

Pokemon Art:

Iteration 1: Standard line art (Too thin, looked like a wireframe).

Iteration 2: "Owl-like" Pikachu (Proportions wrong).

Current: "Fat/Chunky" Pikachu (Gen 1 Style) to match the volume of the 3D ball.

Animation Logic:

Iteration 1: Vertical throw (Ball bounced). Removed because aligning 3D objects with 2D text vertically is difficult.

Current: Horizontal Throw. The ball moves side-to-side. This guarantees it hits the Pokemon "in the face."

Auth Strategy (Planned):

We will NOT use passwords over Telnet (Security risk).

We will use Session Keys (User types a username, if it exists, load state; if not, create new). Low stakes security.

5. Next Steps (Immediate Todo)
Refine Art: The Pikachu ASCII needs to be finalized to the "Fat Gen 1" reference image.

Wobble Animation: Implement the "3 shakes" animation in the GameState::Caught phase before the final capture confirmation.

Networking (Phase 2):

Wrap the main.rs logic in a Tokio TCP Listener.

Convert println! statements to socket.write.

Handle multiple concurrent users (each gets their own instance of the GameState struct).

6. Prompt History Summary (User Requests)
User wants: A retro, ASCII-based interactive experience.

Key Constraints: Must run on Ubuntu Server, viewed via Terminal.

Specific Feedback: "Pikachu looked distorted," "Make the ball move towards the Pokemon," "Ball must spin in perpetuity after reset," "Use Aspect Ratio 1.5."