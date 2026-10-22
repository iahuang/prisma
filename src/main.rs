mod anim;
mod effects;
mod engine;
mod gpu;
mod ir;
mod render;
mod script;
mod text;
mod units;
mod warp;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use clap::Parser;

use crate::anim::{Player, Sink, frames_of, onion};
use crate::engine::{Engine, write_png};
use crate::gpu::IRect;
use crate::ir::{Align, Anchor, Document, Fit, Font, ImageSrc, Kind, Node, Paint, Text};
use crate::script::{Loaded, RunOptions, load, run};
use crate::units::parse_length;

/// Graphic design as Luau. A script returns a `Canvas { ... }`; prisma
/// renders it on the GPU (vello + wgpu) to a PNG.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The .luau script to render.
    file: PathBuf,
    /// Output .png (default: <file>.png; with --emit-ir, .json).
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Random seed for math.random/random/pick.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Device px per doc px.
    #[arg(long, conflicts_with = "dpi")]
    scale: Option<f64>,
    /// Output resolution (same as --scale dpi/96).
    #[arg(long)]
    dpi: Option<f64>,
    /// Render only this region, x,y,w,h in doc units (e.g. "400,120,240,300" or "10mm,10mm,50mm,30mm").
    /// Pixels match the same region of a full render.
    #[arg(long)]
    crop: Option<String>,
    /// A value for `params.k` in the script (repeatable).
    #[arg(short = 'p', long = "param", value_name = "K=V")]
    params: Vec<String>,
    /// Render a contact sheet over values: k=a,b,c or k=1..9 (repeatable; "seed" sweeps seeds).
    #[arg(long, value_name = "K=VALUES")]
    sweep: Vec<String>,
    /// Contact sheet columns.
    #[arg(long)]
    cols: Option<usize>,
    /// Render one frame of an Animation (0-based).
    #[arg(long, conflicts_with_all = ["time", "frames"])]
    frame: Option<u32>,
    /// Render the Animation frame at this time: seconds, or e.g. "1.5s", "500ms", "45f".
    #[arg(long, conflicts_with = "frames")]
    time: Option<String>,
    /// Render these frames of an Animation, a..b with both ends included (default: all).
    #[arg(long, value_name = "A..B")]
    frames: Option<String>,
    /// Overlay these Animation frames into one PNG (a..b/s or a,b,c): the last as is, earlier ones as fading ghosts.
    #[arg(long, value_name = "FRAMES", conflicts_with_all = ["frame", "time", "frames", "emit_ir"])]
    onion: Option<String>,
    /// ffmpeg output arguments replacing the defaults for -o .mp4/.webm/.mov/.gif.
    #[arg(long, value_name = "ARGS", allow_hyphen_values = true)]
    encode: Option<String>,
    /// Write the evaluated scene IR as JSON instead of rendering.
    #[arg(long)]
    emit_ir: bool,
    /// Write every offscreen pass's input and output, and every rasterize({ name = ... }), to this directory.
    #[arg(long)]
    dump: Option<PathBuf>,
    /// Don't read or write the raster cache (.prisma-cache/).
    #[arg(long)]
    no_cache: bool,
    /// Re-render whenever the script or anything it reads changes.
    #[arg(short, long)]
    watch: bool,
    /// Print timings.
    #[arg(long)]
    profile: bool,
    /// Directory scripts may read from (default: the script's directory). Relative paths still resolve from the script.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Script wall-clock limit in seconds (including rasterize calls).
    #[arg(long, default_value_t = 120)]
    timeout: u64,
    /// The most device px on a side one GPU submission draws; bigger renders and passes go in tiles.
    #[arg(long, default_value_t = gpu::DEFAULT_TILE)]
    tile: u32,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = real_main(&cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main(cli: &Cli) -> Result<()> {
    let entry = cli.file.canonicalize().with_context(|| format!("{} not found", cli.file.display()))?;
    let dir = entry.parent().unwrap().to_path_buf();
    let root = match &cli.root {
        Some(r) => r.canonicalize()?,
        None => dir.clone(),
    };
    if !entry.starts_with(&root) {
        bail!("{} is outside --root {}", entry.display(), root.display());
    }
    let out = cli.out.clone().unwrap_or_else(|| entry.with_extension(if cli.emit_ir { "json" } else { "png" }));

    if cli.tile < 16 || !cli.tile.is_multiple_of(16) {
        bail!("--tile must be a multiple of 16 (vello's tile size), got {}", cli.tile);
    }
    let t0 = Instant::now();
    let mut eng = Engine::new(root)?;
    eng.gpu.tile = cli.tile;
    eng.script_dir = dir.clone();
    eng.cache_dir = (!cli.no_cache).then(|| dir.join(".prisma-cache"));
    eng.dump_dir = cli.dump.clone();
    eng.profile = cli.profile;
    if cli.profile {
        eprintln!("gpu init: {:.0}ms", t0.elapsed().as_secs_f64() * 1e3);
    }
    let eng = Rc::new(RefCell::new(eng));

    if !cli.sweep.is_empty() {
        return sweep(cli, &eng, &entry, &out);
    }
    let mut deps = render_once(cli, &eng, &entry, &out, cli.seed, &parse_params(&cli.params)?);
    if !cli.watch {
        return deps.map(|_| ());
    }
    if let Err(e) = &deps {
        eprintln!("error: {e:#}");
    }
    eprintln!("watching for changes (ctrl-c to stop)");
    let mut stamps = stamp_all(deps.as_deref().unwrap_or(&[entry.clone()]));
    loop {
        std::thread::sleep(Duration::from_millis(150));
        let watched: Vec<PathBuf> = stamps.iter().map(|(p, _)| p.clone()).collect();
        let now = stamp_all(&watched);
        if now == stamps {
            continue;
        }
        deps = render_once(cli, &eng, &entry, &out, cli.seed, &parse_params(&cli.params)?);
        if let Err(e) = &deps {
            eprintln!("error: {e:#}");
        }
        // Keep watching the old set if the script failed before reading everything.
        let next = deps.as_deref().map(|d| d.to_vec()).unwrap_or(watched);
        stamps = stamp_all(&next);
    }
}

fn stamp_all(paths: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>)> {
    paths.iter().map(|p| (p.clone(), std::fs::metadata(p).and_then(|m| m.modified()).ok())).collect()
}

fn parse_params(raw: &[String]) -> Result<Vec<(String, String)>> {
    raw.iter()
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => Ok((k.to_string(), v.to_string())),
            None => bail!("--param wants k=v, got {kv:?}"),
        })
        .collect()
}

fn scale(cli: &Cli) -> f64 {
    cli.dpi.map(|d| d / 96.0).or(cli.scale).unwrap_or(1.0)
}

fn crop_region(cli: &Cli) -> Result<Option<IRect>> {
    let Some(spec) = &cli.crop else { return Ok(None) };
    let v: Vec<f64> = spec.split(',').map(parse_length).collect::<Result<_>>()?;
    let [x, y, w, h] = v[..] else { bail!("--crop wants x,y,w,h, got {spec:?}") };
    let s = scale(cli);
    Ok(Some(IRect {
        x: (x * s).round() as i32,
        y: (y * s).round() as i32,
        w: (w * s).round().max(1.0) as u32,
        h: (h * s).round().max(1.0) as u32,
    }))
}

fn options(cli: &Cli, seed: u64, params: &[(String, String)]) -> RunOptions {
    RunOptions {
        seed,
        params: params.to_vec(),
        timeout: Duration::from_secs(cli.timeout),
        memory_limit: 2 << 30,
    }
}

fn evaluate(cli: &Cli, eng: &Rc<RefCell<Engine>>, entry: &Path, seed: u64, params: &[(String, String)]) -> Result<Document> {
    run(eng, entry, &options(cli, seed, params))
}

/// `a,b,c`, `a..b` (both included) or `a..b/s` (every s-th).
fn values(spec: &str) -> Result<Vec<String>> {
    match spec.split_once("..") {
        Some((a, b)) => {
            let (b, step) = b.split_once('/').unwrap_or((b, "1"));
            let (a, b, step): (i64, i64, usize) = (a.trim().parse()?, b.trim().parse()?, step.trim().parse()?);
            if step == 0 {
                bail!("the step after / must be at least 1");
            }
            Ok((a..=b).step_by(step).map(|v| v.to_string()).collect())
        }
        None => Ok(spec.split(',').map(|v| v.trim().to_string()).collect()),
    }
}

/// Renders `--onion`'s frames and overlays them.
fn onion_skin(cli: &Cli, eng: &Rc<RefCell<Engine>>, player: &mut Player, spec: &str) -> Result<(Vec<u8>, IRect)> {
    let frames: Vec<u32> = values(spec)
        .and_then(|vs| vs.iter().map(|v| v.parse::<u32>().map_err(|_| anyhow::anyhow!("{v:?} is not a frame number"))).collect())
        .with_context(|| format!("--onion {spec}"))?;
    let mut pixels = Vec::new();
    let mut region = None;
    for &f in &frames {
        let doc = player.document(f)?;
        let mut e = eng.borrow_mut();
        let (tex, r) = render::render_document(&mut e, &doc, scale(cli), crop_region(cli)?).with_context(|| format!("frame {f}"))?;
        if region.is_some_and(|r0: IRect| (r0.w, r0.h) != (r.w, r.h)) {
            bail!("frame {f} is {}x{}, unlike frame {}: every frame's Canvas must be the same size", r.w, r.h, frames[0]);
        }
        region = Some(r);
        pixels.push(e.gpu.read(&tex)?);
    }
    Ok((onion(&pixels)?, region.unwrap()))
}

/// The one frame `--frame` or `--time` asks for.
fn one_frame(cli: &Cli, fps: u32) -> Result<Option<u32>> {
    match (&cli.frame, &cli.time) {
        (Some(f), _) => Ok(Some(*f)),
        (_, Some(t)) => Ok(Some(frames_of(t, fps).context("--time")?)),
        _ => Ok(None),
    }
}

/// Renders the script once, returning the files it read.
fn render_once(
    cli: &Cli,
    eng: &Rc<RefCell<Engine>>,
    entry: &Path,
    out: &Path,
    seed: u64,
    params: &[(String, String)],
) -> Result<Vec<PathBuf>> {
    let t0 = Instant::now();
    let (session, loaded) = load(eng, entry, &options(cli, seed, params))?;
    let doc = match loaded {
        Loaded::Still(doc) => {
            if cli.frame.is_some() || cli.time.is_some() || cli.frames.is_some() || cli.onion.is_some() {
                bail!("--frame, --time, --frames and --onion need a script that returns an Animation {{ ... }}");
            }
            doc
        }
        Loaded::Anim(spec) => {
            let cache = eng.borrow().cache_dir.clone();
            let mut player = Player::new(session, spec, cache.as_deref())?;
            let res = animate(cli, eng, &mut player, out, t0);
            return res.map(|_| player.session.deps());
        }
    };
    let t_eval = t0.elapsed();
    if cli.emit_ir {
        std::fs::write(out, serde_json::to_string_pretty(&doc)?)?;
        println!("wrote {}", out.display());
        return Ok(session.deps());
    }
    let mut e = eng.borrow_mut();
    let (tex, region) = render::render_document(&mut e, &doc, scale(cli), crop_region(cli)?)?;
    let pixels = e.gpu.read(&tex)?;
    write_png(out, region.w, region.h, pixels)?;
    println!(
        "wrote {} ({}x{}, {:.0}ms; script {:.0}ms)",
        display(out),
        region.w,
        region.h,
        t0.elapsed().as_secs_f64() * 1e3,
        t_eval.as_secs_f64() * 1e3
    );
    Ok(session.deps())
}

/// Renders an Animation: one frame to a PNG (or IR), or a range of frames to a sink.
fn animate(cli: &Cli, eng: &Rc<RefCell<Engine>>, player: &mut Player, out: &Path, t0: Instant) -> Result<()> {
    let (fps, n) = (player.spec.fps, player.spec.frames);
    if let Some(spec) = &cli.onion {
        let (pixels, region) = onion_skin(cli, eng, player, spec)?;
        write_png(out, region.w, region.h, pixels)?;
        println!("wrote {} ({}x{}, onion of frames {spec}, {:.0}ms)", display(out), region.w, region.h, t0.elapsed().as_secs_f64() * 1e3);
        return Ok(());
    }
    if let Some(f) = one_frame(cli, fps)? {
        let t = Instant::now();
        let doc = player.document(f)?;
        let t_eval = t.elapsed();
        if cli.emit_ir {
            std::fs::write(out, serde_json::to_string_pretty(&doc)?)?;
            println!("wrote {}", out.display());
            return Ok(());
        }
        let mut e = eng.borrow_mut();
        let (tex, region) = render::render_document(&mut e, &doc, scale(cli), crop_region(cli)?)?;
        write_png(out, region.w, region.h, e.gpu.read(&tex)?)?;
        println!(
            "wrote {} ({}x{}, frame {f} at {:.3}s, {:.0}ms; script {:.0}ms)",
            display(out),
            region.w,
            region.h,
            f as f64 / fps as f64,
            t0.elapsed().as_secs_f64() * 1e3,
            t_eval.as_secs_f64() * 1e3
        );
        return Ok(());
    }
    if cli.emit_ir {
        bail!("--emit-ir writes one scene: pick a frame with --frame N or --time T");
    }
    let (a, b) = match &cli.frames {
        None => (0, n - 1),
        Some(r) => {
            let parsed = r.split_once("..").and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?)));
            match parsed {
                Some((a, b)) if a <= b && b < n => (a, b),
                Some(_) => bail!("--frames {r}: the animation has frames 0..{}", n - 1),
                None => bail!("--frames wants a..b (both included), got {r:?}"),
            }
        }
    };
    let mut sink: Option<(Sink, u32, u32)> = None;
    let mut t_eval = Duration::ZERO;
    for f in a..=b {
        let t = Instant::now();
        let doc = player.document(f)?;
        t_eval += t.elapsed();
        let mut e = eng.borrow_mut();
        if let Some(dir) = &cli.dump {
            e.dump_dir = Some(dir.join(format!("frame-{f:04}")));
        }
        let (tex, region) = render::render_document(&mut e, &doc, scale(cli), crop_region(cli)?).with_context(|| format!("frame {f}"))?;
        let pixels = e.gpu.read(&tex)?;
        let (w, h) = (region.w, region.h);
        if sink.is_none() {
            sink = Some((Sink::open(out, w, h, fps, a, b - a + 1, cli.encode.as_deref())?, w, h));
        }
        let (s, w0, h0) = sink.as_mut().unwrap();
        if (w, h) != (*w0, *h0) {
            bail!("frame {f} is {w}x{h} but frame {a} was {w0}x{h0}: every frame's Canvas must be the same size");
        }
        s.push(f, w, h, pixels)?;
    }
    let (s, w, h) = sink.unwrap();
    s.finish()?;
    println!(
        "wrote {} ({w}x{h}, {} frames at {fps} fps, {:.0}ms; script {:.0}ms)",
        display(out),
        b - a + 1,
        t0.elapsed().as_secs_f64() * 1e3,
        t_eval.as_secs_f64() * 1e3
    );
    Ok(())
}

fn display(p: &Path) -> String {
    std::env::current_dir().ok().and_then(|cwd| p.strip_prefix(cwd).ok().map(|r| r.display().to_string())).unwrap_or(p.display().to_string())
}

/// Renders every combination of the swept values in-process (one fresh VM
/// each, one shared GPU and cache), then tiles them into a labeled sheet.
fn sweep(cli: &Cli, eng: &Rc<RefCell<Engine>>, entry: &Path, out: &Path) -> Result<()> {
    let base = parse_params(&cli.params)?;
    let mut variants: Vec<Vec<(String, String)>> = vec![vec![]];
    for axis in &cli.sweep {
        let Some((k, vs)) = axis.split_once('=') else { bail!("--sweep wants k=a,b,c or k=1..9, got {axis:?}") };
        let values = values(vs).with_context(|| format!("--sweep {axis}"))?;
        variants = variants.iter().flat_map(|v| values.iter().map(move |x| [v.clone(), vec![(k.to_string(), x.clone())]].concat())).collect();
    }

    const CELL: f64 = 360.0;
    const PAD: f64 = 16.0;
    const LABEL: f64 = 22.0;
    let cols = cli.cols.unwrap_or_else(|| (variants.len() as f64).sqrt().ceil() as usize).max(1);
    let rows = variants.len().div_ceil(cols);
    let mut cells = Vec::new();
    let t0 = Instant::now();
    // Frames of one (seed, params) share a player, so its simulation runs once.
    let mut player: Option<(u64, Vec<(String, String)>, Player)> = None;
    for (i, v) in variants.iter().enumerate() {
        let seed = v.iter().find(|(k, _)| k == "seed").map(|(_, s)| s.parse()).transpose()?.unwrap_or(cli.seed);
        let frame = v.iter().find(|(k, _)| k == "frame").map(|(_, s)| s.parse::<u32>()).transpose()?;
        let mut params = base.clone();
        params.extend(v.iter().filter(|(k, _)| k != "seed" && k != "frame").cloned());
        let label = v.iter().map(|(k, x)| format!("{k}={x}")).collect::<Vec<_>>().join("  ");
        if frame.is_some() && cli.onion.is_some() {
            bail!("--onion picks its own frames, so it can't be combined with a frame= sweep");
        }
        let animated = frame.is_some() || cli.frame.is_some() || cli.time.is_some() || cli.onion.is_some();
        let doc = if animated {
            if !matches!(&player, Some((s, p, _)) if *s == seed && *p == params) {
                let (session, loaded) = load(eng, entry, &options(cli, seed, &params)).with_context(|| label.clone())?;
                let Loaded::Anim(spec) = loaded else { bail!("frame sweeps and --frame need a script that returns an Animation {{ ... }}") };
                let cache = eng.borrow().cache_dir.clone();
                player = Some((seed, params.clone(), Player::new(session, spec, cache.as_deref())?));
            }
            let p = &mut player.as_mut().unwrap().2;
            match (frame, &cli.onion) {
                (Some(f), _) => Some(p.document(f).with_context(|| label.clone())?),
                (None, Some(_)) => None,
                (None, None) => Some(p.document(one_frame(cli, p.spec.fps)?.unwrap()).with_context(|| label.clone())?),
            }
        } else {
            Some(evaluate(cli, eng, entry, seed, &params).with_context(|| label.clone())?)
        };
        let (pixels, region) = match doc {
            Some(doc) => {
                let mut e = eng.borrow_mut();
                let (tex, region) = render::render_document(&mut e, &doc, scale(cli), crop_region(cli)?)?;
                (e.gpu.read(&tex)?, region)
            }
            None => onion_skin(cli, eng, &mut player.as_mut().unwrap().2, cli.onion.as_deref().unwrap()).with_context(|| label.clone())?,
        };
        let mut e = eng.borrow_mut();
        let size = [region.w as f64 / scale(cli), region.h as f64 / scale(cli)];
        let r = e.adopt_pixels(format!("sweep-{i}"), size, scale(cli) * 96.0, region.w, region.h, pixels);
        let k = (CELL / r.size[0]).min(CELL / r.size[1]);
        let (w, h) = (r.size[0] * k, r.size[1] * k);
        let (x, y) = (PAD + (i % cols) as f64 * (CELL + PAD), PAD + (i / cols) as f64 * (CELL + LABEL + PAD));
        cells.push(plain(Kind::Image {
            src: ImageSrc::Raster(r.key.clone()),
            dst: [x + (CELL - w) / 2.0, y + (CELL - h) / 2.0, w, h],
            crop: [0.0, 0.0, r.size[0], r.size[1]],
            fit: Fit::Fill,
        }));
        cells.push(plain(Kind::Text(Text {
            text: label,
            x,
            y: y + CELL + 14.0,
            w: Some(CELL),
            font: Font::Family("Menlo".into()),
            size: 12.0,
            fill: Paint::Solid { color: [0.2, 0.2, 0.2, 1.0] },
            align: Align::Left,
            anchor: Anchor::Start,
            leading: None,
            tracking: 0.0,
            weight: None,
            italic: false,
            features: vec![],
            variations: vec![],
        })));
    }
    let sheet = Document {
        size: [PAD + cols as f64 * (CELL + PAD), PAD + rows as f64 * (CELL + LABEL + PAD)],
        bg: [0.9, 0.9, 0.9, 1.0],
        margin: None,
        children: cells,
    };
    let mut e = eng.borrow_mut();
    let (tex, region) = render::render_document(&mut e, &sheet, 1.0, None)?;
    write_png(out, region.w, region.h, e.gpu.read(&tex)?)?;
    println!("wrote {} ({} variants, {:.0}ms)", display(out), variants.len(), t0.elapsed().as_secs_f64() * 1e3);
    Ok(())
}

fn plain(kind: Kind) -> Node {
    Node { kind, opacity: 1.0, blend: ir::Blend::Normal, blur: 0.0, at: String::new() }
}
