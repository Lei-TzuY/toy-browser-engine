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

// ── Built-in browser default stylesheet ──────────────────────────────────────

const UA_CSS: &str = r#"
html, body { display: block; }
div, p, h1, h2, h3, h4, h5, h6,
ul, ol, li, dl, dt, dd, blockquote, pre,
header, footer, section, article, nav, main, aside,
form, fieldset, table, thead, tbody, tfoot, tr, td, th,
figure, figcaption { display: block; }

span, a, strong, em, b, i, u, s, code, abbr, cite,
small, sub, sup, label, button, input { display: inline-block; }

head, script, style, meta, link, title, noscript { display: none; }

h1 { font-size: 32px; }
h2 { font-size: 24px; }
h3 { font-size: 18px; }
p  { margin: 16px 0; }
ul, ol { margin: 16px 0; padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
a  { text-decoration: underline; color: #0000ee; }
button { padding: 8px 16px; border-radius: 4px; border: none; cursor: pointer; }
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
        A minimal browser engine written in Rust with JavaScript AST Engine, DOM API, CSS Grid, 2D Transforms &amp; Table Layout!
      </p>
      
      <h2>Interactive JavaScript Demo</h2>
      <div class="card">
        <h3 id="script-heading">Live DOM Mutation &amp; JS Events</h3>
        <p id="script-desc">Click the button below to execute embedded JavaScript and dynamically update DOM nodes!</p>
        <button id="counter-btn" class="btn">Click Count: 0</button>
      </div>

      <h2>HTML Table Layout (display: table)</h2>
      <table class="demo-table">
        <tr>
          <th>Subsystem</th>
          <th>Implementation Status</th>
        </tr>
        <tr>
          <td>JS AST &amp; Interpreter</td>
          <td>Active (`script/mod.rs`)</td>
        </tr>
        <tr>
          <td>CSS Grid &amp; Table</td>
          <td>Supported</td>
        </tr>
      </table>

      <h2>HTML Image Element (&lt;img&gt;)</h2>
      <img src="logo.ppm" alt="Rust Browser Engine" />

      <h2>CSS Grid Cards (2D Layout)</h2>
      <div class="grid-container">
        <div class="card">
          <h3>JS Engine</h3>
          <p>Tree-walk AST interpreter with DOM querySelector &amp; innerText API.</p>
          <button class="btn">Learn JS</button>
        </div>
        <div class="card">
          <h3>Layout Subsystems</h3>
          <p>Block, Flexbox, CSS Grid, and Table layout algorithms.</p>
          <button class="btn">View Tables</button>
        </div>
        <div class="card">
          <h3>Rasterizer &amp; Painter</h3>
          <p>PPM image export, soft box-shadow rendering, and interactive GUI window.</p>
          <button class="btn">Try Window</button>
        </div>
      </div>

      <div class="features">
        <h2>Engine Pipeline Capabilities</h2>
        <ul>
          <li>HTML tokenisation &amp; tree construction</li>
          <li>CSS parsing (selectors, cascade, specificity, transforms)</li>
          <li>Embedded JavaScript Interpreter (`let`, DOM mutations, click listeners)</li>
          <li>CSS Grid 2D track layout (`display: grid`, `fr` units, gaps)</li>
          <li>HTML Table Layout (`<table>`, `<tr>`, `<td>`, `<th>`)</li>
          <li>Interactive `:hover` pseudo-class and real-time mouse hit-testing</li>
          <li>Alpha-blended `box-shadow` &amp; `<img>` rendering</li>
        </ul>
      </div>

      <script>
        let count = 0;
        let btn = document.getElementById("counter-btn");
        let heading = document.getElementById("script-heading");
        btn.addEventListener("click", function() {
            count = count + 1;
            btn.innerText = "Click Count: " + count;
            heading.innerText = "JavaScript Triggered! Count = " + count;
        });
      </script>
      
      <p class="note">See <a href="https://limpet.net/mbrubeck/">Matt Brubeck's series</a> for browser engine architecture.</p>
    </main>
    <footer>
      <p>Toy Browser Engine — Interactive &amp; Standards-Capable</p>
    </footer>
  </body>
</html>"#;

const DEMO_CSS: &str = r#"
body    { background-color: #f8f9fa; font-family: sans-serif; }
header  { background-image: linear-gradient(to right, #1a2a3a, #2c3e50); padding: 24px; text-align: center; border-radius: 0; }
h1      { color: #ffffff; }
main    { padding: 24px; max-width: 760px; margin: 0 auto; }
h2      { color: #2c3e50; margin-top: 24px; }
.intro  { color: #555555; line-height: 1.5; }

.grid-container {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    gap: 16px;
    margin: 16px 0;
}

.card {
    background-color: #ffffff;
    padding: 16px;
    border-radius: 8px;
    box-shadow: 0px 4px 12px rgba(0, 0, 0, 0.12);
    margin-bottom: 16px;
}

.card h3 { color: #2980b9; margin: 0 0 8px 0; font-size: 16px; }
.card p  { color: #666666; font-size: 13px; margin: 0 0 12px 0; }

.demo-table {
    display: table;
    width: 100%;
    margin: 16px 0;
    border-color: #cbd5e1;
    border-width: 1px;
}

tr { display: table-row; }
th, td { display: table-cell; padding: 8px 12px; border-width: 1px; border-color: #cbd5e1; }
th { background-color: #2c3e50; color: #ffffff; }
td { background-color: #ffffff; color: #333333; }

img {
    width: 240px;
    height: 48px;
    margin: 12px 0;
    border-radius: 6px;
}

.btn {
    background-color: #3498db;
    color: #ffffff;
    border-radius: 4px;
    padding: 6px 12px;
}

.btn:hover {
    background-color: #1a2a3a;
    color: #f1c40f;
}

.features {
    background-color: #eef2f5;
    padding: 18px;
    border-color: #cbd5e1;
    border-width: 1px;
    border-radius: 8px;
    box-shadow: 0px 2px 8px rgba(0, 0, 0, 0.06);
    overflow: hidden;
}

.note   { color: #7f8c8d; text-align: right; }
footer  { background-color: #2c3e50; padding: 14px; text-align: center; border-radius: 0; }
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
    let mut dom = parse_html(&html_src);
    let event_listeners = browser_engine::script::execute_dom_scripts(&mut dom);

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
            if let Err(error) = show_interactive_window(dom, &stylesheet, event_listeners) {
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

fn show_interactive_window(
    mut dom: browser_engine::dom::Node,
    stylesheet: &browser_engine::css::parser::Stylesheet,
    event_listeners: Vec<(String, String, browser_engine::script::JsValue)>,
) -> Result<(), minifb::Error> {
    let mut window = Window::new(
        "Browser Engine Toy (Interactive -- JS Events, Scroll & Hover)",
        800,
        600,
        WindowOptions::default(),
    )?;

    let mut scroll_y: f32 = 0.0;
    let mut was_mouse_down = false;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        if let Some((_, mouse_y)) = window.get_scroll_wheel() {
            if mouse_y != 0.0 {
                scroll_y = (scroll_y - mouse_y * 30.0).max(0.0);
            }
        }
        if window.is_key_down(Key::Down) {
            scroll_y += 15.0;
        }
        if window.is_key_down(Key::Up) {
            scroll_y = (scroll_y - 15.0).max(0.0);
        }

        let is_mouse_down = window.get_mouse_down(minifb::MouseButton::Left);
        let mouse_pos = window.get_mouse_pos(minifb::MouseMode::Pass);

        let clicked_id = if is_mouse_down && !was_mouse_down {
            let initial_styled = style_tree(&dom, stylesheet);
            let initial_layout = layout_tree(&initial_styled, 800.0);
            if let Some((mx, my)) = mouse_pos {
                if let Some(target_node) = initial_layout.hit_test(mx, my + scroll_y) {
                    if let browser_engine::dom::NodeType::Element(ref e) = target_node.node_type {
                        e.get_attr("id").map(String::from)
                    } else { None }
                } else { None }
            } else { None }
        } else { None };

        if let Some(ref target_id) = clicked_id {
            for (el_id, evt_type, handler) in &event_listeners {
                if el_id == target_id && evt_type == "click" {
                    if let browser_engine::script::JsValue::Function { params: _, body } = handler {
                        let mut interp = browser_engine::script::JsInterpreter::new(&mut dom);
                        interp.eval_program(body);
                    }
                }
            }
        }
        was_mouse_down = is_mouse_down;

        let initial_styled = style_tree(&dom, stylesheet);
        let initial_layout = layout_tree(&initial_styled, 800.0);

        let cur_hovered_node = if let Some((mx, my)) = mouse_pos {
            initial_layout.hit_test(mx, my + scroll_y)
        } else {
            None
        };

        let interaction = browser_engine::style::InteractionState {
            hovered_node: cur_hovered_node,
            active_node: if is_mouse_down { cur_hovered_node } else { None },
        };

        let styled = browser_engine::style::style_tree_with_interaction(&dom, stylesheet, &interaction);
        let layout = layout_tree(&styled, 800.0);
        let canvas = browser_engine::paint::paint_with_scroll(&layout, 800, 600, 0.0, scroll_y);

        let buffer = canvas.to_u32_buffer();
        window.update_with_buffer(&buffer, 800, 600)?;
    }
    Ok(())
}

fn print_layout(lb: &LayoutBox, depth: usize) {
    let indent = "  ".repeat(depth);
    let d = &lb.dimensions;

    let label = match &lb.box_type {
        BoxType::Block(s)       => format!("Block({})", node_label(s.node)),
        BoxType::Flex(s)        => format!("Flex({})", node_label(s.node)),
        BoxType::Grid(s)        => format!("Grid({})", node_label(s.node)),
        BoxType::Table(s)       => format!("Table({})", node_label(s.node)),
        BoxType::TableRow(s)    => format!("TableRow({})", node_label(s.node)),
        BoxType::TableCell(s)   => format!("TableCell({})", node_label(s.node)),
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
