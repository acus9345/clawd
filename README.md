# Clawd

A pixel-art desktop pet for Windows. Clawd lives on top of your taskbar, wanders around, waves, dances, codes on a tiny laptop, naps, does backflips, and plays games with you.

*A fan-made project, not affiliated with Anthropic.*
<img width="886" height="560" alt="image" src="https://github.com/user-attachments/assets/3ee9dea3-5a81-462f-9b3e-e022d209d186" />


## Download

Grab `clawd.exe` from the [latest release](https://github.com/acus9345/clawd/releases/latest) and double-click it. No install needed. Windows 10 or 11.

## Playing with Clawd

| Do this | Clawd does |
|---|---|
| Drag it | Picks it up; let go mid-swing to throw it (it bounces off screen edges) |
| Click it | Hops and shows a heart; click again mid-hop for a backflip |
| Wiggle the cursor over it | Giggles (tickling) |
| Right-click it | Opens the menu: games, tricks, nap, wander on/off, quit |

On its own it idles, walks, waves, dances, writes code, looks around curiously, and yawns before napping.

## Games

Start any of these from the right-click menu.

- **Ping pong**: on a table a quarter of your screen, your paddle follows the mouse up and down. First to 5. Esc stops.
- **Badminton**: the net stands on your taskbar in the middle of the screen. Your racket follows the mouse and tilts as you move; swing through the shuttlecock for power. First to 7. Esc stops.
- **Chess**: you play white; click a piece, then where it should go. Clawd thinks on its laptop. Full rules (castling, en passant, draws); pawns promote to queens.

## Smooth on any monitor

Every frame waits for your monitor's refresh, so animation and dragging run at its exact rate (60, 144, 165 Hz…).

## Building from source

```
cargo build --release
```

The program ends up in `target\release\clawd.exe`. Run the tests with `cargo test --release`.

## License

MIT
