use browser_engine::document::Document;
use browser_engine::html::parse_html;
use browser_engine::net::{MemoryLoader, Url};
use browser_engine::paint::DisplayCommand;
use browser_engine::script::execute_dom_scripts;

#[test]
fn test_canvas_get_context_and_properties() {
    let mut dom = parse_html(r#"<canvas id="c" width="400" height="200"></canvas>"#);
    let runtime = execute_dom_scripts(&mut dom);
    assert_eq!(runtime.console, Vec::<String>::new());

    let mut dom2 = parse_html(
        r##"
        <canvas id="myCanvas" width="500" height="300"></canvas>
        <script>
            let canvas = document.getElementById("myCanvas");
            console.log(canvas.width, canvas.height);
            let ctx = canvas.getContext("2d");
            console.log(ctx.lineWidth);
            ctx.lineWidth = 5;
            console.log(ctx.lineWidth);
            ctx.fillStyle = "#ff0000";
            ctx.fillRect(0, 0, 100, 50);
            let imgData = ctx.getImageData(10, 10, 1, 1);
            console.log(imgData.data[0], imgData.data[1], imgData.data[2], imgData.data[3]);
        </script>
        "##,
    );
    let runtime2 = execute_dom_scripts(&mut dom2);
    assert_eq!(
        runtime2.console,
        vec![
            "500 300",
            "1",
            "5",
            "255 0 0 255",
        ]
    );
}

#[test]
fn test_canvas_draw_paths_and_circles() {
    let mut dom = parse_html(
        r#"
        <canvas id="c" width="100" height="100"></canvas>
        <script>
            let ctx = document.getElementById("c").getContext("2d");
            ctx.fillStyle = "blue";
            ctx.beginPath();
            ctx.arc(50, 50, 20, 0, 6.28318, false);
            ctx.fill();

            let centerPixel = ctx.getImageData(50, 50, 1, 1);
            let outsidePixel = ctx.getImageData(5, 5, 1, 1);
            console.log("center:", centerPixel.data[2], centerPixel.data[3]);
            console.log("outside:", outsidePixel.data[3]);
        </script>
        "#,
    );
    let runtime = execute_dom_scripts(&mut dom);
    assert_eq!(
        runtime.console,
        vec![
            "center: 255 255",
            "outside: 0",
        ]
    );
}

#[test]
fn test_canvas_save_and_restore() {
    let mut dom = parse_html(
        r#"
        <canvas id="c" width="100" height="100"></canvas>
        <script>
            let ctx = document.getElementById("c").getContext("2d");
            ctx.fillStyle = "red";
            ctx.lineWidth = 10;
            ctx.save();

            ctx.fillStyle = "green";
            ctx.lineWidth = 2;
            console.log(ctx.lineWidth);

            ctx.restore();
            console.log(ctx.lineWidth);
        </script>
        "#,
    );
    let runtime = execute_dom_scripts(&mut dom);
    assert_eq!(runtime.console, vec!["2", "10"]);
}

#[test]
fn test_canvas_text_and_measure() {
    let mut dom = parse_html(
        r#"
        <canvas id="c" width="200" height="100"></canvas>
        <script>
            let ctx = document.getElementById("c").getContext("2d");
            ctx.font = "20px sans-serif";
            let m = ctx.measureText("Hello");
            console.log(m.width > 0);
            ctx.fillText("Hello", 10, 30);
        </script>
        "#,
    );
    let runtime = execute_dom_scripts(&mut dom);
    assert_eq!(runtime.console, vec!["true"]);
}

#[test]
fn test_canvas_put_image_data() {
    let mut dom = parse_html(
        r#"
        <canvas id="c" width="10" height="10"></canvas>
        <script>
            let ctx = document.getElementById("c").getContext("2d");
            let img = {
                width: 2,
                height: 2,
                data: [
                    255, 128, 0, 255,
                    0, 255, 0, 255,
                    0, 0, 255, 255,
                    255, 255, 255, 255
                ]
            };
            ctx.putImageData(img, 1, 1);
            let p = ctx.getImageData(1, 1, 1, 1);
            console.log(p.data[0], p.data[1], p.data[2], p.data[3]);
        </script>
        "#,
    );
    let runtime = execute_dom_scripts(&mut dom);
    assert_eq!(runtime.console, vec!["255 128 0 255"]);
}

#[test]
fn test_canvas_renders_to_display_list_pipeline() {
    let html = r#"
    <html>
        <head><style>body { margin: 0; padding: 0; }</style></head>
        <body>
            <canvas id="c" width="200" height="100"></canvas>
            <script>
                let ctx = document.getElementById("c").getContext("2d");
                ctx.fillStyle = "rgb(255, 0, 0)";
                ctx.fillRect(0, 0, 200, 100);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);
    let styled = doc.style_tree(800.0, &browser_engine::PointerState::default());
    let layout = doc.layout(&styled, 800.0);
    let commands = browser_engine::paint::build_display_list(&layout);

    let has_canvas_image = commands.iter().any(|cmd| matches!(cmd, DisplayCommand::Image { .. }));
    assert!(has_canvas_image, "Display list must contain the rendered canvas Image command");

    // Also test full raster render output
    let canvas = doc.render(800, 600, 0.0, &browser_engine::PointerState::default());
    let idx = (10 * canvas.width + 10) * 3;
    assert_eq!(&canvas.pixels[idx..idx + 3], &[255, 0, 0]);
}
