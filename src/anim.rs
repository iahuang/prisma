//! Animation: a script that returns `Animation { fps, duration, substeps, init, step, frame }`.
//!
//! A frame is a function of (seed, params, frame index), never of what was
//! rendered before it. Simulation state is plain data the engine owns: `init`
//! makes it, `step(s, dt, n)` changes it in place at a fixed timestep, and
//! `frame(s, t, f)` draws it. The state is serialized every `CHECKPOINT_EVERY`
//! frames (in memory, and in `.prisma-cache/` keyed by the script's sources,
//! seed, params and timestep), so rendering frame N restores the nearest
//! checkpoint and steps forward from there.
//!
//! State kept anywhere but `s` (upvalues, globals) is undefined behaviour: the
//! engine doesn't check for it.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow, bail};
use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};

use crate::engine::write_png;
use crate::ir::Document;
use crate::script::Session;

/// Bump when the checkpoint format or stepping changes.
const STATE_VERSION: u32 = 1;
pub const CHECKPOINT_EVERY: u32 = 30;

/// What `Animation { ... }` declared.
pub struct Spec {
    pub fps: u32,
    pub frames: u32,
    pub substeps: u32,
    pub init: Option<Function>,
    pub step: Option<Function>,
    pub frame: Function,
}

/// A frame count from a time: a number of seconds, or a string in `s`, `ms` or `f` (frames).
/// It must land on a whole frame.
pub fn frames_of(v: &str, fps: u32) -> Result<u32> {
    let v = v.trim();
    let (num, unit) = match v.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => v.split_at(i),
        None => (v, "s"),
    };
    let n: f64 = num.trim().parse().map_err(|_| anyhow!("bad time {v:?} (use seconds, or e.g. \"1.5s\", \"500ms\", \"45f\")"))?;
    let f = match unit {
        "s" => n * fps as f64,
        "ms" => n / 1000.0 * fps as f64,
        "f" => n,
        _ => bail!("bad time unit in {v:?} (use s, ms or f)"),
    };
    if !f.is_finite() || f < 0.0 {
        bail!("time {v:?} must be zero or more");
    }
    let r = f.round();
    if (f - r).abs() > 1e-6 {
        bail!("{v} is {f:.3} frames at {fps} fps, between frames {} and {} (give a time on a frame, or a frame count like \"{}f\")", f.floor(), f.ceil(), r);
    }
    Ok(r as u32)
}

// ---- keyed randomness ----------------------------------------------------------

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A hash of the run's seed and some keys. The same keys give the same value in any order of calls.
pub fn key_hash(seed: u64, keys: impl IntoIterator<Item = u64>) -> u64 {
    keys.into_iter().fold(splitmix(seed ^ 0x7072_6973_6d61), |h, k| splitmix(h ^ splitmix(k)))
}

/// One `rng(...)` key as bits. Numbers hash by value (-0 is 0), strings by bytes.
pub fn key_bits(v: &Value) -> Result<u64> {
    let num = |n: f64| splitmix(1) ^ (if n == 0.0 { 0.0f64 } else { n }).to_bits();
    match v {
        Value::Integer(i) => Ok(num(*i as f64)),
        Value::Number(n) => Ok(num(*n)),
        Value::Boolean(b) => Ok(splitmix(3) ^ *b as u64),
        Value::String(s) => Ok(s.as_bytes().iter().fold(splitmix(2) ^ 0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))),
        other => bail!("rng keys are numbers, strings or booleans, got {}", other.type_name()),
    }
}

/// `rng(key, ...)`: a random stream fixed by the run's seed and its keys.
pub struct Rng(u64);

impl Rng {
    pub fn new(h: u64) -> Rng {
        Rng(h)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        splitmix(self.0)
    }
    fn float(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

impl UserData for Rng {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // r:float() in [0, 1)
        m.add_method_mut("float", |_, this, ()| Ok(this.float()));
        // r:range(a, b) in [a, b)
        m.add_method_mut("range", |_, this, (a, b): (f64, f64)| Ok(a + this.float() * (b - a)));
        // r:int(a, b): a whole number from a to b, both included
        m.add_method_mut("int", |_, this, (a, b): (f64, f64)| {
            if a.fract() != 0.0 || b.fract() != 0.0 || b < a {
                return Err(mlua::Error::runtime(format!("rng:int(a, b) wants whole numbers with a <= b, got {a} and {b}")));
            }
            Ok(a + (this.float() * (b - a + 1.0)).floor())
        });
        // r:normal() with mean 0 and standard deviation 1
        m.add_method_mut("normal", |_, this, ()| {
            let u = 1.0 - this.float();
            Ok((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * this.float()).cos())
        });
    }
}

// ---- state as data -------------------------------------------------------------

const NIL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const NUM: u8 = 3;
const STR: u8 = 4;
const TABLE: u8 = 5;

/// Serializes state: tables (no cycles; metatables dropped), numbers, strings, booleans.
fn encode(v: &Value, path: &mut Vec<String>, open: &mut Vec<*const c_void>, out: &mut Vec<u8>) -> Result<()> {
    let at = |path: &Vec<String>| format!("s{}", path.concat());
    match v {
        Value::Nil => out.push(NIL),
        Value::Boolean(b) => out.push(if *b { TRUE } else { FALSE }),
        Value::Integer(i) => {
            out.push(NUM);
            out.extend((*i as f64).to_le_bytes());
        }
        Value::Number(n) => {
            out.push(NUM);
            out.extend(n.to_le_bytes());
        }
        Value::String(s) => {
            out.push(STR);
            out.extend((s.as_bytes().len() as u32).to_le_bytes());
            out.extend(&*s.as_bytes());
        }
        Value::Table(t) => {
            if open.contains(&t.to_pointer()) {
                bail!("{} refers back to a table that contains it (state must be a tree of plain data)", at(path));
            }
            open.push(t.to_pointer());
            let n = t.raw_len();
            let mut rest = Vec::new();
            for kv in t.pairs::<Value, Value>() {
                let (k, v) = kv?;
                let in_array = match k {
                    Value::Integer(i) => (1..=n as i64).contains(&i),
                    Value::Number(x) => x.fract() == 0.0 && x >= 1.0 && x <= n as f64,
                    _ => false,
                };
                if !in_array {
                    rest.push((k, v));
                }
            }
            out.push(TABLE);
            out.extend((n as u32).to_le_bytes());
            out.extend((rest.len() as u32).to_le_bytes());
            for i in 1..=n {
                path.push(format!("[{i}]"));
                encode(&t.raw_get::<Value>(i)?, path, open, out)?;
                path.pop();
            }
            for (k, v) in rest {
                let name = match &k {
                    Value::String(s) => format!(".{}", s.to_string_lossy()),
                    Value::Number(x) => format!("[{x}]"),
                    Value::Integer(x) => format!("[{x}]"),
                    Value::Boolean(b) => format!("[{b}]"),
                    other => bail!("{} has a {} key (state keys are strings, numbers or booleans)", at(path), other.type_name()),
                };
                path.push(name);
                encode(&k, path, open, out)?;
                encode(&v, path, open, out)?;
                path.pop();
            }
            open.pop();
        }
        other => bail!(
            "{} is a {} (state is plain data: tables, numbers, strings and booleans; keep functions and rng(...) outside it)",
            at(path),
            other.type_name()
        ),
    }
    Ok(())
}

fn decode(lua: &Lua, b: &mut &[u8]) -> Result<Value> {
    let mut take = |n: usize| -> Result<&[u8]> {
        if b.len() < n {
            bail!("truncated checkpoint");
        }
        let (head, tail) = b.split_at(n);
        *b = tail;
        Ok(head)
    };
    let tag = take(1)?[0];
    Ok(match tag {
        NIL => Value::Nil,
        FALSE => Value::Boolean(false),
        TRUE => Value::Boolean(true),
        NUM => Value::Number(f64::from_le_bytes(take(8)?.try_into()?)),
        STR => {
            let n = u32::from_le_bytes(take(4)?.try_into()?) as usize;
            Value::String(lua.create_string(take(n)?)?)
        }
        TABLE => {
            let narr = u32::from_le_bytes(take(4)?.try_into()?) as usize;
            let nrec = u32::from_le_bytes(take(4)?.try_into()?) as usize;
            let t = lua.create_table_with_capacity(narr, nrec)?;
            for i in 1..=narr {
                t.raw_set(i, decode(lua, b)?)?;
            }
            for _ in 0..nrec {
                let k = decode(lua, b)?;
                t.raw_set(k, decode(lua, b)?)?;
            }
            Value::Table(t)
        }
        _ => bail!("bad checkpoint tag {tag}"),
    })
}

// ---- playback -------------------------------------------------------------------

/// Plays an animation frame by frame: owns its VM, its live state and its checkpoints.
pub struct Player {
    pub session: Session,
    pub spec: Spec,
    /// `.prisma-cache/sim-<key>/` for checkpoints shared across runs.
    dir: Option<PathBuf>,
    saved: BTreeMap<u32, Vec<u8>>,
    /// The state after frame `.0`'s steps.
    live: Option<(u32, Table)>,
}

impl Player {
    pub fn new(session: Session, spec: Spec, cache_dir: Option<&Path>) -> Result<Player> {
        let dir = match (cache_dir, &spec.init) {
            (Some(c), Some(_)) => {
                let mut h = blake3::Hasher::new();
                h.update(&STATE_VERSION.to_le_bytes());
                h.update(&[spec.fps.to_le_bytes(), spec.substeps.to_le_bytes()].concat());
                h.update(&session.seed().to_le_bytes());
                for (k, v) in session.params() {
                    h.update(format!("{k}={v}\0").as_bytes());
                }
                for p in session.deps() {
                    h.update(p.to_string_lossy().as_bytes());
                    h.update(&std::fs::read(&p).unwrap_or_default());
                }
                Some(c.join(format!("sim-{}", &h.finalize().to_hex()[..16])))
            }
            _ => None,
        };
        Ok(Player { session, spec, dir, saved: BTreeMap::new(), live: None })
    }

    fn checkpoint(&mut self, f: u32, s: &Table) -> Result<()> {
        if self.saved.contains_key(&f) {
            return Ok(());
        }
        let mut bytes = Vec::new();
        encode(&Value::Table(s.clone()), &mut vec![], &mut vec![], &mut bytes).with_context(|| format!("state after frame {f}"))?;
        if let Some(dir) = &self.dir {
            let file = dir.join(format!("{f}.state"));
            if !file.exists() {
                std::fs::create_dir_all(dir)?;
                let tmp = dir.join(format!("{f}.state.tmp"));
                std::fs::write(&tmp, &bytes)?;
                std::fs::rename(tmp, file)?;
            }
        }
        self.saved.insert(f, bytes);
        Ok(())
    }

    /// A checkpoint at frame `f`, from memory or disk.
    fn restore(&mut self, f: u32) -> Result<Option<Table>> {
        let bytes = match self.saved.get(&f) {
            Some(b) => b.clone(),
            None => match self.dir.as_ref().and_then(|d| std::fs::read(d.join(format!("{f}.state"))).ok()) {
                Some(b) => {
                    self.saved.insert(f, b.clone());
                    b
                }
                None => return Ok(None),
            },
        };
        match decode(self.session.lua(), &mut &bytes[..])? {
            Value::Table(t) => Ok(Some(t)),
            _ => bail!("bad checkpoint"),
        }
    }

    /// The state after frame `f`'s steps (frame 0 is `init()`'s state).
    fn state(&mut self, f: u32) -> Result<Value> {
        let (Some(init), Some(step)) = (self.spec.init.clone(), self.spec.step.clone()) else { return Ok(Value::Nil) };
        let mut cur = self.live.take().filter(|(g, _)| *g <= f);
        let from = cur.as_ref().map(|(g, _)| *g);
        let mut c = f / CHECKPOINT_EVERY * CHECKPOINT_EVERY;
        while from.is_none_or(|g| c > g) {
            if let Some(t) = self.restore(c)? {
                cur = Some((c, t));
                break;
            }
            if c == 0 {
                break;
            }
            c -= CHECKPOINT_EVERY;
        }
        let (mut g, s) = match cur {
            Some(x) => x,
            None => {
                self.session.reseed(&[0, 0])?;
                self.session.tick();
                let s = match init.call::<Value>(()).map_err(|e| anyhow!("init: {e}"))? {
                    Value::Table(t) => t,
                    other => bail!("init must return the state as a table, got {}", other.type_name()),
                };
                self.checkpoint(0, &s)?;
                (0, s)
            }
        };
        let dt = 1.0 / (self.spec.fps * self.spec.substeps) as f64;
        while g < f {
            for j in 0..self.spec.substeps {
                let n = (g * self.spec.substeps + j) as f64;
                self.session.reseed(&[1, n.to_bits()])?;
                self.session.tick();
                let ret: Value = step.call((s.clone(), dt, n)).map_err(|e| anyhow!("step {n}: {e}"))?;
                if !ret.is_nil() {
                    bail!("step {n}: step changes s in place and returns nothing, but it returned a {}", ret.type_name());
                }
            }
            g += 1;
            if g % CHECKPOINT_EVERY == 0 {
                self.checkpoint(g, &s)?;
            }
        }
        self.live = Some((f, s.clone()));
        Ok(Value::Table(s))
    }

    /// Frame `f` as a document.
    pub fn document(&mut self, f: u32) -> Result<Document> {
        if f >= self.spec.frames {
            bail!("frame {f} is past the end: the animation has {} frames (0..{})", self.spec.frames, self.spec.frames - 1);
        }
        let s = self.state(f)?;
        self.session.reseed(&[2, f as u64])?;
        self.session.tick();
        let t = f as f64 / self.spec.fps as f64;
        let root: Value = self.spec.frame.call((s, t, f)).map_err(|e| anyhow!("frame {f}: {e}"))?;
        self.session.document(root).with_context(|| format!("frame {f}"))
    }
}

// ---- onion skin -----------------------------------------------------------------

/// Overlays straight-RGBA frames of one size: the last is kept exactly, and the
/// earlier ones show as ghosts that fade with age. A ghost is where a frame
/// differs from the still background, taken as the per-pixel median of all the
/// frames, and it's drawn only where the last frame shows that background.
pub fn onion(frames: &[Vec<u8>]) -> Result<Vec<u8>> {
    let n = frames.len();
    if n < 3 {
        bail!("--onion needs at least 3 frames to find the still background (the per-pixel median), got {n}");
    }
    let len = frames[0].len();
    let mut plate = vec![0u8; len];
    let mut col = vec![0u8; n];
    for (i, p) in plate.iter_mut().enumerate() {
        for (c, f) in col.iter_mut().zip(frames) {
            *c = f[i];
        }
        col.sort_unstable();
        *p = col[n / 2];
    }
    // How much a pixel differs from the plate: 0 within 2 levels, 1 past 24.
    let ink = |f: &[u8], px: usize| {
        let d = (0..4).map(|c| f[px + c].abs_diff(plate[px + c])).max().unwrap() as f32;
        ((d - 2.0) / 22.0).clamp(0.0, 1.0)
    };
    let last = &frames[n - 1];
    let mut out = last.clone();
    let ghosts = n - 1;
    for px in (0..len).step_by(4) {
        let open = 1.0 - ink(last, px);
        if open == 0.0 {
            continue;
        }
        // Oldest first, so newer ghosts land on top.
        let mut acc = [0f32; 4];
        for c in 0..4 {
            acc[c] = plate[px + c] as f32;
        }
        for (j, f) in frames[..ghosts].iter().enumerate() {
            let age = (ghosts - j) as f32; // 1 = the frame before the last
            let a = 0.7 * (1.0 - (age - 1.0) / ghosts as f32) * ink(f, px);
            for c in 0..4 {
                acc[c] += (f[px + c] as f32 - acc[c]) * a;
            }
        }
        for c in 0..4 {
            let v = out[px + c] as f32;
            out[px + c] = (v + (acc[c] - v) * open).round() as u8;
        }
    }
    Ok(out)
}

// ---- output ---------------------------------------------------------------------

/// Where rendered frames go, chosen by `-o`: a directory of PNGs, an APNG, or a video through ffmpeg.
pub enum Sink {
    Seq(PathBuf),
    Apng(png::Writer<BufWriter<std::fs::File>>),
    Ffmpeg { child: Child, stdin: Option<ChildStdin>, stderr: Option<JoinHandle<String>>, opaque: bool },
}

/// The ffmpeg arguments for each video extension. RGBA is converted to YUV with
/// the bt709 matrix and tagged with it, so players don't guess.
fn codec_args(ext: &str) -> Option<String> {
    // Tag the frames themselves; encoders and muxers copy the tags from them.
    const TV: &str = "-vf scale=out_color_matrix=bt709:out_range=tv,setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv";
    Some(match ext {
        "mp4" => format!("-c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p {TV} -movflags +faststart"),
        "webm" => format!("-c:v libvpx-vp9 -crf 30 -b:v 0 -pix_fmt yuva420p {TV}"),
        "mov" => format!("-c:v prores_ks -profile:v 4444 -pix_fmt yuva444p10le {TV}"),
        "gif" => "-vf split[a][b];[a]palettegen[p];[b][p]paletteuse -loop 0".into(),
        _ => return None,
    })
}

pub fn is_dir_out(out: &Path) -> bool {
    out.to_string_lossy().ends_with('/') || out.is_dir()
}

impl Sink {
    /// `first` is the first frame's index, which names the first file of a PNG sequence.
    pub fn open(out: &Path, w: u32, h: u32, fps: u32, first: u32, frames: u32, encode: Option<&str>) -> Result<Sink> {
        let ext = out.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if (is_dir_out(out) || ext == "png") && encode.is_some() {
            bail!("--encode only applies to video outputs (.mp4, .webm, .mov, .gif)");
        }
        if is_dir_out(out) {
            std::fs::create_dir_all(out)?;
            return Ok(Sink::Seq(out.to_path_buf()));
        }
        if ext == "png" {
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let file = BufWriter::new(std::fs::File::create(out).with_context(|| format!("writing {}", out.display()))?);
            let mut enc = png::Encoder::new(file, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            enc.set_animated(frames, 0)?;
            enc.set_frame_delay(1, fps as u16)?;
            return Ok(Sink::Apng(enc.write_header()?));
        }
        let Some(default) = codec_args(&ext) else {
            bail!("can't write an animation to {:?}: use DIR/ (PNG frames), .png (APNG), .mp4, .webm, .mov or .gif", out.display().to_string());
        };
        if matches!(ext.as_str(), "mp4" | "webm") && (w % 2 == 1 || h % 2 == 1) {
            bail!(".{ext} stores colour at half resolution, so it needs an even width and height; this render is {w}x{h} (change the Canvas size, --scale or --crop)");
        }
        let args = encode.map(String::from).unwrap_or(default);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let spawned = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-nostats", "-y", "-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{w}x{h}"), "-framerate", &fps.to_string(), "-i", "-"])
            .args(args.split_whitespace())
            .arg(out)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "{} needs ffmpeg on PATH (brew install ffmpeg). Or write PNG frames with -o DIR/ and encode them yourself:\n  ffmpeg -framerate {fps}{} -i DIR/%04d.png {args} {}",
                out.display(),
                if first > 0 { format!(" -start_number {first}") } else { String::new() },
                out.display()
            ),
            Err(e) => return Err(e).context("starting ffmpeg"),
        };
        let mut pipe = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = pipe.read_to_string(&mut s);
            s
        });
        let stdin = child.stdin.take();
        Ok(Sink::Ffmpeg { child, stdin, stderr: Some(stderr), opaque: matches!(ext.as_str(), "mp4" | "gif") })
    }

    pub fn push(&mut self, f: u32, w: u32, h: u32, pixels: Vec<u8>) -> Result<()> {
        match self {
            Sink::Seq(dir) => write_png(&dir.join(format!("{f:04}.png")), w, h, pixels),
            Sink::Apng(writer) => Ok(writer.write_image_data(&pixels)?),
            Sink::Ffmpeg { stdin, opaque, .. } => {
                if *opaque && pixels.chunks_exact(4).any(|p| p[3] < 255) {
                    bail!("frame {f} has transparency, which this format can't store (give the Canvas an opaque bg, or write .webm, .mov or .png)");
                }
                match stdin.as_mut().unwrap().write_all(&pixels) {
                    Ok(()) => Ok(()),
                    // ffmpeg quit: finish() reports why.
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                        stdin.take();
                        self.finish_ffmpeg()
                    }
                    Err(e) => Err(e.into()),
                }
            }
        }
    }

    fn finish_ffmpeg(&mut self) -> Result<()> {
        let Sink::Ffmpeg { child, stdin, stderr, .. } = self else { return Ok(()) };
        drop(stdin.take());
        let status = child.wait()?;
        let log = stderr.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        if !status.success() {
            bail!("ffmpeg failed ({status}):\n{}", log.trim());
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        match self {
            Sink::Seq(_) => Ok(()),
            Sink::Apng(writer) => Ok(writer.finish()?),
            Sink::Ffmpeg { .. } => self.finish_ffmpeg(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_land_on_frames() {
        assert_eq!(frames_of("1.5s", 30).unwrap(), 45);
        assert_eq!(frames_of("1.1", 30).unwrap(), 33);
        assert_eq!(frames_of("500ms", 24).unwrap(), 12);
        assert_eq!(frames_of("7f", 24).unwrap(), 7);
        assert!(frames_of("0.1s", 24).is_err());
        assert!(frames_of("2m", 24).is_err());
    }

    #[test]
    fn state_round_trips_exactly() {
        let lua = Lua::new();
        let v: Value = lua.load("return { 1.1, -0.0, 0/0, { x = 1e-300, y = 'a\\0b' }, flag = true, [2.5] = 3 }").eval().unwrap();
        let mut a = Vec::new();
        encode(&v, &mut vec![], &mut vec![], &mut a).unwrap();
        let back = decode(&lua, &mut &a[..]).unwrap();
        let mut b = Vec::new();
        encode(&back, &mut vec![], &mut vec![], &mut b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn state_rejects_code_and_cycles() {
        let lua = Lua::new();
        let f: Value = lua.load("return { boids = { { x = 1, go = function() end } } }").eval().unwrap();
        let err = encode(&f, &mut vec![], &mut vec![], &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("s.boids[1].go is a function"), "{err}");
        let c: Value = lua.load("local t = {} t.me = t return t").eval().unwrap();
        assert!(encode(&c, &mut vec![], &mut vec![], &mut vec![]).is_err());
    }
}
