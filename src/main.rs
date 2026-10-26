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
    browser::Browser,
    document::PointerState,
    dom::NodeType,
    layout::{BoxType, LayoutBox},
    net::{url_from_argument, DefaultLoader, LoadError, Url},
};
use minifb::{CursorStyle, Key, Window, WindowOptions};

mod demo;
mod location_input;
mod platform;
mod tab_strip;
mod tabs;
mod toolbar;

use tab_strip::{route_window, TabStrip, WindowPoint, TAB_STRIP_HEIGHT};
use tabs::{is_tab_shortcut_key, Applied, TabCommand, Tabs};
use toolbar::{compose, Command, TOOLBAR_HEIGHT};

const USAGE: &str = "\
Usage: browser_engine [options] [<url-or-file> [<out.ppm>]]

  <url-or-file>   file path, file://, http:// or https:// URL (default: the built-in demo site)
  <out.ppm>       write the rendered page here (default: output.ppm)

Options:
  --window        interactive window with tabs and a toolbar (click the
                  address or Ctrl+L to edit, Enter: go, Esc: cancel/close;
                  Ctrl+T: new tab, Ctrl+W: close tab, Ctrl+Tab /
                  Ctrl+Shift+Tab: next/previous tab, Ctrl+1…9: pick a tab)
  --size WxH      canvas and viewport size (default 800x600); the window
                  adds the tab strip and toolbar above the viewport
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

    let url = match &options.target {
        Some(target) => url_from_argument(target),
        None => Url::parse(demo::ENTRY_URL).expect("valid demo URL"),
    };
    let demo_site = options.target.is_none();

    let mut browser = match Browser::open(Box::new(loader(demo_site)), &url) {
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
        // Every new tab starts its own session at the page the window opened
        // with, through a loader of its own.
        let home = url.clone();
        let open_tab = move || Browser::open(Box::new(loader(demo_site)), &home);
        // The CLI output that follows describes the tab that was active when
        // the window closed.
        browser = match run_window(browser, open_tab, options.width, options.height) {
            Ok(browser) => browser,
            Err(error) => {
                eprintln!("\nFailed to open window: {error}");
                process::exit(1);
            }
        };
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

/// The loader a session uses. Without a target the engine serves its own
/// demo site out of memory, so the default run still exercises the
/// resource-loading pipeline.
fn loader(demo_site: bool) -> DefaultLoader {
    if demo_site {
        DefaultLoader::new().with_memory(demo::site())
    } else {
        DefaultLoader::new()
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

/// With Ctrl, these pick a tab by position.
const NUMBER_KEYS: [Key; 9] = [
    Key::Key1,
    Key::Key2,
    Key::Key3,
    Key::Key4,
    Key::Key5,
    Key::Key6,
    Key::Key7,
    Key::Key8,
    Key::Key9,
];

/// Drive the window until it is closed, then hand back the session of the
/// tab that was active at the time. `open_tab` starts each new tab.
fn run_window(
    browser: Browser,
    open_tab: impl Fn() -> Result<Browser, LoadError>,
    width: usize,
    height: usize,
) -> Result<Browser, minifb::Error> {
    // The page keeps the full `--size` viewport; the tab strip and toolbar
    // sit above it.
    let window_height = TAB_STRIP_HEIGHT + TOOLBAR_HEIGHT + height;
    let mut window = Window::new(
        &browser.status_line(),
        width,
        window_height,
        WindowOptions::default(),
    )?;
    // Pace the loop so animations run at a steady rate and an idle page does
    // not spin the CPU. minifb sleeps for us inside `update_with_buffer`.
    window.set_target_fps(FRAME_RATE);
    let input = platform::InputAdapter::attach(&mut window);

    let mut tabs = Tabs::new(browser, width);
    let mut strip = TabStrip::new(width);
    let mut was_mouse_down = false;
    let mut was_middle_down = false;
    // Whether the button went down over the page, so dragging in from the
    // chrome does not paint page elements as :active.
    let mut press_on_page = false;
    let mut title = String::new();
    let mut text_cursor = false;
    // A window-level message, such as a new tab that failed to load. It
    // leads the title until the next tab action.
    let mut notice: Option<String> = None;

    while window.is_open() {
        let chrome = window.is_key_down(Key::LeftAlt) || window.is_key_down(Key::RightAlt);
        let ctrl = window.is_key_down(Key::LeftCtrl) || window.is_key_down(Key::RightCtrl);
        let shift = window.is_key_down(Key::LeftShift) || window.is_key_down(Key::RightShift);

        // ── Tab shortcuts ─────────────────────────────────────────────────
        // First, so the rest of the frame already works on the tab they
        // pick. A frame with one hands no keys to any tab.
        let mut tab_key = None;
        if ctrl && !chrome {
            if window.is_key_pressed(Key::T, minifb::KeyRepeat::No) {
                tab_key = Some(TabCommand::New);
            }
            if window.is_key_pressed(Key::W, minifb::KeyRepeat::No) {
                tab_key = Some(TabCommand::Close(tabs.active_index()));
            }
            if window.is_key_pressed(Key::Tab, minifb::KeyRepeat::Yes) {
                tab_key = Some(if shift {
                    TabCommand::Previous
                } else {
                    TabCommand::Next
                });
            }
            for (index, &key) in NUMBER_KEYS.iter().enumerate() {
                if window.is_key_pressed(key, minifb::KeyRepeat::No) {
                    tab_key = Some(TabCommand::Number(index + 1));
                }
            }
        }
        if let Some(command) = tab_key {
            if !apply_tab_command(&mut tabs, &strip, command, &open_tab, &mut notice) {
                break;
            }
            press_on_page = false;
        }

        let count = tabs.len();
        let tab = tabs.active_mut();

        // Escape cancels address entry; outside entry it retains the window's
        // existing Escape-to-close shortcut.
        if !tab.toolbar.is_editing() && window.is_key_pressed(Key::Escape, minifb::KeyRepeat::No) {
            break;
        }
        // ── Browser chrome shortcuts ──────────────────────────────────────
        // These are the window's own keys; everything else goes to the page.
        if let Some((_, wheel_y)) = window.get_scroll_wheel() {
            if wheel_y != 0.0 {
                tab.scroll_y -= wheel_y * 30.0;
            }
        }
        let address_opened = ctrl && window.is_key_pressed(Key::L, minifb::KeyRepeat::No);
        if address_opened {
            // The current URL starts selected, like a familiar address field.
            // All editor keys stay in window chrome, never in the page's DOM.
            let current = tab.browser.url().to_string();
            tab.toolbar.focus_address(&current);
        }
        if tab.toolbar.is_editing() && ctrl && window.is_key_pressed(Key::A, minifb::KeyRepeat::No)
        {
            // Ctrl+A arrives as a control character, which the text channel
            // drops, so select-all is read from the physical key.
            tab.toolbar.select_all();
        }
        let mut command = None;
        if chrome && !tab.toolbar.is_editing() {
            if window.is_key_pressed(Key::Left, minifb::KeyRepeat::No) {
                command = Some(Command::Back);
            }
            if window.is_key_pressed(Key::Right, minifb::KeyRepeat::No) {
                command = Some(Command::Forward);
            }
            if window.is_key_pressed(Key::R, minifb::KeyRepeat::No) {
                command = Some(Command::Reload);
            }
            if window.is_key_pressed(Key::Down, minifb::KeyRepeat::Yes) {
                tab.scroll_y += 40.0;
            }
            if window.is_key_pressed(Key::Up, minifb::KeyRepeat::Yes) {
                tab.scroll_y -= 40.0;
            }
        }

        // ── Keyboard → address field or page ─────────────────────────────
        // Consume once per frame, even for Alt shortcuts, so old text input
        // cannot leak into a document on a later frame. Tab shortcut keys,
        // including their auto-repeats, never reach a tab.
        let mut events = input.drain(&window);
        events.retain(|event| !is_tab_shortcut_key(event));
        if !address_opened && tab_key.is_none() {
            command = command.or(tab.handle_keys(events, chrome));
        }

        // ── Pointer → tab strip, toolbar or page ──────────────────────────
        let is_mouse_down = window.get_mouse_down(minifb::MouseButton::Left);
        let is_middle_down = window.get_mouse_down(minifb::MouseButton::Middle);
        let position = window.get_mouse_pos(minifb::MouseMode::Pass);
        let point = position.map(|(x, y)| route_window(&tab.toolbar, x, y, height, tab.scroll_y));
        match position {
            Some((x, y)) => {
                strip.pointer_moved(x, y, count);
                tab.toolbar.pointer_moved(x, y - TAB_STRIP_HEIGHT as f32);
            }
            None => {
                strip.pointer_left();
                tab.toolbar.pointer_left();
            }
        }
        if tab.toolbar.over_address() != text_cursor {
            text_cursor = tab.toolbar.over_address();
            window.set_cursor_style(if text_cursor {
                CursorStyle::Ibeam
            } else {
                CursorStyle::Arrow
            });
        }

        let mut tab_click = None;
        if is_mouse_down && !was_mouse_down {
            press_on_page = false;
            match point {
                Some(WindowPoint::TabStrip { x, y }) => tab_click = strip.press(x, y, count),
                Some(WindowPoint::Toolbar { x, y }) => {
                    let current = tab.browser.url().to_string();
                    tab.toolbar.press(x, y, &current, shift);
                }
                Some(WindowPoint::Page { x, y }) => {
                    if tab.toolbar.is_editing() {
                        // A click outside the address field closes it without
                        // also activating the page's link or button.
                        tab.toolbar.cancel();
                    } else {
                        press_on_page = true;
                        let outcome = tab.browser.click_at(x, y, width as f32);
                        tab.report_page_outcome(outcome);
                    }
                }
                _ => {}
            }
        }
        if !is_mouse_down && was_mouse_down {
            let (x, y) = position.unwrap_or((-1.0, -1.0));
            tab_click = tab_click.or(strip.release(x, y, count));
            let navigation = tab.navigation();
            let toolbar_y = y - TAB_STRIP_HEIGHT as f32;
            command = command.or(tab.toolbar.release(x, toolbar_y, navigation));
        }
        if is_middle_down && !was_middle_down {
            if let Some(WindowPoint::TabStrip { x, y }) = point {
                strip.press_middle(x, y, count);
            }
        }
        if !is_middle_down && was_middle_down {
            let (x, y) = position.unwrap_or((-1.0, -1.0));
            tab_click = tab_click.or(strip.release_middle(x, y, count));
        }
        was_mouse_down = is_mouse_down;
        was_middle_down = is_middle_down;

        if let Some(command) = command {
            tab.run_command(command);
        }
        if let Some(command) = tab_click {
            if !apply_tab_command(&mut tabs, &strip, command, &open_tab, &mut notice) {
                break;
            }
            press_on_page = false;
        }

        // ── Event loop ────────────────────────────────────────────────────
        // Timers and animation frames run here, between input and paint, so
        // whatever a callback changed is in the frame that follows. Hidden
        // tabs keep running, as in other browsers.
        for tab in tabs.iter_mut() {
            tab.browser.tick();
        }

        // ── Draw ──────────────────────────────────────────────────────────
        let tab = tabs.active_mut();
        let max_scroll =
            (tab.browser.document().content_height(width as f32) - height as f32).max(0.0);
        tab.scroll_y = tab.scroll_y.clamp(0.0, max_scroll);

        // Hover is worked out against the tab on screen now, which may not
        // be the one the pointer was routed to earlier in the frame.
        let page_point = position.and_then(|(x, y)| {
            match route_window(&tab.toolbar, x, y, height, tab.scroll_y) {
                WindowPoint::Page { x, y } => Some((x, y)),
                _ => None,
            }
        });
        let hovered =
            page_point.and_then(|(x, y)| tab.browser.document().hit_test(x, y, width as f32));
        let active = (is_mouse_down && press_on_page)
            .then(|| hovered.clone())
            .flatten();
        let pointer = PointerState {
            focused: active.clone().or(hovered.clone()),
            active,
            hovered,
        };

        let page = tab.browser.render(width, height, tab.scroll_y, &pointer);
        let navigation = tab.navigation();
        let bar = tab
            .toolbar
            .paint(&tab.browser.url().to_string(), navigation);
        let mut frame = strip
            .paint(&tabs.labels(), tabs.active_index())
            .to_u32_buffer();
        frame.extend(compose(&bar, &page));
        window.update_with_buffer(&frame, width, window_height)?;

        let status = match &notice {
            Some(notice) => format!("{notice} — {}", tabs.active().status()),
            None => tabs.active().status(),
        };
        if status != title {
            window.set_title(&status);
            title = status;
        }
    }
    Ok(tabs.into_active())
}

/// Carry out a tab-strip click or tab shortcut. Returns false when the last
/// tab was closed, which closes the window.
fn apply_tab_command(
    tabs: &mut Tabs,
    strip: &TabStrip,
    command: TabCommand,
    open_tab: &impl Fn() -> Result<Browser, LoadError>,
    notice: &mut Option<String>,
) -> bool {
    *notice = None;
    match tabs.apply(command, strip.capacity(), open_tab) {
        Applied::Continue => true,
        Applied::Notice(message) => {
            *notice = Some(message);
            true
        }
        Applied::CloseWindow => false,
    }
}
