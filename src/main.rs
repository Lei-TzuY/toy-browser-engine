// ============================================================
//  main.rs  —  Browser Engine Toy CLI
// ============================================================
//
//  A thin shell around the engine: parse arguments, open a `Browser`, then
//  either dump the trees and write a PPM, or drive an interactive window.
//  Everything else — loading, parsing, styling, layout, paint, navigation —
//  lives in the library.

use std::{env, fs, process};

use browser_engine::{
    browser::{Browser, ClickOutcome},
    browser_chrome::BrowserChromeTracker,
    cursor_assets::CursorResolver,
    cursor_frame::prepare_cursor_frame,
    document::PointerState,
    dom::NodeType,
    layout::{BoxType, LayoutBox},
    net::{url_from_argument, DefaultLoader, Url},
};
use minifb::{Key, Window, WindowOptions};

mod demo;
mod platform;

const USAGE: &str = "\
Usage: browser_engine [options] [<url-or-file> [<out.ppm>]]

  <url-or-file>   file path, file:// or http:// URL (default: the built-in demo site)
  <out.ppm>       write the rendered page here (default: output.ppm)

Options:
  --window        open an interactive window (click links, scroll, Backspace to go back)
  --size WxH      canvas and viewport size (default 800x600)
  --viewport WxH  alias for --size WxH
  --stats         print engine performance and node count diagnostics
  --inspect       dump detailed DOM node and computed CSS style tree
  --screenshot P  render page and save PPM screenshot to P
  --benchmark     execute pipeline stress test and timing statistics
  --json-stats    output structured machine-readable JSON metrics
  --dump-html     serialize and print the DOM tree as HTML markup
  --dump-css      dump all active CSS stylesheet rules
  --quiet         skip the DOM and layout tree dumps
  --help          show this message";

struct Options {
    target: Option<String>,
    output: Option<String>,
    width: usize,
    height: usize,
    show_window: bool,
    show_stats: bool,
    show_inspect: bool,
    show_benchmark: bool,
    show_json_stats: bool,
    show_dump_html: bool,
    show_dump_css: bool,
    quiet: bool,
}

fn main() {
    let options = match parse_options(env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            process::exit(2);
        }
    };

    // Without a target the engine serves its own demo site out of memory, so
    // the default run still exercises the resource-loading pipeline.
    let (loader, url) = match &options.target {
        Some(target) => (DefaultLoader::new(), url_from_argument(target)),
        None => (
            DefaultLoader::new().with_memory(demo::site()),
            Url::parse(demo::ENTRY_URL).expect("valid demo URL"),
        ),
    };

    let mut browser = match Browser::open(Box::new(loader), &url) {
        Ok(browser) => browser,
        Err(error) => {
            eprintln!("Failed to load {url}: {error}");
            process::exit(1);
        }
    };

    for diagnostic in &browser.document().diagnostics {
        eprintln!("warning: {diagnostic}");
    }

    if !options.quiet {
        print_document(&browser, options.width);
    }

    let output = options.output.clone().or_else(|| {
        // Only the default demo run writes a file without being asked to.
        options.target.is_none().then(|| "output.ppm".to_string())
    });
    if let Some(path) = output {
        let canvas = browser.render(options.width, options.height, 0.0, &PointerState::default());
        let ppm = canvas.to_ppm();
        match fs::write(&path, &ppm) {
            Ok(()) => println!(
                "\nPainted → {path} ({}×{}, {} bytes)",
                options.width,
                options.height,
                ppm.len()
            ),
            Err(error) => eprintln!("\nFailed to write {path}: {error}"),
        }
    }

    if options.show_window {
        if let Err(error) = run_window(&mut browser, options.width, options.height) {
            eprintln!("\nFailed to open window: {error}");
            process::exit(1);
        }
    }

    if options.show_stats {
        print_stats(&browser, options.width, options.height);
    }

    if options.show_inspect {
        print_inspect(&browser, options.width);
    }

    if options.show_benchmark {
        run_benchmark(&browser, options.width, options.height);
    }

    if options.show_json_stats {
        print_json_stats(&browser, options.width, options.height);
    }

    if options.show_dump_html {
        print_dump_html(&browser);
    }

    if options.show_dump_css {
        print_dump_css(&browser);
    }

    if !options.quiet {
        println!("\nDone.");
    }
}

fn print_dump_css(browser: &Browser) {
    let doc = browser.document();
    println!("/* ================================================== */");
    println!("/*           MERGED CSS STYLESHEET RULES              */");
    println!("/* ================================================== */");
    for rule in &doc.stylesheet.rules {
        let sel_str: Vec<String> = rule.selectors.iter().map(|s| format!("{s:?}")).collect();
        println!("/* {} */", sel_str.join(", "));
        for decl in &rule.declarations {
            println!("  {}: {:?};", decl.name, decl.value);
        }
    }
}

fn print_dump_html(browser: &Browser) {
    let doc = browser.document();
    println!("{}", browser_engine::script::dom_api::outer_html(&doc.dom));
}

fn print_json_stats(browser: &Browser, width: usize, height: usize) {
    let doc = browser.document();
    let dom_count = count_dom_nodes(&doc.dom);
    let styled = doc.style_tree(width as f32, &PointerState::default());
    let layout_root = doc.layout(&styled, width as f32);
    let layout_count = count_layout_boxes(&layout_root);

    println!(
        r#"{{"width":{},"height":{},"dom_nodes":{},"layout_boxes":{},"url":{:?},"status":"ok"}}"#,
        width,
        height,
        dom_count,
        layout_count,
        browser.url()
    );
}

fn run_benchmark(browser: &Browser, width: usize, height: usize) {
    use std::time::Instant;
    println!("\n==================================================");
    println!("          ENGINE BENCHMARK STRESS TEST            ");
    println!("==================================================");
    let start = Instant::now();
    let iterations = 50;
    for _ in 0..iterations {
        let doc = browser.document();
        let styled = doc.style_tree(width as f32, &PointerState::default());
        let _layout = doc.layout(&styled, width as f32);
        let _ppm = browser
            .render(width, height, 0.0, &PointerState::default())
            .to_ppm();
    }
    let total = start.elapsed();
    let avg = total / iterations;
    println!("Iterations     : {}", iterations);
    println!("Total Elapsed  : {:.2?}", total);
    println!("Average Frame  : {:.2?}", avg);
    println!("FPS Estimate   : {:.1}", 1.0 / avg.as_secs_f32());
    println!("==================================================\n");
}

fn print_inspect(browser: &Browser, width: usize) {
    let doc = browser.document();
    let styled = doc.style_tree(width as f32, &PointerState::default());
    println!("\n==================================================");
    println!("          DOM & COMPUTED CSS INSPECTOR            ");
    println!("==================================================");
    dump_styled_node(&styled, 0);
    println!("==================================================\n");
}

fn dump_styled_node(sn: &browser_engine::style::StyledNode, indent: usize) {
    let pad = "  ".repeat(indent);
    match sn.node.node_type {
        browser_engine::dom::NodeType::Document => {
            println!("{}#document", pad);
        }
        browser_engine::dom::NodeType::Text(ref text) => {
            let t = text.trim();
            if t.is_empty() {
                return;
            }
            println!("{}Text({:?})", pad, t);
        }
        browser_engine::dom::NodeType::Element(ref el) => {
            let id = el
                .get_attr("id")
                .map(|i| format!("#{}", i))
                .unwrap_or_default();
            let classes = el
                .get_attr("class")
                .map(|c| format!(".{}", c.replace(' ', ".")))
                .unwrap_or_default();
            println!("{}<{}{}{}>", pad, el.tag_name, id, classes);
        }
        _ => return,
    };
    for child in &sn.children {
        dump_styled_node(child, indent + 1);
    }
}

fn print_stats(browser: &Browser, width: usize, height: usize) {
    let doc = browser.document();
    let dom_count = count_dom_nodes(&doc.dom);
    let styled = doc.style_tree(width as f32, &PointerState::default());
    let layout_root = doc.layout(&styled, width as f32);
    let layout_count = count_layout_boxes(&layout_root);
    println!("\n==================================================");
    println!("          ENGINE DIAGNOSTICS & METRICS            ");
    println!("==================================================");
    println!("Target URL     : {}", browser.url());
    println!("Viewport Size  : {}x{}", width, height);
    println!("DOM Tree Nodes : {}", dom_count);
    println!("Layout Boxes   : {}", layout_count);
    println!("Diagnostics    : {} warning(s)", doc.diagnostics.len());
    println!("==================================================\n");
}

fn count_dom_nodes(node: &browser_engine::dom::Node) -> usize {
    1 + node.children.iter().map(count_dom_nodes).sum::<usize>()
}

fn count_layout_boxes(lb: &browser_engine::layout::LayoutBox) -> usize {
    1 + lb.children.iter().map(count_layout_boxes).sum::<usize>()
}

// ── Arguments ─────────────────────────────────────────────────────────────────

fn parse_options(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        target: None,
        output: None,
        width: 800,
        height: 600,
        show_window: false,
        show_stats: false,
        show_inspect: false,
        show_benchmark: false,
        show_json_stats: false,
        show_dump_html: false,
        show_dump_css: false,
        quiet: false,
    };
    let mut positional: Vec<String> = Vec::new();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--window" => options.show_window = true,
            "--stats" => options.show_stats = true,
            "--inspect" => options.show_inspect = true,
            "--benchmark" => options.show_benchmark = true,
            "--json-stats" => options.show_json_stats = true,
            "--dump-html" => options.show_dump_html = true,
            "--dump-css" => options.show_dump_css = true,
            "--quiet" => options.quiet = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                process::exit(0);
            }
            "--size" | "--viewport" => {
                let value = args.next().ok_or("--size / --viewport needs a WxH value")?;
                (options.width, options.height) = parse_size(&value)?;
            }
            other if other.starts_with("--size=") || other.starts_with("--viewport=") => {
                let val = other
                    .strip_prefix("--size=")
                    .or_else(|| other.strip_prefix("--viewport="))
                    .unwrap_or_default();
                (options.width, options.height) = parse_size(val)?;
            }
            "--screenshot" => {
                let path = args.next().ok_or("--screenshot needs an output filepath")?;
                options.output = Some(path);
            }
            other if other.starts_with("--screenshot=") => {
                options.output = Some(other["--screenshot=".len()..].to_string());
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => positional.push(other.to_string()),
        }
    }

    match positional.len() {
        0 => {}
        1 => options.target = Some(positional.remove(0)),
        2 => {
            options.output = Some(positional.pop().unwrap());
            options.target = Some(positional.pop().unwrap());
        }
        _ => return Err("too many arguments".into()),
    }
    Ok(options)
}

fn parse_size(spec: &str) -> Result<(usize, usize), String> {
    let (w, h) = spec
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("malformed --size {spec:?} (expected WxH)"))?;
    match (w.trim().parse::<usize>(), h.trim().parse::<usize>()) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => Err(format!("malformed --size {spec:?} (expected WxH)")),
    }
}

// ── Console output ────────────────────────────────────────────────────────────

fn print_document(browser: &Browser, width: usize) {
    println!("╔══════════════════════════════╗");
    println!("║         DOM  TREE            ║");
    println!("╚══════════════════════════════╝");
    println!("{}", browser.status_line());
    browser.document().dom.pretty_print();

    println!("\n╔══════════════════════════════╗");
    println!("║       LAYOUT  TREE           ║");
    println!("╚══════════════════════════════╝");
    let document = browser.document();
    let styled = document.style_tree(width as f32, &PointerState::default());
    let layout = document.layout(&styled, width as f32);
    print_layout(&layout, 0);
}

fn print_layout(lb: &LayoutBox, depth: usize) {
    let indent = "  ".repeat(depth);
    let d = &lb.dimensions;

    let label = match &lb.box_type {
        BoxType::Block(s) => format!("Block({})", node_label(s.node)),
        BoxType::Flex(s) => format!("Flex({})", node_label(s.node)),
        BoxType::Grid(s) => format!("Grid({})", node_label(s.node)),
        BoxType::Table(s) => format!("Table({})", node_label(s.node)),
        BoxType::TableRow(s) => format!("TableRow({})", node_label(s.node)),
        BoxType::TableCell(s) => format!("TableCell({})", node_label(s.node)),
        BoxType::Inline(s) => format!("Inline({})", node_label(s.node)),
        BoxType::InlineBlock(s) => format!("InlineBlock({})", node_label(s.node)),
        BoxType::AnonymousBlock => "AnonymousBlock".into(),
    };

    println!(
        "{}{:<36} x={:<6.1} y={:<6.1} w={:<6.1} h={:.1}",
        indent, label, d.content.x, d.content.y, d.content.width, d.content.height,
    );

    for child in &lb.children {
        print_layout(child, depth + 1);
    }
}

fn node_label(node: &browser_engine::dom::Node) -> String {
    match &node.node_type {
        NodeType::Element(e) => {
            let mut s = format!("<{}", e.tag_name);
            if let Some(id) = e.get_attr("id") {
                s.push_str(&format!(" #{id}"));
            }
            if let Some(class) = e.get_attr("class") {
                s.push_str(&format!(
                    " .{}",
                    class.split_whitespace().next().unwrap_or("")
                ));
            }
            s.push('>');
            s
        }
        NodeType::Text(t) => {
            let trimmed = t.trim();
            if trimmed.chars().count() > 16 {
                let cut: String = trimmed.chars().take(16).collect();
                format!("{cut:?}…")
            } else {
                format!("{trimmed:?}")
            }
        }
        NodeType::Document => "#document".into(),
        NodeType::Comment(_) => "<!-- -->".into(),
        NodeType::Doctype(n) => format!("<!DOCTYPE {n}>"),
    }
}

// ── Interactive window ────────────────────────────────────────────────────────

/// Frames per second the window aims for.
///
/// The event loop is ticked once per frame, which is also how often
/// `requestAnimationFrame` callbacks get to run.
const FRAME_RATE: usize = 60;

fn reset_cursor_resolver(resolver: &mut CursorResolver, browser: &Browser) {
    *resolver = CursorResolver::new();
    let report = resolver.preload_stylesheet(browser);
    if report.failed > 0 {
        eprintln!(
            "cursor preload: {} loaded, {} failed",
            report.loaded, report.failed
        );
    }
}

fn run_window(browser: &mut Browser, width: usize, height: usize) -> Result<(), minifb::Error> {
    let mut chrome_tracker = BrowserChromeTracker::new();
    let initial_chrome = chrome_tracker.poll(browser);
    let mut window = Window::new(
        &initial_chrome.state.status_line(),
        width,
        height,
        WindowOptions::default(),
    )?;
    // Pace the loop so animations run at a steady rate and an idle page does
    // not spin the CPU. minifb sleeps for us inside `update_with_buffer`.
    window.set_target_fps(FRAME_RATE);
    let input = platform::InputAdapter::attach(&mut window);
    let mut cursor_resolver = CursorResolver::new();
    let preload = cursor_resolver.preload_stylesheet(browser);
    if preload.failed > 0 {
        eprintln!(
            "cursor preload: {} loaded, {} failed",
            preload.loaded, preload.failed
        );
    }

    let mut scroll_y: f32 = 0.0;
    let mut was_mouse_down = false;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        let mut document_changed = false;

        // ── Browser chrome shortcuts ──────────────────────────────────────
        // These are the window's own keys; everything else goes to the page.
        if let Some((_, wheel_y)) = window.get_scroll_wheel() {
            if wheel_y != 0.0 {
                scroll_y -= wheel_y * 30.0;
            }
        }
        let chrome = window.is_key_down(Key::LeftAlt) || window.is_key_down(Key::RightAlt);
        if chrome {
            if window.is_key_pressed(Key::Left, minifb::KeyRepeat::No) && browser.back() {
                scroll_y = 0.0;
                document_changed = true;
            }
            if window.is_key_pressed(Key::Right, minifb::KeyRepeat::No) && browser.forward() {
                scroll_y = 0.0;
                document_changed = true;
            }
            if window.is_key_pressed(Key::R, minifb::KeyRepeat::No) {
                match browser.reload() {
                    Ok(()) => document_changed = true,
                    Err(error) => eprintln!("reload failed: {error}"),
                }
            }
            if window.is_key_pressed(Key::Down, minifb::KeyRepeat::Yes) {
                scroll_y += 40.0;
            }
            if window.is_key_pressed(Key::Up, minifb::KeyRepeat::Yes) {
                scroll_y -= 40.0;
            }
        }

        // ── Keyboard → page ───────────────────────────────────────────────
        if !chrome {
            for event in input.drain(&window) {
                match browser.press_key(&event) {
                    ClickOutcome::Navigated(url) => {
                        println!("→ {url}");
                        scroll_y = 0.0;
                        document_changed = true;
                    }
                    ClickOutcome::NavigationFailed { url, error } => {
                        eprintln!("could not open {url}: {error}");
                    }
                    ClickOutcome::Script | ClickOutcome::Ignored => {}
                }
            }
        }

        let is_mouse_down = window.get_mouse_down(minifb::MouseButton::Left);
        let mouse = window.get_mouse_pos(minifb::MouseMode::Pass);
        let page_point = mouse.map(|(x, y)| (x, y + scroll_y));

        // ── Click ─────────────────────────────────────────────────────────
        if is_mouse_down && !was_mouse_down {
            if let Some((x, y)) = page_point {
                match browser.click_at(x, y, width as f32) {
                    ClickOutcome::Navigated(url) => {
                        println!("→ {url}");
                        scroll_y = 0.0;
                        document_changed = true;
                    }
                    ClickOutcome::NavigationFailed { url, error } => {
                        eprintln!("could not open {url}: {error}");
                    }
                    ClickOutcome::Script | ClickOutcome::Ignored => {}
                }
            }
        }
        was_mouse_down = is_mouse_down;

        if document_changed {
            reset_cursor_resolver(&mut cursor_resolver, browser);
        }

        // ── Event loop ────────────────────────────────────────────────────
        // Timers and animation frames run here, between input and paint, so
        // whatever a callback changed is in the frame that follows.
        browser.tick();

        // Poll live browser chrome after the event-loop turn: timers/network
        // callbacks may change <title>, navigation state, or the page icon.
        // If such a turn navigated without coming through the input paths
        // above, reset viewport-local state before painting the new document.
        let chrome_update = chrome_tracker.poll(browser);
        if !document_changed && (chrome_update.changes.url || chrome_update.changes.history) {
            scroll_y = 0.0;
            reset_cursor_resolver(&mut cursor_resolver, browser);
        }
        if chrome_update.changes.title || chrome_update.changes.url || chrome_update.changes.history {
            window.set_title(&chrome_update.state.status_line());
        }

        // ── Draw ──────────────────────────────────────────────────────────
        let max_scroll = (browser.document().content_height(width as f32) - height as f32).max(0.0);
        scroll_y = scroll_y.clamp(0.0, max_scroll);

        // Recompute page coordinates after navigation or scroll clamping. The
        // software cursor itself is composited with the original viewport
        // coordinates by `prepare_cursor_frame` below.
        let page_point = mouse.map(|(x, y)| (x, y + scroll_y));
        let hovered = page_point.and_then(|(x, y)| browser.document().hit_test(x, y, width as f32));
        let active = is_mouse_down.then(|| hovered.clone()).flatten();
        let pointer = PointerState {
            focused: active.clone().or(hovered.clone()),
            active,
            hovered,
        };

        let mut canvas = browser.render(width, height, scroll_y, &pointer);
        let cursor_outcome = prepare_cursor_frame(
            &mut cursor_resolver,
            browser,
            &mut canvas,
            pointer.hovered.as_ref(),
            mouse,
            width as f32,
            &pointer,
        );
        platform::apply_cursor_presentation(&mut window, cursor_outcome.presentation);
        window.update_with_buffer(&canvas.to_u32_buffer(), width, height)?;
    }
    Ok(())
}
