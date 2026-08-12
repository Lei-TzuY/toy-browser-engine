# Toy Browser Engine

A minimal browser engine written from scratch in Rust.

It implements the core browser pipeline:

- HTML tokenization and tree construction
- CSS parsing, selectors, cascade, and specificity
- DOM/style/layout tree construction
- Block and CSS Grid layout, including `fr` tracks and gaps
- Inline text layout and basic line breaking
- Pixel-canvas painting with alpha-blended box shadows and PPM output
- A small embedded JavaScript interpreter for DOM mutation and click listeners
- Interactive hit testing, `:hover` styling, clicking, and scrolling through `minifb`

## Run

```powershell
cargo run
```

Render the built-in demo to `output.ppm`:

```powershell
cargo run -- demo.html demo.css output.ppm
```

Open the interactive window:

```powershell
cargo run -- --window
```

## Test

```powershell
cargo test
```

Current verification: 141 Rust tests passing locally and in GitHub Actions.

## Notes

This is an educational browser engine, not a production browser. The goal is to make the rendering pipeline inspectable and testable rather than standards-complete.
