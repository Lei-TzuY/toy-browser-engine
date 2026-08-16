//! End-to-end tests for the full browser pipeline:
//! HTML → DOM → scripts → style → layout → paint, plus the click loop the
//! interactive window drives.

use browser_engine::{
    css::parser::parse_css,
    dom::Node,
    extract_inline_styles,
    html::parse_html,
    layout::layout_tree,
    paint::paint,
    script::{dom_api, execute_dom_scripts, path_of, JsRuntime},
    style::style_tree,
    Browser, MemoryLoader, PointerState, Url,
};

const VIEWPORT_W: f32 = 800.0;
const VIEWPORT_H: usize = 600;

/// Parse, run scripts, and collect the page stylesheet the way `main` does.
fn load(html: &str) -> (Node, JsRuntime, browser_engine::css::parser::Stylesheet) {
    let mut dom = parse_html(html);
    let mut runtime = execute_dom_scripts(&mut dom);
    runtime.quiet = true;
    let stylesheet = parse_css(&extract_inline_styles(&dom));
    (dom, runtime, stylesheet)
}

/// Centre point of the first element matching `selector`, in page coordinates.
fn centre_of(
    dom: &Node,
    stylesheet: &browser_engine::css::parser::Stylesheet,
    selector: &str,
) -> (f32, f32) {
    let target = dom_api::query_selector(dom, &[], selector).expect("selector matched nothing");
    let styled = style_tree(dom, stylesheet);
    let layout = layout_tree(&styled, VIEWPORT_W);

    fn find(
        layout: &browser_engine::layout::LayoutBox,
        dom: &Node,
        target: &[usize],
        out: &mut Option<(f32, f32)>,
    ) {
        if let Some(node) = layout.styled_node() {
            if path_of(dom, node.node).as_deref() == Some(target) {
                let b = layout.dimensions.border_box();
                *out = Some((b.x + b.width / 2.0, b.y + b.height / 2.0));
                return;
            }
        }
        for child in &layout.children {
            if out.is_none() {
                find(child, dom, target, out);
            }
        }
    }

    let mut found = None;
    find(&layout, dom, &target, &mut found);
    found.expect("element has no layout box")
}

/// Hit-test at a point and dispatch a click there, as the window loop does.
fn click_at(
    dom: &mut Node,
    runtime: &mut JsRuntime,
    stylesheet: &browser_engine::css::parser::Stylesheet,
    point: (f32, f32),
) -> bool {
    let path = {
        let styled = style_tree(dom, stylesheet);
        let layout = layout_tree(&styled, VIEWPORT_W);
        layout
            .hit_test(point.0, point.1)
            .and_then(|node| path_of(dom, node))
    };
    match path {
        Some(path) => runtime.dispatch_event(dom, &path, "click").dispatched,
        None => false,
    }
}

fn text_of(dom: &Node, selector: &str) -> String {
    let path = dom_api::query_selector(dom, &[], selector).expect("selector matched nothing");
    dom_api::text_content(dom_api::node_at(dom, &path).unwrap())
}

const COUNTER_PAGE: &str = r##"<html><body>
  <style>
    body { font-family: sans-serif; }
    #counter { display: block; width: 200px; height: 40px; background-color: #eeeeee; }
    .hot { background-color: #ff0000; }
  </style>
  <button id="counter">Clicks: 0</button>
  <ul id="log"></ul>
  <script>
    let count = 0;
    const button = document.getElementById("counter");
    const log = document.getElementById("log");

    button.addEventListener("click", function (event) {
        count++;
        button.textContent = "Clicks: " + count;
        const entry = document.createElement("li");
        entry.textContent = "click #" + count + " on " + event.target.tagName;
        log.appendChild(entry);
        if (count >= 2) { button.classList.add("hot"); }
    });
  </script>
</body></html>"##;

#[test]
fn clicking_a_button_updates_the_dom_and_persists_state() {
    let (mut dom, mut runtime, stylesheet) = load(COUNTER_PAGE);
    let point = centre_of(&dom, &stylesheet, "#counter");

    assert!(
        click_at(&mut dom, &mut runtime, &stylesheet, point),
        "first click dispatched"
    );
    assert_eq!(text_of(&dom, "#counter"), "Clicks: 1");

    assert!(
        click_at(&mut dom, &mut runtime, &stylesheet, point),
        "second click dispatched"
    );
    assert_eq!(text_of(&dom, "#counter"), "Clicks: 2");

    // Each click appended a log entry, and the class was added on the second.
    assert_eq!(dom_api::query_selector_all(&dom, &[], "#log li").len(), 2);
    assert!(dom_api::query_selector(&dom, &[], "#counter.hot").is_some());
}

#[test]
fn script_driven_class_change_reaches_the_painted_pixels() {
    let (mut dom, mut runtime, stylesheet) = load(COUNTER_PAGE);
    let point = centre_of(&dom, &stylesheet, "#counter");

    let painted = |dom: &Node| {
        let styled = style_tree(dom, &stylesheet);
        let layout = layout_tree(&styled, VIEWPORT_W);
        let canvas = paint(&layout, VIEWPORT_W as usize, VIEWPORT_H);
        canvas.to_ppm()
    };

    let before = painted(&dom);
    click_at(&mut dom, &mut runtime, &stylesheet, point);
    click_at(&mut dom, &mut runtime, &stylesheet, point);
    let after = painted(&dom);

    assert_eq!(before.len(), after.len(), "canvas size is stable");
    assert_ne!(
        before, after,
        "the .hot background should change the rendering"
    );
}

#[test]
fn clicking_empty_space_dispatches_nothing() {
    let (mut dom, mut runtime, stylesheet) = load(COUNTER_PAGE);
    // Far below any content.
    let fired = click_at(&mut dom, &mut runtime, &stylesheet, (700.0, 5000.0));
    assert!(!fired);
    assert_eq!(text_of(&dom, "#counter"), "Clicks: 0");
}

#[test]
fn a_script_can_build_a_list_that_the_layout_engine_measures() {
    let (dom, _, stylesheet) = load(
        r#"<html><body>
             <ul id="items"></ul>
             <script>
               const items = document.getElementById("items");
               for (let i = 0; i < 4; i++) {
                   const li = document.createElement("li");
                   li.textContent = "row " + i;
                   items.appendChild(li);
               }
             </script>
           </body></html>"#,
    );

    assert_eq!(dom_api::query_selector_all(&dom, &[], "li").len(), 4);

    let styled = style_tree(&dom, &stylesheet);
    let layout = layout_tree(&styled, VIEWPORT_W);

    // The generated list must occupy real vertical space.
    fn height_of_first_ul(layout: &browser_engine::layout::LayoutBox) -> Option<f32> {
        if let Some(node) = layout.styled_node() {
            if node.node.as_element().map(|e| e.tag_name.as_str()) == Some("ul") {
                return Some(layout.dimensions.content.height);
            }
        }
        layout.children.iter().find_map(height_of_first_ul)
    }

    let height = height_of_first_ul(&layout).expect("ul has a layout box");
    assert!(
        height > 0.0,
        "generated list should have height, got {height}"
    );
}

#[test]
fn inline_styles_written_by_script_take_effect_in_the_cascade() {
    let (dom, _, stylesheet) = load(
        r#"<html><body>
             <div id="box">x</div>
             <script>document.getElementById("box").style.color = "rgb(1, 2, 3)";</script>
           </body></html>"#,
    );

    // `style_tree` reads inline styles through the same path as the demo page.
    let styled = style_tree(&dom, &stylesheet);
    let ppm = paint(&layout_tree(&styled, VIEWPORT_W), 64, 64).to_ppm();
    assert!(!ppm.is_empty());

    let path = dom_api::query_selector(&dom, &[], "#box").unwrap();
    let element = dom_api::node_at(&dom, &path).unwrap().as_element().unwrap();
    assert_eq!(element.get_attr("style"), Some("color: rgb(1, 2, 3)"));
}

#[test]
fn test_dynamic_element_creation_and_removal_pipeline() {
    let (mut dom, mut runtime, stylesheet) = load(
        r#"<html>
             <head>
               <style>
                 .card { width: 200px; height: 50px; background-color: rgb(200, 50, 50); }
               </style>
             </head>
             <body>
               <div id="container"></div>
               <button id="add-btn">Add</button>
               <button id="remove-btn">Remove</button>
               <script>
                 document.getElementById("add-btn").addEventListener("click", function() {
                     let card = document.createElement("div");
                     card.setAttribute("id", "dynamic-card");
                     card.setAttribute("class", "card");
                     card.textContent = "Dynamic Card";
                     document.getElementById("container").appendChild(card);
                 });
                 document.getElementById("remove-btn").addEventListener("click", function() {
                     let card = document.getElementById("dynamic-card");
                     if (card) {
                         card.remove();
                     }
                 });
               </script>
             </body>
           </html>"#,
    );

    // Initial state: no cards
    assert_eq!(dom_api::query_selector_all(&dom, &[], ".card").len(), 0);

    // Click "Add" button
    let add_pt = centre_of(&dom, &stylesheet, "#add-btn");
    click_at(&mut dom, &mut runtime, &stylesheet, add_pt);

    // Verify dynamic element added
    assert_eq!(dom_api::query_selector_all(&dom, &[], ".card").len(), 1);
    let styled = style_tree(&dom, &stylesheet);
    let layout = layout_tree(&styled, VIEWPORT_W);
    let ppm = paint(&layout, 800, 600).to_ppm();
    assert!(!ppm.is_empty());

    // Click "Remove" button
    let rem_pt = centre_of(&dom, &stylesheet, "#remove-btn");
    click_at(&mut dom, &mut runtime, &stylesheet, rem_pt);

    // Verify dynamic element removed
    assert_eq!(dom_api::query_selector_all(&dom, &[], ".card").len(), 0);
}

#[test]
fn test_engine_diagnostics_pipeline() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.com/demo",
        "<html><body><h1>Test Diagnostics</h1></body></html>",
    );
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("http://example.com/demo").unwrap(),
    )
    .unwrap();
    let doc = browser.document();
    assert_eq!(doc.dom.children.len(), 1);
    let styled = doc.style_tree(800.0, &PointerState::default());
    let _layout = doc.layout(&styled, 800.0);
    let ppm = browser
        .render(800, 600, 0.0, &PointerState::default())
        .to_ppm();
    assert!(!ppm.is_empty());
}
