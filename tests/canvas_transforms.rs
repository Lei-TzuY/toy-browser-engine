use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(html: &str, js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body>{}<script>{}</script></body></html>",
        html, js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_canvas_transform_translate_scale_and_restore() {
    let doc = run_js(
        r#"<canvas id="c" width="100" height="100"></canvas>"#,
        r#"
            let canvas = document.getElementById("c");
            let ctx = canvas.getContext("2d");

            ctx.fillStyle = "rgb(255, 0, 0)";
            ctx.save();
            ctx.translate(20, 20);
            ctx.scale(2, 2);
            // Draws 10x10 rect at transformed (20,20) with 2x scale -> covers [20..40, 20..40]
            ctx.fillRect(0, 0, 10, 10);
            ctx.restore();

            // Check pixels: (25, 25) must be red (255, 0, 0, 255), (5, 5) must be transparent (0, 0, 0, 0)
            let imgData1 = ctx.getImageData(25, 25, 1, 1);
            let imgData2 = ctx.getImageData(5, 5, 1, 1);

            console.log("p25_r:" + imgData1.data[0]);
            console.log("p25_a:" + imgData1.data[3]);
            console.log("p5_a:" + imgData2.data[3]);
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "p25_r:255");
    assert_eq!(logs[1], "p25_a:255");
    assert_eq!(logs[2], "p5_a:0");
}

#[test]
fn test_canvas_curves_and_measure_text() {
    let doc = run_js(
        r#"<canvas id="c" width="120" height="120"></canvas>"#,
        r#"
            let canvas = document.getElementById("c");
            let ctx = canvas.getContext("2d");

            // Measure text
            ctx.font = "16px sans-serif";
            let metrics = ctx.measureText("Hello Browser Engine");
            console.log("has_width:" + (metrics.width > 0 ? "yes" : "no"));

            // Draw quadratic and bezier curves
            ctx.strokeStyle = "rgb(0, 255, 0)";
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(10, 10);
            ctx.quadraticCurveTo(50, 0, 90, 10);
            ctx.bezierCurveTo(110, 30, 110, 70, 90, 90);
            ctx.stroke();

            // Check stroke rendered pixels along the path
            let imgData = ctx.getImageData(50, 5, 1, 1);
            console.log("curve_pixel_rendered:" + (imgData.data[1] > 0 || imgData.data[3] > 0 ? "yes" : "no"));
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "has_width:yes");
    assert_eq!(logs[1], "curve_pixel_rendered:yes");
}
