# Toy Browser Engine

A minimal browser engine written from scratch in Rust.

It implements the core browser pipeline:

- HTML tokenization and tree construction
- CSS parsing, selectors, cascade, and specificity
- DOM/style/layout tree construction
- Block layout with margin, border, and padding
- Inline text layout and basic line breaking
- A simple pixel-canvas painter that can write PPM output
- Optional interactive window rendering through `minifb`

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

Current local audit: 133 Rust tests passing.

## Notes

This is an educational browser engine, not a production browser. The goal is to make the rendering pipeline inspectable and testable rather than standards-complete.

