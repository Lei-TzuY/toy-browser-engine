// ============================================================
//  main.rs  —  Browser Engine Toy CLI
// ============================================================
//
//  Usage:
//    browser_engine                          # built-in demo
//    browser_engine <file.html>             # parse with default CSS
//    browser_engine <file.html> <file.css>  # parse with custom CSS
//    browser_engine <file.html> <file.css> <out.ppm>  # also write image
//    browser_engine --window                         # render in an interactive window
//
//  Always prints the DOM tree and a layout summary to stdout.

use std::{env, fs, process};

use browser_engine::{
    css::parser::parse_css,
    extract_inline_styles,
    html::parse_html,
    layout::{layout_tree, BoxType, LayoutBox},
    paint::paint,
    style::style_tree,
    dom::NodeType,
};
use minifb::{Key, Window, WindowOptions};

// ── Built-in browser default stylesheet ──────────────────────────────────────

const UA_CSS: &str = r#"
html, body { display: block; }
div, p, h1, h2, h3, h4, h5, h6,
ul, ol, li, dl, dt, dd, blockquote, pre,
header, footer, section, article, nav, main, aside,
form, fieldset, table, thead, tbody, tfoot, tr, td, th,
figure, figcaption { display: block; }

span, a, strong, em, b, i, u, s, code, abbr, cite,
small, sub, sup, label, button { display: inline; }

head, script, style, meta, link, title, noscript { display: none; }

h1 { font-size: 32px; }
h2 { font-size: 24px; }
h3 { font-size: 18px; }
p  { margin: 16px 0; }
ul, ol { margin: 16px 0; padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
a  { text-decoration: underline; color: #0000ee; }
"#;

// ── Demo content ──────────────────────────────────────────────────────────────

const DEMO_HTML: &str = r#"<!DOCTYPE html>
<html>
  <head>
    <title>Browser Engine Toy</title>
  </head>
  <body>
    <header>
      <h1 id="title" class="heading">Browser Engine Toy</h1>
    </header>
    <main>
      <p class="intro">
        A minimal browser engine written in Rust, built from scratch.
      </p>
      <div class="features">
        <h2>What it does</h2>
        <ul>
          <li>HTML tokenisation &amp; tree construction</li>
          <li>CSS parsing (selectors, cascade, specificity)</li>
          <li>Style matching &amp; computed property maps</li>
          <li>Block layout (width, height, margin, border, padding)</li>
          <li>Inline text layout with line breaking</li>
          <li>Single-axis flex-grow and flex-shrink layout</li>
          <li>PPM pixel-canvas painter</li>
        </ul>
      </div>
      <!-- A comment node -->
      <p class="note">See <a href="https://limpet.net/mbrubeck/">Matt Brubeck's series</a> for the theory.</p>
    </main>
    <footer>
      <p>End of document.</p>
    </footer>
  </body>
</html>"#;

const DEMO_CSS: &str = r#"
body    { background-color: #f5f5f5; }
header  { background-image: linear-gradient(to right, #2c3e50, #3498db); padding: 20px; text-align: center; border-radius: 0; }
h1      { color: #ecf0f1; }
main    { padding: 20px; max-width: 700px; margin: 0 auto; }
h2      { color: #2980b9; }
.intro  { color: #555555; }
.features {
    background-color: #ecf0f1;
    padding: 15px;
    border-color: #bdc3c7;
    border-width: 1px;
    border-radius: 6px;
    overflow: hidden;
}
.note   { color: #7f8c8d; text-align: right; }
footer  { background-color: #34495e; padding: 10px; text-align: center; border-radius: 0; }
footer p { color: #ecf0f1; }
a       { color: #3498db; }
"#;

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = env::args().collect();
    let show_window = args.iter().any(|arg| arg == "--window");
    let positional: Vec<&str> = args
        .iter()
        .skip(1)
        .filter(|arg| arg.as_str() != "--window")
        .map(String::as_str)
        .collect();

    let (html_src, css_src, ppm_path): (String, String, Option<String>) = match positional.len() {
        0 => (DEMO_HTML.into(), DEMO_CSS.into(), Some("output.ppm".into())),
        1 => (read_file(positional[0]), String::new(), None),
        2 => (read_file(positional[0]), read_file(positional[1]), None),
        3 => (
            read_file(positional[0]),
            read_file(positional[1]),
            Some(positional[2].into()),
        ),
        _ => {
            eprintln!("Usage: browser_engine [--window] [<html> [<css> [<out.ppm>]]]");
            process::exit(1);
        }
    };

    // ── Parse ─────────────────────────────────────────────────────────────
    let dom = parse_html(&html_src);

    // Collect CSS: UA defaults < inline <style> blocks < external/argument CSS
    let mut stylesheet = parse_css(UA_CSS);
    let inline_css = extract_inline_styles(&dom);
    stylesheet.rules.extend(parse_css(&inline_css).rules);
    stylesheet.rules.extend(parse_css(&css_src).rules);

    // ── DOM tree ──────────────────────────────────────────────────────────
    println!("╔══════════════════════════════╗");
    println!("║         DOM  TREE            ║");
    println!("╚══════════════════════════════╝");
    dom.pretty_print();

    // ── Style + layout ────────────────────────────────────────────────────
    let styled = style_tree(&dom, &stylesheet);
    let layout = layout_tree(&styled, 800.0);

    println!("\n╔══════════════════════════════╗");
    println!("║       LAYOUT  TREE           ║");
    println!("╚══════════════════════════════╝");
    print_layout(&layout, 0);

    // ── Paint → PPM ───────────────────────────────────────────────────────
    if ppm_path.is_some() || show_window {
        let canvas = paint(&layout, 800, 600);
        if let Some(path) = ppm_path {
            let ppm = canvas.to_ppm();
            match fs::write(&path, &ppm) {
                Ok(()) => println!("\nPainted → {} ({}×{}, {} bytes)", path, 800, 600, ppm.len()),
                Err(e) => eprintln!("\nFailed to write {}: {}", path, e),
            }
        }
        if show_window {
            if let Err(error) = show_canvas(&canvas) {
                eprintln!("\nFailed to open window: {}", error);
            }
        }
    }

    println!("\nDone.");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn read_file(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error reading '{}': {}", path, e);
        process::exit(1);
    })
}

fn show_canvas(canvas: &browser_engine::paint::Canvas) -> Result<(), minifb::Error> {
    let buffer = canvas.to_u32_buffer();
    let mut window = Window::new(
        "Browser Engine Toy",
        canvas.width,
        canvas.height,
        WindowOptions::default(),
    )?;
    while window.is_open() && !window.is_key_down(Key::Escape) {
        window.update_with_buffer(&buffer, canvas.width, canvas.height)?;
    }
    Ok(())
}

fn print_layout(lb: &LayoutBox, depth: usize) {
    let indent = "  ".repeat(depth);
    let d = &lb.dimensions;

    let label = match &lb.box_type {
        BoxType::Block(s)       => format!("Block({})", node_label(s.node)),
        BoxType::Flex(s)        => format!("Flex({})", node_label(s.node)),
        BoxType::Inline(s)      => format!("Inline({})", node_label(s.node)),
        BoxType::InlineBlock(s) => format!("InlineBlock({})", node_label(s.node)),
        BoxType::AnonymousBlock => "AnonymousBlock".into(),
    };

    println!(
        "{}{:<36} x={:<6.1} y={:<6.1} w={:<6.1} h={:.1}",
        indent, label,
        d.content.x, d.content.y,
        d.content.width, d.content.height,
    );

    for child in &lb.children {
        print_layout(child, depth + 1);
    }
}

fn node_label(node: &browser_engine::dom::Node) -> String {
    match &node.node_type {
        NodeType::Element(e) => {
            let mut s = format!("<{}", e.tag_name);
            if let Some(id) = e.get_attr("id") { s.push_str(&format!(" #{}", id)); }
            if let Some(cls) = e.get_attr("class") { s.push_str(&format!(" .{}", cls.split_whitespace().next().unwrap_or(""))); }
            s.push('>');
            s
        }
        NodeType::Text(t) => {
            let trimmed = t.trim();
            if trimmed.len() > 16 {
                format!("{:?}…", &trimmed[..16])
            } else {
                format!("{:?}", trimmed)
            }
        }
        NodeType::Document => "#document".into(),
        NodeType::Comment(_) => "<!-- -->".into(),
        NodeType::Doctype(n) => format!("<!DOCTYPE {}>", n),
    }
}
