// ============================================================
//  script/mod.rs  —  Embedded JavaScript engine
// ============================================================
//
//  Pipeline:
//    source → `lexer::Lexer`  → tokens
//           → `parser::Parser` → `ast::Stmt` program
//           → `interp::JsRuntime` → DOM mutations + event listeners
//
//  The runtime is long-lived: `execute_dom_scripts` runs every inline
//  `<script>` in the document and hands back the `JsRuntime` that ran
//  them, with its global scope and listeners intact.  Event dispatch
//  then re-enters that same runtime, so variables declared at load
//  time (a click counter, cached element handles, …) survive.

pub mod ast;
pub mod dom_api;
pub mod fetch_api;
pub mod host;
pub mod interp;
pub mod json;
pub mod lexer;
pub mod parser;
pub mod promise;

pub use dom_api::{path_of, NodePath};
pub use host::{AbortState, HostObject, RequestData, ResponseData};
pub use interp::{
    to_string as value_to_string, EventOutcome, JsRuntime, JsValue, Listener, NodeRef,
};
pub use parser::Parser as JsParser;

use crate::dom::{Node, NodeType};

/// Run every inline `<script>` in the document, in document order.
///
/// Returns the runtime that executed them so callers can dispatch events
/// into the same global scope later.
pub fn execute_dom_scripts(dom: &mut Node) -> JsRuntime {
    let mut runtime = JsRuntime::new();
    for source in collect_script_sources(dom) {
        runtime.run_script(dom, &source);
    }
    runtime
}

/// Text of every inline `<script>` element, in document order.
///
/// Elements with a `src` attribute are skipped: there is no loader.
pub fn collect_script_sources(node: &Node) -> Vec<String> {
    let mut out = Vec::new();
    collect_scripts_inner(node, &mut out);
    out
}

fn collect_scripts_inner(node: &Node, out: &mut Vec<String>) {
    if let NodeType::Element(e) = &node.node_type {
        if e.tag_name == "script" {
            if e.get_attr("src").is_none() {
                let source: String = node
                    .children
                    .iter()
                    .filter_map(|c| match &c.node_type {
                        NodeType::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect();
                if !source.trim().is_empty() {
                    out.push(source);
                }
            }
            return;
        }
    }
    for child in &node.children {
        collect_scripts_inner(child, out);
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    /// Run `source` against `html` and return the mutated DOM plus the runtime.
    fn run(html: &str, source: &str) -> (Node, JsRuntime) {
        let mut dom = parse_html(html);
        let mut rt = JsRuntime::new();
        rt.quiet = true;
        rt.run_script(&mut dom, source);
        (dom, rt)
    }

    /// Everything a script logged, joined with newlines.
    fn logs(rt: &JsRuntime) -> String {
        rt.console.join("\n")
    }

    fn text_of(dom: &Node, selector: &str) -> String {
        let path = dom_api::query_selector(dom, &[], selector).expect("selector matched nothing");
        dom_api::text_content(dom_api::node_at(dom, &path).unwrap())
    }

    // ── Language ──────────────────────────────────────────────────────────

    #[test]
    fn arithmetic_respects_precedence() {
        let (_, rt) = run("<p></p>", "console.log(1 + 2 * 3 - 4 / 2);");
        assert_eq!(logs(&rt), "5");
    }

    #[test]
    fn string_concatenation_and_numbers() {
        let (_, rt) = run("<p></p>", r#"console.log("n=" + (2 + 3));"#);
        assert_eq!(logs(&rt), "n=5");
    }

    #[test]
    fn if_else_selects_branch() {
        let (_, rt) = run(
            "<p></p>",
            r#"
            let n = 7;
            if (n > 10) { console.log("big"); }
            else if (n > 5) { console.log("medium"); }
            else { console.log("small"); }
            "#,
        );
        assert_eq!(logs(&rt), "medium");
    }

    #[test]
    fn while_loop_accumulates() {
        let (_, rt) = run(
            "<p></p>",
            "let i = 0; let total = 0; while (i < 5) { total = total + i; i++; } console.log(total);",
        );
        assert_eq!(logs(&rt), "10");
    }

    #[test]
    fn for_loop_with_break_and_continue() {
        let (_, rt) = run(
            "<p></p>",
            r#"
            let out = "";
            for (let i = 0; i < 10; i++) {
                if (i === 3) { continue; }
                if (i === 6) { break; }
                out += i;
            }
            console.log(out);
            "#,
        );
        assert_eq!(logs(&rt), "01245");
    }

    #[test]
    fn functions_return_values_and_recurse() {
        let (_, rt) = run(
            "<p></p>",
            r#"
            function fact(n) {
                if (n <= 1) { return 1; }
                return n * fact(n - 1);
            }
            console.log(fact(5));
            "#,
        );
        assert_eq!(logs(&rt), "120");
    }

    #[test]
    fn functions_can_be_called_before_declaration() {
        let (_, rt) = run(
            "<p></p>",
            "console.log(double(4)); function double(x) { return x * 2; }",
        );
        assert_eq!(logs(&rt), "8");
    }

    #[test]
    fn closures_capture_their_defining_scope() {
        let (_, rt) = run(
            "<p></p>",
            r#"
            function counter() {
                let n = 0;
                return function() { n = n + 1; return n; };
            }
            let next = counter();
            next();
            console.log(next());
            "#,
        );
        assert_eq!(logs(&rt), "2");
    }

    #[test]
    fn arrow_functions_and_array_methods() {
        let (_, rt) = run(
            "<p></p>",
            r#"
            let nums = [1, 2, 3, 4];
            let doubled = nums.map(n => n * 2);
            let big = doubled.filter(n => n > 4);
            console.log(big.join("-"));
            "#,
        );
        assert_eq!(logs(&rt), "6-8");
    }

    #[test]
    fn for_of_iterates_arrays() {
        let (_, rt) = run(
            "<p></p>",
            r#"let out = ""; for (const c of ["a", "b"]) { out += c; } console.log(out);"#,
        );
        assert_eq!(logs(&rt), "ab");
    }

    #[test]
    fn objects_store_and_update_properties() {
        let (_, rt) = run(
            "<p></p>",
            r#"let o = { a: 1 }; o.b = 2; o.a = o.a + 10; console.log(o.a + "," + o.b);"#,
        );
        assert_eq!(logs(&rt), "11,2");
    }

    #[test]
    fn strict_and_loose_equality_differ() {
        let (_, rt) = run(
            "<p></p>",
            r#"console.log(1 == "1"); console.log(1 === "1");"#,
        );
        assert_eq!(logs(&rt), "true\nfalse");
    }

    #[test]
    fn logical_operators_short_circuit() {
        let (_, rt) = run(
            "<p></p>",
            r#"
            function boom() { console.log("evaluated"); return true; }
            let x = false && boom();
            let y = true || boom();
            console.log(x + "," + y);
            "#,
        );
        assert_eq!(logs(&rt), "false,true");
    }

    #[test]
    fn ternary_and_typeof() {
        let (_, rt) = run(
            "<p></p>",
            r#"console.log(typeof "s"); console.log(1 > 2 ? "a" : "b");"#,
        );
        assert_eq!(logs(&rt), "string\nb");
    }

    #[test]
    fn string_and_math_builtins() {
        let (_, rt) = run(
            "<p></p>",
            r#"console.log("  Hi ".trim().toUpperCase() + Math.max(2, 9) + Math.floor(3.7));"#,
        );
        assert_eq!(logs(&rt), "HI93");
    }

    #[test]
    fn runaway_recursion_is_contained() {
        let (_, rt) = run(
            "<p></p>",
            "function f() { return f(); } f(); console.log('alive');",
        );
        assert!(logs(&rt).contains("alive"));
    }

    // ── DOM ───────────────────────────────────────────────────────────────

    #[test]
    fn get_element_by_id_and_text_content() {
        let (dom, _) = run(
            r#"<div><h1 id="title">Old</h1></div>"#,
            r#"document.getElementById("title").textContent = "New";"#,
        );
        assert_eq!(text_of(&dom, "h1"), "New");
    }

    #[test]
    fn query_selector_all_updates_every_match() {
        let (dom, _) = run(
            r#"<ul><li class="row">a</li><li class="row">b</li></ul>"#,
            r#"
            let rows = document.querySelectorAll("li.row");
            for (let i = 0; i < rows.length; i++) {
                rows[i].textContent = "row " + (i + 1);
            }
            "#,
        );
        assert_eq!(text_of(&dom, "ul"), "row 1row 2");
    }

    #[test]
    fn style_properties_merge_into_the_style_attribute() {
        let (dom, _) = run(
            r#"<div id="box" style="padding: 4px"></div>"#,
            r#"
            let box = document.getElementById("box");
            box.style.backgroundColor = "red";
            box.style.color = "white";
            "#,
        );
        let path = dom_api::query_selector(&dom, &[], "#box").unwrap();
        let style = dom_api::node_at(&dom, &path)
            .unwrap()
            .as_element()
            .unwrap()
            .get_attr("style")
            .unwrap()
            .to_string();
        assert!(
            style.contains("padding: 4px"),
            "existing declarations kept: {style}"
        );
        assert!(
            style.contains("background-color: red"),
            "camelCase mapped: {style}"
        );
        assert!(
            style.contains("color: white"),
            "second property added: {style}"
        );
    }

    #[test]
    fn class_list_add_remove_toggle() {
        let (dom, rt) = run(
            r#"<div id="d" class="a b"></div>"#,
            r#"
            let d = document.getElementById("d");
            d.classList.remove("a");
            d.classList.add("c");
            console.log(d.classList.toggle("b"));
            console.log(d.className);
            "#,
        );
        assert_eq!(logs(&rt), "false\nc");
        let path = dom_api::query_selector(&dom, &[], "#d").unwrap();
        assert_eq!(
            dom_api::node_at(&dom, &path)
                .unwrap()
                .as_element()
                .unwrap()
                .get_attr("class"),
            Some("c")
        );
    }

    #[test]
    fn create_element_append_child_and_late_mutation() {
        let (dom, _) = run(
            r#"<ul id="list"></ul>"#,
            r#"
            let list = document.getElementById("list");
            for (let i = 1; i <= 3; i++) {
                let li = document.createElement("li");
                li.textContent = "item " + i;
                list.appendChild(li);
                // Handle stays valid after insertion:
                li.setAttribute("data-index", i);
            }
            "#,
        );
        assert_eq!(text_of(&dom, "#list"), "item 1item 2item 3");
        let items = dom_api::query_selector_all(&dom, &[], "li");
        assert_eq!(items.len(), 3);
        let last = dom_api::node_at(&dom, &items[2])
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(last.get_attr("data-index"), Some("3"));
    }

    #[test]
    fn nested_subtree_can_be_built_before_insertion() {
        let (dom, _) = run(
            r#"<div id="host"></div>"#,
            r#"
            let card = document.createElement("div");
            card.className = "card";
            let title = document.createElement("h3");
            title.textContent = "Hello";
            card.appendChild(title);
            document.getElementById("host").appendChild(card);
            title.setAttribute("data-ready", "yes");
            "#,
        );
        let path = dom_api::query_selector(&dom, &[], "#host .card h3").expect("nested insert");
        let node = dom_api::node_at(&dom, &path).unwrap();
        assert_eq!(dom_api::text_content(node), "Hello");
        assert_eq!(
            node.as_element().unwrap().get_attr("data-ready"),
            Some("yes")
        );
    }

    #[test]
    fn remove_detaches_an_element() {
        let (dom, _) = run(
            r#"<div><p id="gone">x</p><p>stay</p></div>"#,
            r#"document.getElementById("gone").remove();"#,
        );
        assert_eq!(dom_api::query_selector_all(&dom, &[], "p").len(), 1);
        assert_eq!(text_of(&dom, "div"), "stay");
    }

    #[test]
    fn inner_html_parses_a_fragment() {
        let (dom, _) = run(
            r#"<div id="d"></div>"#,
            r#"document.getElementById("d").innerHTML = "<span class='x'>hi</span>";"#,
        );
        assert_eq!(text_of(&dom, "#d .x"), "hi");
    }

    #[test]
    fn attributes_round_trip() {
        let (dom, rt) = run(
            r#"<a id="a" href="/one">link</a>"#,
            r#"
            let a = document.getElementById("a");
            console.log(a.getAttribute("href"));
            a.setAttribute("href", "/two");
            console.log(a.hasAttribute("target"));
            "#,
        );
        assert_eq!(logs(&rt), "/one\nfalse");
        let path = dom_api::query_selector(&dom, &[], "a").unwrap();
        assert_eq!(
            dom_api::node_at(&dom, &path)
                .unwrap()
                .as_element()
                .unwrap()
                .get_attr("href"),
            Some("/two")
        );
    }

    #[test]
    fn element_navigation_properties() {
        let (_, rt) = run(
            r#"<div id="p"><span>a</span><span>b</span></div>"#,
            r#"
            let p = document.getElementById("p");
            console.log(p.tagName);
            console.log(p.children.length);
            console.log(p.children[1].textContent);
            console.log(p.children[0].parentElement.id);
            "#,
        );
        assert_eq!(logs(&rt), "DIV\n2\nb\np");
    }

    // ── Events ────────────────────────────────────────────────────────────

    #[test]
    fn click_listener_mutates_dom_and_keeps_state() {
        let mut dom = parse_html(
            r#"<body><button id="btn">Count: 0</button>
               <script>
                 let count = 0;
                 const btn = document.getElementById("btn");
                 btn.addEventListener("click", function () {
                     count++;
                     btn.textContent = "Count: " + count;
                 });
               </script></body>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;

        let btn = dom_api::get_element_by_id(&dom, "btn").unwrap();
        for _ in 0..3 {
            assert!(rt.dispatch_event(&mut dom, &btn, "click").dispatched);
        }
        // State persists across dispatches because the runtime does.
        assert_eq!(text_of(&dom, "#btn"), "Count: 3");
    }

    #[test]
    fn events_bubble_to_ancestors() {
        let mut dom = parse_html(
            r#"<div id="outer"><button id="inner">go</button></div>
               <script>
                 document.getElementById("outer").addEventListener("click", function (e) {
                     console.log("outer saw " + e.target.id);
                 });
                 document.getElementById("inner").addEventListener("click", function () {
                     console.log("inner");
                 });
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        rt.console.clear();

        let inner = dom_api::get_element_by_id(&dom, "inner").unwrap();
        rt.dispatch_event(&mut dom, &inner, "click");
        assert_eq!(logs(&rt), "inner\nouter saw inner");
    }

    #[test]
    fn stop_propagation_halts_bubbling() {
        let mut dom = parse_html(
            r#"<div id="outer"><button id="inner">go</button></div>
               <script>
                 document.getElementById("outer").addEventListener("click", function () {
                     console.log("outer");
                 });
                 document.getElementById("inner").addEventListener("click", function (e) {
                     e.stopPropagation();
                     console.log("inner");
                 });
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        rt.console.clear();

        let inner = dom_api::get_element_by_id(&dom, "inner").unwrap();
        rt.dispatch_event(&mut dom, &inner, "click");
        assert_eq!(logs(&rt), "inner");
    }

    #[test]
    fn listeners_survive_dom_mutation_by_earlier_handlers() {
        let mut dom = parse_html(
            r#"<div id="host"><button id="btn">add</button></div>
               <script>
                 const host = document.getElementById("host");
                 const btn = document.getElementById("btn");
                 btn.addEventListener("click", function () {
                     const p = document.createElement("p");
                     p.textContent = "added";
                     host.appendChild(p);
                 });
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;

        let btn = dom_api::get_element_by_id(&dom, "btn").unwrap();
        rt.dispatch_event(&mut dom, &btn, "click");
        rt.dispatch_event(&mut dom, &btn, "click");
        assert_eq!(dom_api::query_selector_all(&dom, &[], "p").len(), 2);
    }

    // ── Script extraction ─────────────────────────────────────────────────

    #[test]
    fn script_source_survives_angle_brackets() {
        // `<` inside a script must not be tokenized as markup.
        let dom = parse_html(r#"<script>for (let i = 0; i < 3; i++) { x(); }</script>"#);
        let sources = collect_script_sources(&dom);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].contains("i < 3"), "got: {}", sources[0]);
    }

    #[test]
    fn external_scripts_are_skipped() {
        let dom = parse_html(r#"<script src="app.js"></script><script>let a = 1;</script>"#);
        assert_eq!(collect_script_sources(&dom).len(), 1);
    }

    #[test]
    fn execute_dom_scripts_runs_scripts_in_order() {
        let mut dom = parse_html(
            r#"<p id="t">x</p><script>let v = "first";</script><script>document.getElementById("t").textContent = v;</script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#t"), "first");
    }

    #[test]
    fn custom_event_dispatch_triggers_listeners() {
        let mut dom = parse_html(
            r#"<button id="btn">Box</button>
               <script>
                 const btn = document.getElementById("btn");
                 btn.addEventListener("custom", function() {
                     btn.textContent = "Triggered";
                 });
                 btn.dispatchEvent("custom");
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#btn"), "Triggered");
    }

    #[test]
    fn json_stringify_and_parse_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const data = { ok: true, count: 42 };
                 const str = JSON.stringify(data);
                 const parsed = JSON.parse(str);
                 document.getElementById("res").textContent = str;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), r#"{"ok":true,"count":42}"#);
    }

    #[test]
    fn performance_now_timer_test() {
        let mut dom = parse_html(
            r#"<p id="time"></p>
               <script>
                 const t = performance.now();
                 document.getElementById("time").textContent = (t >= 0.0) ? "valid" : "invalid";
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#time"), "valid");
    }

    #[test]
    fn object_keys_and_values_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const obj = { a: 1, b: 2 };
                 const ks = Object.keys(obj).join(",");
                 const vs = Object.values(obj).join(",");
                 document.getElementById("res").textContent = ks + ":" + vs;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), "a,b:1,2");
    }

    #[test]
    fn uri_component_encoding_decoding_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const enc = encodeURIComponent("hello world!");
                 const dec = decodeURIComponent(enc);
                 document.getElementById("res").textContent = enc + " -> " + dec;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), "hello%20world! -> hello world!");
    }

    #[test]
    fn btoa_and_atob_base64_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const encoded = btoa("Hello Rust!");
                 const decoded = atob(encoded);
                 document.getElementById("res").textContent = encoded + " -> " + decoded;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), "SGVsbG8gUnVzdCE= -> Hello Rust!");
    }

    #[test]
    fn location_object_properties_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const info = location.protocol + "|" + location.pathname;
                 document.getElementById("res").textContent = info;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), "demo:|/index.html");
    }

    #[test]
    fn navigator_object_properties_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const ua = navigator.userAgent;
                 const lang = navigator.language;
                 const online = navigator.onLine;
                 document.getElementById("res").textContent = ua + "|" + lang + "|" + online;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(
            text_of(&dom, "#res"),
            "BrowserEngineToy/0.1.0 (Rust)|en-US|true"
        );
    }

    #[test]
    fn screen_object_properties_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 const metrics = screen.width + "x" + screen.height + ":" + screen.colorDepth;
                 document.getElementById("res").textContent = metrics;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), "1920x1080:24");
    }

    #[test]
    fn history_object_properties_and_methods_test() {
        let mut dom = parse_html(
            r#"<p id="res"></p>
               <script>
                 history.back();
                 history.forward();
                 document.getElementById("res").textContent = "len:" + history.length;
               </script>"#,
        );
        let mut rt = execute_dom_scripts(&mut dom);
        rt.quiet = true;
        assert_eq!(text_of(&dom, "#res"), "len:1");
        assert_eq!(rt.pending.len(), 2);
        assert_eq!(rt.pending[0], crate::script::interp::PendingAction::Back);
        assert_eq!(rt.pending[1], crate::script::interp::PendingAction::Forward);
    }
}
