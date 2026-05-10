# analog-felx

A GPU-accelerated, node-and-timeline video compositor written in Rust + wgpu, aimed at replacing the After Effects workflows the author uses for video glitch art.

```bash
cargo run -p felx-app          # GUI
cargo run -p felx-cli -- help  # CLI render runner
```

See [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md) for usage, [`PRD.md`](PRD.md) for the vision, and [`CLAUDE.md`](CLAUDE.md) for the codebase tour.

Dual-licensed under MIT and Apache-2.0; see `LICENSE-MIT`, `LICENSE-APACHE`, and `NOTICES` for attribution.
