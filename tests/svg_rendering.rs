use browser_engine::html::parse_html;
use browser_engine::svg::{parse_svg_path_data, render_svg, SvgPathSegment};

#[test]
fn test_svg_path_data_parser() {
    let d = "M 10 20 L 30 40 H 50 V 60 C 10 20 30 40 50 60 Q 70 80 90 100 Z";
    let segments = parse_svg_path_data(d);

    assert_eq!(segments[0], SvgPathSegment::MoveTo(10.0, 20.0));
    assert_eq!(segments[1], SvgPathSegment::LineTo(30.0, 40.0));
    assert_eq!(segments[2], SvgPathSegment::LineTo(50.0, 40.0));
    assert_eq!(segments[3], SvgPathSegment::LineTo(50.0, 60.0));
    assert_eq!(
        segments[4],
        SvgPathSegment::CubicCurveTo {
            cp1x: 10.0,
            cp1y: 20.0,
            cp2x: 30.0,
            cp2y: 40.0,
            x: 50.0,
            y: 60.0,
        }
    );
    assert_eq!(
        segments[5],
        SvgPathSegment::QuadraticCurveTo {
            cpx: 70.0,
            cpy: 80.0,
            x: 90.0,
            y: 100.0,
        }
    );
    assert_eq!(segments[6], SvgPathSegment::ClosePath);
}

#[test]
fn test_svg_rasterization() {
    let html = r#"<svg viewBox="0 0 100 100" width="100" height="100">
        <rect x="0" y="0" width="50" height="50" fill="red" />
        <rect x="50" y="50" width="50" height="50" fill="blue" />
        <circle cx="25" cy="75" r="20" fill="green" />
    </svg>"#;

    let root = parse_html(html);
    let svg_node = root
        .children
        .iter()
        .find(|n| n.as_element().map(|e| e.tag_name.as_str()) == Some("svg"))
        .expect("svg element not found");

    let raster = render_svg(svg_node, 100, 100);
    assert_eq!(raster.width, 100);
    assert_eq!(raster.height, 100);

    // Top-left pixel inside red rect (x=10, y=10)
    let red_pixel_idx = ((10 * 100 + 10) * 4) as usize;
    assert_eq!(raster.pixels[red_pixel_idx], 255); // R
    assert_eq!(raster.pixels[red_pixel_idx + 1], 0); // G
    assert_eq!(raster.pixels[red_pixel_idx + 2], 0); // B
    assert_eq!(raster.pixels[red_pixel_idx + 3], 255); // A

    // Bottom-right pixel inside blue rect (x=75, y=75)
    let blue_pixel_idx = ((75 * 100 + 75) * 4) as usize;
    assert_eq!(raster.pixels[blue_pixel_idx], 0); // R
    assert_eq!(raster.pixels[blue_pixel_idx + 1], 0); // G
    assert_eq!(raster.pixels[blue_pixel_idx + 2], 255); // B
    assert_eq!(raster.pixels[blue_pixel_idx + 3], 255); // A

    // Pixel inside green circle (x=25, y=75)
    let green_pixel_idx = ((75 * 100 + 25) * 4) as usize;
    assert_eq!(raster.pixels[green_pixel_idx], 0); // R
    assert_eq!(raster.pixels[green_pixel_idx + 1], 128); // G (CSS 'green' is rgb(0, 128, 0))
    assert_eq!(raster.pixels[green_pixel_idx + 2], 0); // B
    assert_eq!(raster.pixels[green_pixel_idx + 3], 255); // A
}
