mod effects;
mod engine;
mod gpu;
mod ir;
mod render;
mod script;
mod text;
mod units;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use clap::Parser;

use crate::engine::{Engine, write_png};
use crate::gpu::IRect;
use crate::ir::{Align, Anchor, Document, Fit, Font, ImageSrc, Kind, Node, Paint, Text};
use crate::script::{RunOptions, run};
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

    let t0 = Instant::now();
    let mut eng = Engine::new(root)?;
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

fn evaluate(cli: &Cli, eng: &Rc<RefCell<Engine>>, entry: &Path, seed: u64, params: &[(String, String)]) -> Result<script::RunResult> {
    let opts = RunOptions {
        seed,
        params: params.to_vec(),
        timeout: Duration::from_secs(cli.timeout),
        memory_limit: 2 << 30,
    };
    run(eng, entry, &opts)
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
    let res = evaluate(cli, eng, entry, seed, params)?;
    let t_eval = t0.elapsed();
    if cli.emit_ir {
        std::fs::write(out, serde_json::to_string_pretty(&res.doc)?)?;
        println!("wrote {}", out.display());
        return Ok(res.deps);
    }
    let mut e = eng.borrow_mut();
    let (tex, region) = render::render_document(&mut e, &res.doc, scale(cli), crop_region(cli)?)?;
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
    Ok(res.deps)
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
        let values: Vec<String> = match vs.split_once("..") {
            Some((a, b)) => {
                let (a, b): (i64, i64) = (a.parse()?, b.parse()?);
                (a..=b).map(|v| v.to_string()).collect()
            }
            None => vs.split(',').map(String::from).collect(),
        };
        variants = variants.iter().flat_map(|v| values.iter().map(move |x| [v.clone(), vec![(k.to_string(), x.clone())]].concat())).collect();
    }

    const CELL: f64 = 360.0;
    const PAD: f64 = 16.0;
    const LABEL: f64 = 22.0;
    let cols = cli.cols.unwrap_or_else(|| (variants.len() as f64).sqrt().ceil() as usize).max(1);
    let rows = variants.len().div_ceil(cols);
    let mut cells = Vec::new();
    let t0 = Instant::now();
    for (i, v) in variants.iter().enumerate() {
        let seed = v.iter().find(|(k, _)| k == "seed").map(|(_, s)| s.parse()).transpose()?.unwrap_or(cli.seed);
        let mut params = base.clone();
        params.extend(v.iter().filter(|(k, _)| k != "seed").cloned());
        let label = v.iter().map(|(k, x)| format!("{k}={x}")).collect::<Vec<_>>().join("  ");
        let res = evaluate(cli, eng, entry, seed, &params).with_context(|| label.clone())?;
        let mut e = eng.borrow_mut();
        let (tex, region) = render::render_document(&mut e, &res.doc, scale(cli), crop_region(cli)?)?;
        let pixels = e.gpu.read(&tex)?;
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
