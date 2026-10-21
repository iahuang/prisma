//! Renders every example and checks a few pixels. Needs a GPU.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs prisma on `script` with `extra` args, writing to a fresh file. Returns that path and the process output.
fn prisma(script: &str, extra: &[&str]) -> (PathBuf, Output) {
    // Tests run in parallel, so every render gets its own file.
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let stem = Path::new(script).file_stem().unwrap().to_string_lossy();
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{stem}-{n}.png"));
    let output = Command::new(env!("CARGO_BIN_EXE_prisma")).arg(script).arg("-o").arg(&out).arg("--no-cache").args(extra).output().unwrap();
    (out, output)
}

fn render_script(script: &str, extra: &[&str]) -> image::RgbaImage {
    let (out, output) = prisma(script, extra);
    assert!(output.status.success(), "{script} failed to render: {}", String::from_utf8_lossy(&output.stderr));
    image::open(&out).unwrap().into_rgba8()
}

fn render(example: &str, extra: &[&str]) -> image::RgbaImage {
    render_script(&format!("examples/{example}.luau"), extra)
}

#[test]
fn all_examples_render() {
    for ex in ["basics", "poster", "halftone", "a-grid", "dracula", "outline", "sdf", "field", "type", "transform", "logo", "refract", "warp"] {
        render(ex, &[]);
    }
}

#[test]
fn basics_pixels() {
    let img = render("basics", &[]);
    assert_eq!(img.dimensions(), (800, 600));
    // Background.
    assert_eq!(img.get_pixel(5, 5).0, [0xf3, 0xef, 0xe6, 0xff]);
    // Red and blue circles multiply to near-black where they overlap.
    let [r, g, b, _] = img.get_pixel(130, 400).0;
    assert!(r < 60 && g < 40 && b < 60, "multiply overlap is {r},{g},{b}");
    // Center of the blurred green square is fully green.
    let [r, g, b, _] = img.get_pixel(510, 400).0;
    assert!(r < 10 && g > 150 && b > 80, "blur center is {r},{g},{b}");
}

#[test]
fn sdf_pixels() {
    // The left half is the field of a circle of radius 100 at (200, 200), range 50,
    // over black: gray = 0.5 + d / 100, d positive inside.
    for scale in ["1", "2"] {
        let img = render("sdf", &["--scale", scale]);
        let k: u32 = scale.parse().unwrap();
        let gray = |x: u32, y: u32| img.get_pixel(x * k, y * k).0[0] as f32 / 255.0;
        for (x, d) in [(200, 100.0), (275, 25.0), (300, 0.0), (325, -25.0), (360, -60.0)] {
            let want = (0.5 + d / 100.0f32).clamp(0.0, 1.0);
            let got = gray(x, 200);
            assert!((got - want).abs() < 0.02, "scale {scale}: field at d={d} is {got}, want {want}");
        }
        // Diagonal: distance from the circle is exact in every direction, not just along axes.
        let r = 200.0 + 115.0 / 2f32.sqrt();
        let got = gray(r.round() as u32, r.round() as u32);
        assert!((got - 0.35).abs() < 0.03, "scale {scale}: diagonal field is {got}, want 0.35");
    }
}


#[test]
fn type_pixels() {
    for scale in ["1", "2"] {
        let img = render("type", &["--scale", scale]);
        let k: u32 = scale.parse().unwrap();
        let px = |x: u32, y: u32| img.get_pixel(x * k, y * k).0;
        let ink = |x, y| px(x, y)[0] < 60;
        // Text `y` is the baseline: the H stem stands on y = 200 and rises to cap height.
        assert!(ink(62, 110) && ink(62, 198), "scale {scale}: H stem missing above the baseline");
        assert!(!ink(62, 95) && !ink(62, 206), "scale {scale}: H stem extends past cap height or baseline");
        // The paper effect's `area` covers the canvas, corners included.
        for (x, y) in [(2, 2), (797, 397)] {
            let [r, g, b, a] = px(x, y);
            assert!(a == 255 && r > 220 && g > 210 && b > 195 && b < 235, "scale {scale}: corner {x},{y} is {r},{g},{b},{a}, want paper");
        }
        // The text gradient is in canvas coordinates: orange at cap height (y = 280), blue at the baseline (y = 360).
        let tint = |y: u32| {
            let (mut n, mut r, mut b) = (0u32, 0u32, 0u32);
            for x in 0..800 {
                let p = px(x, y);
                if p.iter().take(3).max().unwrap() - p.iter().take(3).min().unwrap() > 80 {
                    (n, r, b) = (n + 1, r + p[0] as u32, b + p[2] as u32);
                }
            }
            assert!(n > 100, "scale {scale}: no gradient ink on row {y}");
            (r / n, b / n)
        };
        let ((r_top, b_top), (r_bot, b_bot)) = (tint(287), tint(354));
        assert!(r_top > 200 && b_top < 90, "scale {scale}: cap height is r {r_top} b {b_top}, want orange");
        assert!(b_bot > 200 && r_bot < 90, "scale {scale}: baseline is r {r_bot} b {b_bot}, want blue");
    }
}

#[test]
fn transform_pixels() {
    for scale in ["1", "2"] {
        let img = render("transform", &["--scale", scale]);
        let k: u32 = scale.parse().unwrap();
        // transform = { 1, 0, 0.5, 1, 600, 300 } on a 40x40 square: row y' covers
        // x' in [600 + 0.5 (y' - 300), 640 + 0.5 (y' - 300)].
        let red = |x: u32, y: u32| {
            let [r, g, b, _] = img.get_pixel(x * k, y * k).0;
            r > 240 && g < 15 && b < 15
        };
        for (x, y, want) in [(630, 320, true), (605, 320, false), (655, 320, false), (650, 335, true), (612, 335, false), (603, 305, true)] {
            assert_eq!(red(x, y), want, "scale {scale}: ({x}, {y}) red should be {want}");
        }
    }
}

#[test]
fn measure_matches_layout() {
    // The fixture asserts measure's numbers against anchor/align placement and
    // draws a marker at the advance end of "HH".
    let img = render_script("tests/fixtures/measure.luau", &[]);
    // Helvetica H: advance 0.722 em, so "HH" at 80 px ends at 40 + 115.55; its ink spans 46.3..149.6.
    let px = |x: u32| img.get_pixel(x, 70).0;
    assert_eq!(px(44), [255, 255, 255, 255], "left side bearing");
    assert_eq!(px(50)[..3], [0, 0, 0], "left stem");
    assert_eq!(px(146)[..3], [0, 0, 0], "right stem");
    assert_eq!(px(153), [255, 255, 255, 255], "right side bearing");
    assert_eq!(px(157), [0, 0, 255, 255], "the marker starts where the advance ends");
}

#[test]
fn path_helpers_and_doc_coordinates() {
    // The fixture asserts transformPath/pathBounds numbers; at --scale 2 its pixels
    // check a path fitted to a box and an effect that reads src at a doc-px offset.
    let img = render_script("tests/fixtures/path_helpers.luau", &["--scale", "2"]);
    let px = |x: u32, y: u32| img.get_pixel(x, y).0;
    assert_eq!(px(2 * 21, 2 * 21)[..3], [0, 0, 0], "fitted path fills its box's corner");
    assert_eq!(px(2 * 79, 2 * 59)[..3], [0, 0, 0], "fitted path fills its box's far corner");
    assert_eq!(px(2 * 82, 2 * 40), [255, 255, 255, 255], "nothing past the box");
    assert_eq!(px(2 * 140, 2 * 125), [255, 0, 0, 255], "src_doc(to_doc(p) + 20) moves the square 20 doc px left");
    assert_eq!(px(2 * 160, 2 * 125), [255, 255, 255, 255], "the square's own place is empty");
}

#[test]
fn named_effects_can_change_between_runs() {
    // A sweep reuses one engine, like watch mode; each cell redefines "solid" with different code.
    let img = render_script("tests/fixtures/sweep_effect.luau", &["--sweep", "v=0.2,0.8"]);
    let (a, b) = (img.get_pixel(196, 196).0[0], img.get_pixel(572, 196).0[0]);
    assert!(a.abs_diff(51) <= 2 && b.abs_diff(204) <= 2, "sweep cells are {a} and {b}, want 51 and 204");
}

#[test]
fn paths_resolve_from_the_script_within_root() {
    let img = render_script("tests/fixtures/paths/sub/main.luau", &["--root", "tests/fixtures/paths"]);
    assert_eq!(img.get_pixel(5, 5).0, [0, 255, 0, 255]);
    // A helper in another folder: its require and image paths resolve from the helper's file.
    let img = render_script("tests/fixtures/paths/sub/shared.luau", &["--root", "tests/fixtures/paths"]);
    assert_eq!(img.get_pixel(5, 2).0, [255, 0, 0, 255], "the helper's image");
    assert_eq!(img.get_pixel(5, 8).0, [0, 0, 255, 255], "the helper's own require");
    let (_, out) = prisma("tests/fixtures/paths/sub/escape.luau", &["--root", "tests/fixtures/paths"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && err.contains("outside the project directory"), "escape wasn't refused: {err}");
}

#[test]
fn logo_pixels() {
    let img = render("logo", &[]);
    let (ink, bg) = ([0x1d, 0x35, 0x57, 0xff], [0xf4, 0xf1, 0xea, 0xff]);
    // Even-odd, around the mark's center (190, 180): the ring is filled, the hole
    // inside it is not (nonzero would fill it: both circles wind the same way),
    // the bar is cut where it crosses the ring and filled inside and outside it.
    for (x, y, want, what) in [
        (190, 80, ink, "ring"),
        (230, 140, bg, "hole"),
        (190, 180, ink, "bar inside the hole"),
        (290, 180, bg, "bar across the ring"),
        (325, 180, ink, "bar outside the ring"),
    ] {
        assert_eq!(img.get_pixel(x, y).0, want, "{what} at {x},{y}");
    }
    // The construction is the same path unfilled.
    assert_eq!(img.get_pixel(610, 140).0, bg);
    assert_eq!(img.get_pixel(570 + 135, 180 - 10).0, bg, "inside the bar's outline");
}

fn overflow(case: &str, extra: &[&str]) -> (image::RgbaImage, String) {
    let mut args = vec!["-p".to_string(), format!("case={case}")];
    args.extend(extra.iter().map(|s| s.to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (out, o) = prisma("tests/fixtures/overflow.luau", &args);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(o.status.success(), "{case}: {err}");
    (image::open(out).unwrap().into_rgba8(), err)
}

const RED: [u8; 4] = [0xe8, 0x40, 0x2a, 0xff];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn nested_masks_fit_a_large_canvas() {
    // Four masks deep over a 2400px canvas blanked the whole render when each
    // mask's layers covered the canvas instead of the mask.
    let (img, err) = overflow("masks", &[]);
    assert!(!err.contains("warning"), "{err}");
    assert_eq!(img.get_pixel(1200, 1200).0, RED);
    assert_eq!(img.get_pixel(5, 5).0, WHITE);
    // The inverted mask keeps its content outside the shape and cuts the hole.
    assert_eq!(img.get_pixel(210, 300).0, RED);
    assert_eq!(img.get_pixel(300, 300).0, WHITE);
}

#[test]
fn overflowing_render_is_split() {
    let (img, err) = overflow("crowd", &[]);
    assert!(err.contains("warning: the scene overflowed vello's GPU buffers") && err.contains("rendered in pieces"), "{err}");
    assert_eq!(img.get_pixel(1200, 1200).0, RED);
}

#[test]
fn overflow_that_splitting_cant_fix_fails() {
    let (_, o) = prisma("tests/fixtures/overflow.luau", &["-p", "case=lines"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("line segments") && err.contains("can't help"), "{err}");
}

#[test]
fn images_bigger_than_the_atlas_draw() {
    // A 600x12000 page, then read back whole (scaled to a tenth) and as a 1:1 crop.
    let (page, o) = prisma("tests/fixtures/big.luau", &["-p", "case=page"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let src = format!("src={}", page.display());
    let img = render_script("tests/fixtures/big.luau", &["-p", "case=view", "-p", &src, "--root", "/"]);
    for (y, want) in [(50, [255, 0, 0, 255]), (150, [0, 255, 0, 255]), (250, [0, 0, 255, 255]), (1010, [0, 0, 0, 255])] {
        assert_eq!(img.get_pixel(30, y).0, want, "whole page at y = {y}");
    }
    // The crop starts at source (100, 9900): the square's left edge lands at x = 250.
    assert_eq!(img.get_pixel(110, 50).0, [255, 0, 0, 255]);
    assert_eq!(img.get_pixel(110, 300).0, [0, 255, 0, 255]);
    assert_eq!(img.get_pixel(249, 200).0, [0, 255, 0, 255]);
    assert_eq!(img.get_pixel(250, 200).0, [0, 0, 0, 255]);
}

#[test]
fn passes_bigger_than_the_atlas_draw() {
    // A blur's texture is 10000px tall; vello drops an image that size, so it goes back in tiles.
    let img = render_script("tests/fixtures/big.luau", &["-p", "case=tallblur"]);
    for y in [500, 8000, 8300, 9800] {
        assert_eq!(img.get_pixel(200, y).0, [0, 0, 255, 255], "y = {y}");
    }
}

#[test]
fn reads_stop_at_the_canvas_margin() {
    let blue = |p: [u8; 4]| p[2] as i32 - p[0] as i32;
    // By default the margin is the canvas's larger side: the band's off-canvas
    // part still blurs in, ~82% blue 18px inside its edge (σ = 20).
    let img = render_script("tests/fixtures/margin.luau", &[]);
    assert!(blue(img.get_pixel(2, 100).0) > 130, "default margin: edge is {:?}", img.get_pixel(2, 100).0);
    // With no margin, nothing left of the canvas exists to blur in: only the
    // band's last 20px do, ~36% blue.
    let img = render_script("tests/fixtures/margin.luau", &["-p", "margin=0"]);
    assert!(blue(img.get_pixel(2, 100).0) < 100, "margin 0: edge is {:?}", img.get_pixel(2, 100).0);
    let (_, o) = prisma("tests/fixtures/margin.luau", &["-p", "margin=-5"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("margin: can't be negative"), "{err}");
}

#[test]
fn tile_must_be_a_multiple_of_16() {
    let (_, o) = prisma("examples/basics.luau", &["--tile", "100"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("--tile must be a multiple of 16"), "{err}");
}

#[test]
fn sdf_input_is_exact() {
    // Every tested pixel encodes its own errors against a true rotated square (see the fixture).
    for scale in ["1", "2"] {
        let img = render_script("tests/fixtures/field_probe.luau", &["--scale", scale]);
        let tested: Vec<[f32; 3]> = img.pixels().filter(|p| p.0[3] == 255).map(|p| [0, 1, 2].map(|c| p.0[c] as f32 / 255.0)).collect();
        assert!(tested.len() > 50_000, "scale {scale}: only {} pixels tested", tested.len());
        let worst = |c: usize| tested.iter().map(|e| e[c]).fold(0.0, f32::max);
        let (d, n, m) = (worst(0) * 0.1, worst(1) * 8.0, worst(2) * 0.5);
        let within_1deg = tested.iter().filter(|e| e[1] * 8.0 <= 1.0).count() as f32 / tested.len() as f32;
        println!("scale {scale}: {} px; max d error {d:.3}px, march {m:.3}px, normal {n:.2}° ({:.1}% within 1°)", tested.len(), within_1deg * 100.0);
        assert!(d <= 0.05, "scale {scale}: distance off by up to {d:.3}px");
        assert!(m <= 0.1, "scale {scale}: march off by up to {m:.3}px");
        assert!(n <= 7.0 && within_1deg >= 0.95, "scale {scale}: normal off by up to {n:.2}°, {:.1}% within 1°", within_1deg * 100.0);
    }
}

#[test]
fn sdf_inputs_are_explicit() {
    let run = |case: &str| prisma("tests/fixtures/field_errors.luau", &["-p", &format!("case={case}")]).1;
    assert!(run("ok").status.success(), "{}", String::from_utf8_lossy(&run("ok").stderr));
    for (case, want) in [
        ("missing", "missing input `body`"),
        ("bare", "input `body` expects an Sdf, got Circle (wrap it: body = Sdf"),
        ("list", "expects an Sdf, got a list"),
        ("fill", "`fill` has no effect on an effect input"),
        ("blur", "`blur` on an effect input is not supported yet"),
    ] {
        let out = run(case);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && err.contains(want), "{case}: {err}");
        assert!(err.contains("field_errors.luau:"), "{case}: no script location in {err}");
    }
}

fn explicit(case: &str, extra: &[&str]) -> (PathBuf, Output) {
    let mut args = vec!["-p".to_string(), format!("case={case}")];
    args.extend(extra.iter().map(|s| s.to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    prisma("tests/fixtures/explicit.luau", &args)
}

#[test]
fn explicit_rules_render() {
    let ok = |case: &str, extra: &[&str]| {
        let (out, o) = explicit(case, extra);
        assert!(o.status.success(), "{case}: {}", String::from_utf8_lossy(&o.stderr));
        image::open(out).unwrap().into_rgba8()
    };
    // param("n", 3) with -p n=007 is the number 7; params.n stays "007".
    let img = ok("params", &["-p", "n=007"]);
    assert_eq!(img.get_pixel(65, 5).0[0], 0, "param n should read as 7 (70px wide)");
    assert_eq!(img.get_pixel(75, 5).0[0], 255, "param n should read as 7 (70px wide)");
    // A stroked shape is still filled black by default.
    assert_eq!(ok("fill", &[]).get_pixel(50, 50).0, [0, 0, 0, 255]);
    // opacity = 0.5 on fill + stroke: where the stroke covers the fill, the result is
    // half blue over white (the fill doesn't show through); the bare fill is half red.
    let img = ok("opacity", &[]);
    let near = |p: [u8; 4], want: [u8; 3]| p.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3);
    assert!(near(img.get_pixel(100, 76).0, [127, 127, 255]), "stroke over fill is {:?}", img.get_pixel(100, 76).0);
    assert!(near(img.get_pixel(100, 50).0, [255, 127, 127]), "fill is {:?}", img.get_pixel(100, 50).0);
    // anchor = "middle" centers unboxed text on x.
    let img = ok("anchor", &[]);
    let ink: Vec<u32> = img.enumerate_pixels().filter(|(_, _, p)| p.0[0] < 128).map(|(x, _, _)| x).collect();
    let (lo, hi) = (*ink.iter().min().unwrap(), *ink.iter().max().unwrap());
    assert!(((lo + hi) as f32 / 2.0 - 100.0).abs() <= 2.0, "ink spans {lo}..{hi}, want it centered on 100");
}

#[test]
fn explicit_rules_reject() {
    for (case, extra, want) in [
        ("param_number", &["-p", "n=nan"][..], "-p n=nan is not a number"),
        ("param_number", &["-p", "n=1O"][..], "-p n=1O is not a number"),
        ("param_required", &[][..], "param \"missing\": missing (pass -p missing=...)"),
        ("opacity_range", &[][..], "`opacity` must be between 0 and 1, got 1.5"),
        ("blur_negative", &[][..], "`blur` can't be negative"),
        ("line_fill", &[][..], "a Line has no area to fill"),
        ("line_fill_rule", &[][..], "a Line has no area to fill, so no `fillRule`"),
        ("fill_rule", &[][..], "\"winding\" is not one of nonzero, evenodd"),
        ("align_unboxed", &[][..], "`align` places lines inside a box: set `w`, or use `anchor`"),
        ("anchor_boxed", &[][..], "with `w`, use `align`"),
        ("font_face", &[][..], "the family is \"Courier New\" (use font = \"Courier New\", weight = 700)"),
        ("font_path", &[][..], "for a font file use fontFile"),
        ("font_both", &[][..], "give either `font` (a family) or `fontFile`"),
        ("pad", &[][..], "`pad = x` was `reach = x, grow = x`"),
        ("reach_negative", &[][..], "`r` is this effect's reach and can't be negative"),
        ("transform_and_x", &[][..], "give either `transform` or `x`, not both"),
        ("transform_short", &[][..], "expected 6 values { a, b, c, d, e, f }"),
        ("measure_not_text", &[][..], "measure expects a Text { ... } element"),
        ("transform_path_mixed", &[][..], "transformPath: give a matrix { a, b, c, d, e, f } or named keys"),
        ("transform_path_key", &[][..], "transformPath: unknown prop `rotat` (did you mean `rotate`?)"),
        ("transform_path_bad_d", &[][..], "transformPath: bad path data"),
    ] {
        let (_, out) = explicit(case, extra);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && err.contains(want), "{case}: {err}");
        assert!(err.contains("explicit.luau:"), "{case}: no script location in {err}");
    }
}

#[test]
fn height_input_is_exact() {
    // Every tested pixel encodes its own errors against the exact ramp (see the fixture).
    for scale in ["1", "2"] {
        let img = render_script("tests/fixtures/height_probe.luau", &["--scale", scale, "-p", "blur=4"]);
        let tested: Vec<[f32; 3]> = img.pixels().filter(|p| p.0[3] == 255).map(|p| [0, 1, 2].map(|c| p.0[c] as f32 / 255.0)).collect();
        assert!(tested.len() > 30_000, "scale {scale}: only {} pixels tested", tested.len());
        let worst = |c: usize| tested.iter().map(|e| e[c]).fold(0.0, f32::max);
        let (h, gx, gy) = (worst(0), worst(1) * 5.0, worst(2) * 5.0);
        println!("scale {scale}: {} px; max h error {h:.2}% of scale, slope {gx:.2}% along, {gy:.2}% across", tested.len());
        assert!(h <= 0.6, "scale {scale}: height off by up to {h:.2}% of scale");
        assert!(gx <= 2.5, "scale {scale}: slope off by up to {gx:.2}%");
        // Also 1 (= 5%) if the absent optional input read as anything but empty.
        assert!(gy <= 2.5, "scale {scale}: slope across the ramp up to {gy:.2}% (or the absent input wasn't empty)");
    }
}

#[test]
fn refraction_follows_snell() {
    // A 45-degree chamfer at ior 1.5 bends the view by 45 - asin(sin 45 / 1.5) =
    // 16.87 degrees, so it reads tan(16.87) = 0.3033 x the height away, inward.
    for scale in ["1", "2"] {
        let k: u32 = scale.parse().unwrap();
        let img = render_script("tests/fixtures/refract_probe.luau", &["--scale", scale]);
        let red = |x: u32| img.get_pixel(x * k, 50 * k).0[0] as f32;
        let at = |x: u32| red(x) - red(20) + 20.0; // where the ramp was read, in doc px
        for (x, want) in [(70, 70.0 + 0.3033 * 20.0), (80, 80.0 + 0.3033 * 30.0), (150, 150.0), (230, 230.0 - 0.3033 * 20.0)] {
            let got = at(x);
            assert!((got - want).abs() <= 1.5, "scale {scale}: x = {x} read the backdrop at {got}, want {want:.1}");
        }
        // Gloss with the light overhead and shininess 2: (n . up)^2 = cos(45)^2 = 0.5 on the chamfer.
        let img = render_script("tests/fixtures/refract_probe.luau", &["--scale", scale, "-p", "case=gloss"]);
        let green = |x: u32| img.get_pixel(x * k, 50 * k).0[1] as i32;
        assert_eq!(green(30), 0, "scale {scale}: gloss outside the body");
        assert!((green(70) - 128).abs() <= 2, "scale {scale}: gloss on the chamfer is {}, want cos(45)^2 = 0.5", green(70));
        assert!(green(150) >= 254, "scale {scale}: gloss on the flat top is {}", green(150));
    }
}

#[test]
fn height_inputs_are_explicit() {
    let run = |case: &str| prisma("tests/fixtures/height_errors.luau", &["-p", &format!("case={case}")]).1;
    // `extra` is optional and left out throughout.
    assert!(run("ok").status.success(), "{}", String::from_utf8_lossy(&run("ok").stderr));
    for (case, want) in [
        ("missing", "missing input `surface` (a Height { ... })"),
        ("bare", "input `surface` expects a Height, got Rect (wrap it: surface = Height { scale = ..."),
        ("sdf", "expects a Height, got Sdf"),
        ("drawn", "Height is data for an effect's height input, not something drawn"),
        ("noscale", "Height: missing `scale` (how many doc px high white is"),
        ("zero", "`scale` can't be 0"),
        ("fill", "`fill` has no effect on an effect input"),
        ("typo", "unknown prop `blurr` (did you mean `blur`?)"),
    ] {
        let out = run(case);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && err.contains(want), "{case}: {err}");
        assert!(err.contains("height_errors.luau:"), "{case}: no script location in {err}");
    }
}

fn warp_case(case: &str) -> (PathBuf, Output) {
    prisma("tests/fixtures/warp.luau", &["-p", &format!("case={case}")])
}

#[test]
fn affine_warps_match_transforms() {
    // A Coons patch on a parallelogram (from its outline or four edges) and a
    // function map are all the same affine, so they must draw what a Group
    // transform does, up to antialiasing at edges.
    let want = render_script("tests/fixtures/warp.luau", &["-p", "case=grouped"]);
    for case in ["warped", "edges", "fn"] {
        let got = render_script("tests/fixtures/warp.luau", &["-p", &format!("case={case}")]);
        let (mut worst, mut off) = (0u8, 0usize);
        for (a, b) in got.pixels().zip(want.pixels()) {
            let d = a.0.iter().zip(b.0.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
            worst = worst.max(d);
            off += (d > 8) as usize;
        }
        assert!(worst <= 96, "{case}: a pixel differs by {worst}");
        assert!(off * 1000 < (want.width() * want.height()) as usize, "{case}: {off} pixels differ by more than 8");
    }
}

#[test]
fn warp_rules_reject() {
    for (case, want) in [
        ("outside", "is outside `from` { 0, 0, 50, 50 }: grow `from` to cover the content, or set outside = \"extend\""),
        ("no_from", "Warp: missing `from = { x, y, w, h }`"),
        ("fn_from", "a map function maps doc px directly, so it takes no `from`"),
        ("image", "Image in a Warp: warping pixels isn't supported"),
        ("gradient", "gradient paint can't follow a warp"),
        ("clip", "a `clip` rect doesn't stay a rect when warped"),
        ("corners", "the corners must lie around the outline in the order top-left, top-right, bottom-right, bottom-left"),
        ("edges_apart", "the bottom-left (bottom's start, left's end) corner doesn't meet"),
    ] {
        let (_, out) = warp_case(case);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && err.contains(want), "{case}: {err}");
        assert!(err.contains("warp.luau:"), "{case}: no script location in {err}");
    }
}
