//! The Luau front end.
//!
//! Elements are plain tables: `Rect { x = 10, w = 40, h = 20, fill = "#f00" }`
//! calls the `Rect` constructor with a table whose hash part holds props and
//! whose array part holds children. Constructors only tag the table (`_op`,
//! plus `_at`, the calling line, for error messages); nothing is validated or
//! drawn until the script returns and the tree is converted to IR here.
//!
//! The VM is sandboxed: Luau's safe libs only, read-only globals, a memory cap,
//! a wall-clock limit, and file access (require, images, fonts) confined to the
//! project directory.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, Value, VmState};
use vello::kurbo::{Affine, BezPath, Rect, Shape};

use crate::effects::{EffectDef, InputKind, Margin, UType};
use crate::engine::{Engine, Raster};
use crate::ir::{Align, Anchor, Blend, Cap, Document, Fit, Font, Geom, ImageSrc, Join, Kind, Node, Paint, Stroke, Text, UValue};
use crate::units::{Rgba, parse_color, parse_length};

const PRELUDE: &str = include_str!("prelude.luau");

const PRIMITIVES: &[&str] = &["Canvas", "Group", "Mask", "Rect", "Circle", "Ellipse", "Line", "Path", "Text", "Image", "Sdf"];

pub struct RunOptions {
    pub seed: u64,
    pub params: Vec<(String, String)>,
    pub timeout: Duration,
    pub memory_limit: usize,
}

pub struct RunResult {
    pub doc: Document,
    /// Every file the run read, for watch mode.
    pub deps: Vec<PathBuf>,
}

type Shared = Rc<RefCell<Engine>>;
type Deps = Rc<RefCell<BTreeSet<PathBuf>>>;

fn lua_err(e: anyhow::Error) -> mlua::Error {
    mlua::Error::external(format!("{e:#}"))
}

/// "poster.luau:12" for the Lua code that called the current Rust function.
fn caller(lua: &Lua) -> String {
    lua.inspect_stack(1, |d| format!("{}:{}", chunk_file(&d.source()).unwrap_or("?"), d.current_line().unwrap_or(0))).unwrap_or_default()
}

/// A chunk's file: scripts and modules are named `@<path from the entry script's directory>`.
fn chunk_file<'a>(src: &'a mlua::debug::DebugSource) -> Option<&'a str> {
    src.source.as_deref().and_then(|s| s.strip_prefix('@')).or(src.short_src.as_deref())
}

/// `rel` as written in `file` (a chunk's file, or "" for the entry script's
/// directory), as a path from the entry script's directory. Relative paths
/// resolve from the file they're written in, so shared helpers work from any script.
fn from_file(file: &str, rel: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in Path::new(file).parent().unwrap_or(Path::new("")).join(rel).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

pub fn run(eng: &Shared, entry: &Path, opts: &RunOptions) -> Result<RunResult> {
    // Each run defines its effects afresh, so watch mode and sweeps see edited code.
    eng.borrow_mut().effects.defs.clear();
    let lua = Lua::new();
    lua.set_memory_limit(opts.memory_limit)?;
    let deps: Deps = Rc::new(RefCell::new(BTreeSet::from([entry.to_path_buf()])));
    install(&lua, eng, &deps, opts)?;
    lua.load(PRELUDE).set_name("@prelude.luau").exec().map_err(|e| anyhow!("prelude: {e}"))?;
    lua.load(format!("math.randomseed({})", opts.seed)).exec()?;
    // Code loading and environment poking have no place in a design script.
    for name in ["loadstring", "getfenv", "setfenv"] {
        lua.globals().raw_set(name, Value::Nil)?;
    }
    lua.sandbox(true)?;

    let start = Instant::now();
    let timeout = opts.timeout;
    lua.set_interrupt(move |_| {
        if start.elapsed() > timeout {
            return Err(mlua::Error::runtime(format!("script exceeded {}s (raise with --timeout)", timeout.as_secs())));
        }
        Ok(VmState::Continue)
    });

    let src = std::fs::read_to_string(entry).with_context(|| format!("reading {}", entry.display()))?;
    let name = entry.file_name().unwrap_or_default().to_string_lossy();
    let mut root: Value = lua.load(&src).set_name(format!("@{name}")).eval().map_err(|e| anyhow!("{e}"))?;
    if let Value::Function(f) = &root {
        let params: Value = lua.globals().get("params")?;
        root = f.call(params).map_err(|e| anyhow!("{e}"))?;
    }
    let doc = {
        let e = eng.borrow();
        let conv = Conv { eng: &e, deps: &deps };
        conv.document(root)?
    };
    let deps = deps.borrow().iter().cloned().collect();
    Ok(RunResult { doc, deps })
}

fn install(lua: &Lua, eng: &Shared, deps: &Deps, opts: &RunOptions) -> Result<()> {
    let g = lua.globals();

    for &op in PRIMITIVES {
        g.set(
            op,
            lua.create_function(move |lua, t: Value| {
                let Value::Table(t) = t else {
                    return Err(mlua::Error::runtime(format!("{}: {op} expects a table: {op} {{ ... }}", caller(lua))));
                };
                t.raw_set("_op", op)?;
                t.raw_set("_at", caller(lua))?;
                Ok(t)
            })?,
        )?;
    }

    // `-p k=v` values stay the strings typed; `param(name, default)` reads one
    // as the type of its default, and errors when it isn't one.
    let params = lua.create_table()?;
    for (k, v) in &opts.params {
        params.set(k.as_str(), v.as_str())?;
    }
    g.set("params", params)?;
    let given = opts.params.clone();
    g.set(
        "param",
        lua.create_function(move |lua, (name, dflt): (String, Value)| {
            let at = caller(lua);
            let raw = given.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str());
            let fail = |msg: String| Err(lua_err(anyhow!("{at}: param {name:?}: {msg}")));
            match (raw, &dflt) {
                (None, Value::Nil) => fail(format!("missing (pass -p {name}=...)")),
                (None, _) => Ok(dflt),
                (Some(v), Value::Nil | Value::String(_)) => Ok(Value::String(lua.create_string(v)?)),
                (Some(v), Value::Integer(_) | Value::Number(_)) => match v.trim().parse::<f64>() {
                    Ok(n) if n.is_finite() => Ok(Value::Number(n)),
                    _ => fail(format!("-p {name}={v} is not a number (its default is a number)")),
                },
                (Some(v), Value::Boolean(_)) => match v {
                    "true" => Ok(Value::Boolean(true)),
                    "false" => Ok(Value::Boolean(false)),
                    _ => fail(format!("-p {name}={v} is not true or false (its default is a boolean)")),
                },
                (Some(_), other) => fail(format!("the default must be a number, string or boolean, got {}", type_name(other))),
            }
        })?,
    )?;

    let e = eng.clone();
    g.set(
        "effect",
        lua.create_function(move |lua, t: Table| {
            let at = caller(lua);
            let def = parse_effect(&t).map_err(|err| lua_err(anyhow!("{at}: {err:#}")))?;
            let name = def.name.clone();
            e.borrow_mut().effects.define(def).map_err(lua_err)?;
            lua.create_function(move |lua, t: Table| {
                t.raw_set("_op", "Effect")?;
                t.raw_set("_effect", name.as_str())?;
                t.raw_set("_at", caller(lua))?;
                Ok(t)
            })
        })?,
    )?;

    let (e, d) = (eng.clone(), deps.clone());
    g.set(
        "rasterize",
        lua.create_function(move |lua, (scene, o): (Value, Table)| {
            let at = caller(lua);
            let wrap = |err: anyhow::Error| lua_err(anyhow!("{at}: rasterize: {err:#}"));
            let size = point(&o.get::<Value>("size")?).map_err(|err| wrap(err.context("size")))?;
            let dpi = o.get::<Option<f64>>("dpi")?.unwrap_or(96.0);
            let bg = match o.get::<Option<String>>("bg")? {
                Some(c) => parse_color(&c).map_err(wrap)?,
                None => [0.0; 4],
            };
            let margin = match o.get::<Value>("margin")? {
                Value::Nil => None,
                v => Some(margin(&v).map_err(|err| wrap(err.context("margin")))?),
            };
            let name = o.get::<Option<String>>("name")?;
            let doc = {
                let eng = e.borrow();
                let conv = Conv { eng: &eng, deps: &d };
                let mut children = Vec::new();
                conv.nodes(scene, &mut children).map_err(wrap)?;
                Document { size, bg, margin, children }
            };
            let mut eng = e.borrow_mut();
            let r = eng.rasterize(&doc, dpi).map_err(wrap)?;
            if let (Some(name), Some(dir)) = (name, &eng.dump_dir) {
                crate::engine::write_png(&dir.join(format!("{name}.png")), r.width, r.height, r.pixels.clone()).map_err(wrap)?;
            }
            Ok(RasterUd(r))
        })?,
    )?;

    let (e, d) = (eng.clone(), deps.clone());
    g.set(
        "outline",
        lua.create_function(move |lua, t: Table| {
            let at = caller(lua);
            let mut eng = e.borrow_mut();
            let node = Conv { eng: &eng, deps: &d }.node(t).map_err(lua_err)?;
            let Kind::Text(text) = node.kind else {
                return Err(lua_err(anyhow!("{at}: outline expects a Text {{ ... }} element")));
            };
            let path = eng.fonts.outline(&text).map_err(|err| lua_err(anyhow!("{at}: outline: {err:#}")))?;
            Ok(path.to_svg())
        })?,
    )?;

    // transformPath(d, t): path data moved through a placement, as absolute path data.
    // `t` is a matrix { a, b, c, d, e, f } or the keys a Group takes: x y rotate scale, or transform.
    g.set(
        "transformPath",
        lua.create_function(|lua, (d, t): (String, Value)| {
            let at = caller(lua);
            let fail = |err: anyhow::Error| lua_err(anyhow!("{at}: transformPath: {err:#}"));
            let path = BezPath::from_svg(&d).map_err(|err| fail(anyhow!("bad path data: {err}")))?;
            let Value::Table(t) = t else {
                return Err(fail(anyhow!("expected a matrix {{ a, b, c, d, e, f }} or {{ x, y, rotate, scale }}, got {}", type_name(&t))));
            };
            let m = if t.raw_len() > 0 {
                if t.pairs::<Value, Value>().any(|kv| kv.is_ok_and(|(k, _)| k.is_string())) {
                    return Err(fail(anyhow!("give a matrix {{ a, b, c, d, e, f }} or named keys {{ x, y, rotate, scale }}, not both")));
                }
                let p = Props { t: lua.create_table_from([("transform", t)])?, op: "transformPath".into(), at: at.clone(), known: RefCell::default() };
                placement(&p).map_err(lua_err)?
            } else {
                let p = Props { t, op: "transformPath".into(), at: at.clone(), known: RefCell::default() };
                let m = placement(&p).map_err(lua_err)?;
                p.finish().map_err(lua_err)?;
                m
            };
            Ok((Affine::new(m) * path).to_svg())
        })?,
    )?;

    // pathBounds(d) -> { x, y, w, h }: the tight bounds of path data, or nil when it's empty.
    g.set(
        "pathBounds",
        lua.create_function(|lua, d: String| {
            let path = BezPath::from_svg(&d).map_err(|err| lua_err(anyhow!("{}: pathBounds: bad path data: {err}", caller(lua))))?;
            if path.elements().is_empty() {
                return Ok(Value::Nil);
            }
            let r = path.bounding_box();
            let t = lua.create_table()?;
            t.raw_set("x", r.x0)?;
            t.raw_set("y", r.y0)?;
            t.raw_set("w", r.width())?;
            t.raw_set("h", r.height())?;
            Ok(Value::Table(t))
        })?,
    )?;

    let (e, d) = (eng.clone(), deps.clone());
    g.set(
        "measure",
        lua.create_function(move |lua, t: Table| {
            let at = caller(lua);
            let mut eng = e.borrow_mut();
            let node = Conv { eng: &eng, deps: &d }.node(t).map_err(lua_err)?;
            let Kind::Text(text) = node.kind else {
                return Err(lua_err(anyhow!("{at}: measure expects a Text {{ ... }} element")));
            };
            let m = eng.fonts.measure(&text).map_err(|err| lua_err(anyhow!("{at}: measure: {err:#}")))?;
            let rect = |r: Rect| -> mlua::Result<Table> {
                let t = lua.create_table()?;
                t.raw_set("x", r.x0)?;
                t.raw_set("y", r.y0)?;
                t.raw_set("w", r.width())?;
                t.raw_set("h", r.height())?;
                Ok(t)
            };
            let out = rect(m.advance)?;
            out.raw_set("baseline", m.baseline)?;
            out.raw_set("ascent", m.ascent)?;
            out.raw_set("descent", m.descent)?;
            let lines = lua.create_table()?;
            for l in &m.lines {
                let lt = lua.create_table()?;
                lt.raw_set("x", l.x)?;
                lt.raw_set("w", l.w)?;
                lt.raw_set("baseline", l.baseline)?;
                lines.raw_push(lt)?;
            }
            out.raw_set("lines", lines)?;
            if let Some(ink) = m.ink {
                out.raw_set("ink", rect(ink)?)?;
            }
            Ok(out)
        })?,
    )?;

    // require("lib/brushes") loads lib/brushes.luau next to the requiring file, once.
    let (e, d) = (eng.clone(), deps.clone());
    let loaded: Rc<RefCell<HashMap<PathBuf, mlua::RegistryKey>>> = Rc::default();
    g.set(
        "require",
        lua.create_function(move |lua, name: String| {
            let file = lua.inspect_stack(1, |d| chunk_file(&d.source()).unwrap_or("").to_string()).unwrap_or_default();
            let rel = if name.ends_with(".luau") || name.ends_with(".lua") { name.clone() } else { format!("{name}.luau") };
            let rel = from_file(&file, &rel).to_string_lossy().into_owned();
            let path = e.borrow().resolve(&rel).map_err(lua_err)?;
            if let Some(key) = loaded.borrow().get(&path) {
                return lua.registry_value::<Value>(key);
            }
            d.borrow_mut().insert(path.clone());
            let src = std::fs::read_to_string(&path).map_err(mlua::Error::external)?;
            let v: Value = lua.load(&src).set_name(format!("@{rel}")).eval()?;
            let v = if v.is_nil() { Value::Boolean(true) } else { v };
            loaded.borrow_mut().insert(path, lua.create_registry_value(v.clone())?);
            Ok(v)
        })?,
    )?;
    Ok(())
}

/// A raster as scripts see it.
#[derive(Clone)]
pub struct RasterUd(pub Rc<Raster>);

fn sample_table(lua: &Lua, r: &Raster, x: f64, y: f64, into: Option<Table>) -> mlua::Result<Table> {
    let s = r.at(x, y);
    let t = match into {
        Some(t) => t,
        None => lua.create_table()?,
    };
    t.raw_set("r", s.r)?;
    t.raw_set("g", s.g)?;
    t.raw_set("b", s.b)?;
    t.raw_set("a", s.a)?;
    t.raw_set("lum", 0.299 * s.r + 0.587 * s.g + 0.114 * s.b)?;
    Ok(t)
}

impl UserData for RasterUd {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("width", |_, this| Ok(this.0.width));
        f.add_field_method_get("height", |_, this| Ok(this.0.height));
        f.add_field_method_get("dpi", |_, this| Ok(this.0.dpi));
        f.add_field_method_get("size", |lua, this| lua.create_sequence_from(this.0.size));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // r:at(x, y) -> { r, g, b, a, lum }, bilinear, doc px.
        m.add_method("at", |lua, this, (x, y): (f64, f64)| sample_table(lua, &this.0, x, y, None));
        // r:sample(points) -> copies of each point with r, g, b, a, lum added.
        m.add_method("sample", |lua, this, pts: Table| {
            let out = lua.create_table_with_capacity(pts.raw_len(), 0)?;
            for p in pts.sequence_values::<Table>() {
                let p = p?;
                let copy = lua.create_table()?;
                for kv in p.pairs::<Value, Value>() {
                    let (k, v) = kv?;
                    copy.raw_set(k, v)?;
                }
                let (x, y): (f64, f64) = (p.get("x")?, p.get("y")?);
                out.raw_push(sample_table(lua, &this.0, x, y, Some(copy))?)?;
            }
            Ok(out)
        });
    }
}

fn parse_effect(t: &Table) -> Result<EffectDef> {
    let wgsl: String = t.get::<Option<String>>("wgsl")?.ok_or_else(|| anyhow!("effect needs `wgsl`"))?;
    let name = match t.get::<Option<String>>("name")? {
        Some(n) => n,
        None => format!("fx_{}", &blake3::hash(wgsl.as_bytes()).to_hex()[..8]),
    };
    let mut uniforms = Vec::new();
    if let Some(us) = t.get::<Option<Table>>("uniforms")? {
        for kv in us.pairs::<String, Value>() {
            let (k, spec) = kv?;
            if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.starts_with(|c: char| c.is_ascii_digit()) {
                bail!("uniform name {k:?} is not a WGSL identifier");
            }
            if ["opacity", "blend", "blur", "id", "area"].contains(&k.as_str()) {
                bail!("uniform name {k:?} is taken by the prop every effect has (opacity, blend, blur, id, area)");
            }
            let Value::Table(spec) = spec else { bail!("uniform {k:?}: use {{ \"type\", default }}, e.g. {{ \"length\", 4 }}") };
            let ty = UType::parse(&spec.get::<String>(1).with_context(|| format!("uniform {k:?} type"))?)?;
            let dflt = uvalue(ty, &spec.get::<Value>(2)?).with_context(|| format!("uniform {k:?} default"))?;
            uniforms.push((k, ty, dflt));
        }
    }
    let mut inputs = Vec::new();
    if let Some(is) = t.get::<Option<Table>>("inputs")? {
        for kv in is.pairs::<String, Value>() {
            let (k, kind) = kv?;
            if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.starts_with(|c: char| c.is_ascii_digit()) {
                bail!("input name {k:?} is not a WGSL identifier");
            }
            if ["opacity", "blend", "blur", "id", "area"].contains(&k.as_str()) {
                bail!("input name {k:?} is taken by the prop every effect has (opacity, blend, blur, id, area)");
            }
            let Value::String(kind) = kind else { bail!("input {k:?}: give its kind as a string, e.g. {k} = \"sdf\"") };
            inputs.push((k.clone(), InputKind::parse(&kind.to_str()?).with_context(|| format!("input {k:?}"))?));
        }
    }
    if !t.get::<Value>("pad")?.is_nil() {
        bail!(
            "effect: `pad` is now two fields: `reach` (how far beyond what it draws the shader reads src and inputs) \
             and `grow` (how far beyond its children it draws); `pad = x` was `reach = x, grow = x`"
        );
    }
    let margin = |k: &str| -> Result<Margin> {
        let m = match t.get::<Value>(k)? {
            Value::Nil => Margin::Fixed(0.0),
            Value::String(s) if uniforms.iter().any(|(n, ..)| *n == s.to_string_lossy()) => Margin::Uniform(s.to_string_lossy()),
            v => Margin::Fixed(length(&v).with_context(|| format!("`{k}`: a length, or the name of a length uniform"))?),
        };
        if let Margin::Fixed(v) = m
            && v < 0.0
        {
            bail!("effect: `{k}` can't be negative, got {v}");
        }
        Ok(m)
    };
    let (reach, grow) = (margin("reach")?, margin("grow")?);
    for k in t.pairs::<String, Value>() {
        let k = k?.0;
        if !["name", "uniforms", "inputs", "reach", "grow", "wgsl"].contains(&k.as_str()) {
            bail!("effect: unknown field {k:?} (fields: name, uniforms, inputs, reach, grow, wgsl)");
        }
    }
    EffectDef::new(name, uniforms, inputs, reach, grow, wgsl)
}

fn uvalue(ty: UType, v: &Value) -> Result<UValue> {
    Ok(match ty {
        UType::Float => UValue::Num(number(v)?),
        UType::Length => UValue::Num(length(v)?),
        UType::Angle => UValue::Num(number(v)?.to_radians()),
        UType::Color => UValue::Color(color(v)?),
    })
}

fn type_name(v: &Value) -> &'static str {
    v.type_name()
}

fn number(v: &Value) -> Result<f64> {
    match v {
        Value::Integer(i) => Ok(*i as f64),
        Value::Number(n) => Ok(*n),
        Value::String(s) => s.to_str()?.trim().parse().map_err(|_| anyhow!("expected a number, got {:?}", s.to_string_lossy())),
        _ => bail!("expected a number, got {}", type_name(v)),
    }
}

fn length(v: &Value) -> Result<f64> {
    match v {
        Value::String(s) => parse_length(&s.to_str()?),
        _ => number(v).map_err(|_| anyhow!("expected a length (number or e.g. \"3mm\"), got {}", type_name(v))),
    }
}

/// A canvas `margin`: a length, or `math.huge` for no limit.
fn margin(v: &Value) -> Result<f64> {
    let m = length(v)?;
    if m < 0.0 {
        bail!("can't be negative, got {m}");
    }
    Ok(m)
}

fn color(v: &Value) -> Result<Rgba> {
    match v {
        Value::String(s) => parse_color(&s.to_str()?),
        _ => bail!("expected a color string, got {}", type_name(v)),
    }
}

fn point(v: &Value) -> Result<[f64; 2]> {
    let Value::Table(t) = v else { bail!("expected {{x, y}}, got {}", type_name(v)) };
    Ok([length(&t.raw_get(1)?)?, length(&t.raw_get(2)?)?])
}

/// Where a `Group` or `Path` sits: `x y rotate scale` (translate · rotate · scale), or
/// an explicit `transform = { a, b, c, d, e, f }` in SVG `matrix()` order, never both.
fn placement(p: &Props) -> Result<[f64; 6]> {
    let matrix = p.with("transform", |v| {
        let Value::Table(t) = v else { bail!("expected {{ a, b, c, d, e, f }}, got {}", type_name(v)) };
        let items: Vec<Value> = t.sequence_values::<Value>().collect::<mlua::Result<_>>()?;
        if items.len() != 6 {
            bail!("expected 6 values {{ a, b, c, d, e, f }} (x' = a x + c y + e, y' = b x + d y + f), got {}", items.len());
        }
        let mut m = [0.0; 6];
        for (i, v) in items.iter().enumerate() {
            // a..d are unitless; e, f are a translation, so they take lengths
            m[i] = if i < 4 { number(v)? } else { length(v)? };
        }
        if m.iter().any(|v| !v.is_finite()) {
            bail!("values must be finite, got {m:?}");
        }
        Ok(m)
    })?;
    if let Some(m) = matrix {
        for k in ["x", "y", "rotate", "scale"] {
            if !p.t.raw_get::<Value>(k)?.is_nil() {
                return Err(p.err(format!("give either `transform` or `{k}`, not both (fold `{k}` into the matrix)")));
            }
        }
        return Ok(m);
    }
    let (x, y) = (p.len_or("x", 0.0)?, p.len_or("y", 0.0)?);
    let rotate = p.num("rotate")?.unwrap_or(0.0);
    let (sx, sy) = p
        .with("scale", |v| match v {
            Value::Table(_) => Ok(point(v).map(|[a, b]| (a, b))?),
            _ => number(v).map(|s| (s, s)),
        })?
        .unwrap_or((1.0, 1.0));
    Ok((Affine::translate((x, y)) * Affine::rotate(rotate.to_radians()) * Affine::scale_non_uniform(sx, sy)).as_coeffs())
}

fn rect4(v: &Value) -> Result<[f64; 4]> {
    let Value::Table(t) = v else { bail!("expected {{x, y, w, h}}, got {}", type_name(v)) };
    Ok([length(&t.raw_get(1)?)?, length(&t.raw_get(2)?)?, length(&t.raw_get(3)?)?, length(&t.raw_get(4)?)?])
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = (prev + (ca != b[j]) as usize).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Reads props off an element table, remembering which were asked for so
/// `finish` can reject the rest (typos are errors, not silent no-ops).
struct Props {
    t: Table,
    op: String,
    at: String,
    known: RefCell<Vec<&'static str>>,
}

impl Props {
    fn err(&self, msg: impl std::fmt::Display) -> anyhow::Error {
        anyhow!("{}: {}: {msg}", self.at, self.op)
    }

    fn raw(&self, k: &'static str) -> Result<Option<Value>> {
        self.known.borrow_mut().push(k);
        let v: Value = self.t.raw_get(k)?;
        Ok(if v.is_nil() { None } else { Some(v) })
    }

    fn with<T>(&self, k: &'static str, f: impl FnOnce(&Value) -> Result<T>) -> Result<Option<T>> {
        match self.raw(k)? {
            Some(v) => f(&v).map(Some).map_err(|e| self.err(format!("{k}: {e:#}"))),
            None => Ok(None),
        }
    }

    fn len(&self, k: &'static str) -> Result<Option<f64>> {
        self.with(k, length)
    }
    fn len_or(&self, k: &'static str, d: f64) -> Result<f64> {
        Ok(self.len(k)?.unwrap_or(d))
    }
    fn req_len(&self, k: &'static str) -> Result<f64> {
        self.len(k)?.ok_or_else(|| self.err(format!("missing `{k}`")))
    }
    fn num(&self, k: &'static str) -> Result<Option<f64>> {
        self.with(k, number)
    }
    fn string(&self, k: &'static str) -> Result<Option<String>> {
        self.with(k, |v| match v {
            Value::String(s) => Ok(s.to_str()?.to_string()),
            _ => bail!("expected a string, got {}", type_name(v)),
        })
    }
    fn boolean(&self, k: &'static str) -> Result<bool> {
        Ok(self.with(k, |v| v.as_boolean().ok_or_else(|| anyhow!("expected true/false")))?.unwrap_or(false))
    }
    fn choice<T: Copy>(&self, k: &'static str, options: &[(&str, T)], d: T) -> Result<T> {
        Ok(self
            .with(k, |v| {
                let s = v.as_string().map(|s| s.to_string_lossy()).unwrap_or_default();
                options.iter().find(|(n, _)| *n == s).map(|(_, t)| *t).ok_or_else(|| {
                    anyhow!("{s:?} is not one of {}", options.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))
                })
            })?
            .unwrap_or(d))
    }

    /// A fill or stroke paint: a color string, "none", or linear(...)/radial(...).
    fn paint(&self, k: &'static str) -> Result<Option<Option<Paint>>> {
        self.with(k, |v| match v {
            Value::String(s) if &*s.to_str()? == "none" => Ok(None),
            Value::String(_) => Ok(Some(Paint::Solid { color: color(v)? })),
            Value::Table(t) => {
                let stops = gradient_stops(&t.get::<Value>("stops")?)?;
                match t.get::<Option<String>>("_grad")?.as_deref() {
                    Some("linear") => Ok(Some(Paint::Linear { from: point(&t.get("from")?)?, to: point(&t.get("to")?)?, stops })),
                    Some("radial") => Ok(Some(Paint::Radial { center: point(&t.get("center")?)?, r: length(&t.get("r")?)?, stops })),
                    _ => bail!("expected a color or linear(...)/radial(...)"),
                }
            }
            _ => bail!("expected a color or gradient, got {}", type_name(v)),
        })
    }

    fn common(&self) -> Result<(f32, Blend, f64)> {
        let opacity = self.num("opacity")?.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&opacity) {
            return Err(self.err(format!("`opacity` must be between 0 and 1, got {opacity}")));
        }
        let blend = self.choice("blend", Blend::NAMES, Blend::Normal)?;
        let blur = self.len_or("blur", 0.0)?;
        if blur < 0.0 {
            return Err(self.err(format!("`blur` can't be negative, got {blur}")));
        }
        let opacity = opacity as f32;
        Ok((opacity, blend, blur))
    }

    /// Array-part entries in order (nil holes skipped).
    fn children(&self) -> Result<Vec<Value>> {
        let mut items: Vec<(i64, Value)> = Vec::new();
        for kv in self.t.pairs::<Value, Value>() {
            let (k, v) = kv?;
            if let Some(i) = k.as_integer().or_else(|| k.as_number().filter(|n| n.fract() == 0.0).map(|n| n as i64)) {
                items.push((i, v));
            }
        }
        items.sort_by_key(|(i, _)| *i);
        Ok(items.into_iter().map(|(_, v)| v).collect())
    }

    fn finish(&self) -> Result<()> {
        let known = self.known.borrow();
        for kv in self.t.pairs::<Value, Value>() {
            let (k, _) = kv?;
            let Value::String(k) = k else { continue };
            let k = k.to_string_lossy();
            if k.starts_with('_') || known.contains(&k.as_str()) {
                continue;
            }
            let best = known.iter().min_by_key(|c| edit_distance(&k, c)).filter(|c| edit_distance(&k, c) <= 2);
            return Err(self.err(match best {
                Some(b) => format!("unknown prop `{k}` (did you mean `{b}`?)"),
                None => format!("unknown prop `{k}` (props: {})", known.join(", ")),
            }));
        }
        Ok(())
    }
}

fn gradient_stops(v: &Value) -> Result<Vec<(f32, Rgba)>> {
    let Value::Table(t) = v else { bail!("gradient stops must be a list") };
    let items: Vec<Value> = t.sequence_values::<Value>().collect::<mlua::Result<_>>()?;
    let n = items.len().max(2) - 1;
    items
        .iter()
        .enumerate()
        .map(|(i, s)| match s {
            Value::Table(pair) => Ok((number(&pair.raw_get(1)?)? as f32, color(&pair.raw_get(2)?)?)),
            _ => Ok((i as f32 / n as f32, color(s)?)),
        })
        .collect()
}

/// A path through `pts`: straight segments, or with `smooth` a uniform
/// Catmull-Rom spline (the curve passes through every point). Open smooth
/// paths repeat their end points so the ends have tangents too.
fn through(pts: &[[f64; 2]], closed: bool, smooth: bool) -> BezPath {
    let mut path = BezPath::new();
    let n = pts.len();
    path.move_to((pts[0][0], pts[0][1]));
    let segments = if closed { n } else { n - 1 };
    let at = |i: isize| -> [f64; 2] {
        if closed { pts[i.rem_euclid(n as isize) as usize] } else { pts[i.clamp(0, n as isize - 1) as usize] }
    };
    for i in 0..segments as isize {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        if smooth {
            path.curve_to(
                (p1[0] + (p2[0] - p0[0]) / 6.0, p1[1] + (p2[1] - p0[1]) / 6.0),
                (p2[0] - (p3[0] - p1[0]) / 6.0, p2[1] - (p3[1] - p1[1]) / 6.0),
                (p2[0], p2[1]),
            );
        } else if !(closed && i == segments as isize - 1) {
            path.line_to((p2[0], p2[1]));
        }
    }
    if closed {
        path.close_path();
    }
    path
}

struct Conv<'a> {
    eng: &'a Engine,
    deps: &'a Deps,
}

impl Conv<'_> {
    fn document(&self, root: Value) -> Result<Document> {
        let Value::Table(t) = root else {
            bail!("the script must return a Canvas {{ ... }} (or a function returning one), got {}", type_name(&root))
        };
        let op: Option<String> = t.raw_get("_op")?;
        if op.as_deref() != Some("Canvas") {
            bail!("the script must return a Canvas {{ ... }}, got {}", op.unwrap_or("a plain table".into()));
        }
        let p = Props { at: t.raw_get::<Option<String>>("_at")?.unwrap_or_default(), op: "Canvas".into(), t, known: RefCell::default() };
        let size = p.with("size", point)?.ok_or_else(|| p.err("missing `size = {w, h}`"))?;
        let bg = p.with("bg", color)?.unwrap_or([1.0; 4]);
        let margin = p.with("margin", margin)?;
        let mut children = Vec::new();
        for c in p.children()? {
            self.nodes(c, &mut children)?;
        }
        p.finish()?;
        Ok(Document { size, bg, margin, children })
    }

    /// Converts an element, or a list of them (nested lists flatten; nil/false are skipped).
    fn nodes(&self, v: Value, out: &mut Vec<Node>) -> Result<()> {
        match v {
            Value::Nil | Value::Boolean(false) => Ok(()),
            Value::Table(t) => {
                if t.raw_get::<Value>("_op")?.is_nil() {
                    let p = Props { t, op: "list".into(), at: String::new(), known: RefCell::default() };
                    for c in p.children()? {
                        self.nodes(c, out)?;
                    }
                    Ok(())
                } else {
                    out.push(self.node(t)?);
                    Ok(())
                }
            }
            Value::String(s) => bail!("stray text {:?}: text must be inside Text {{ ... }}", s.to_string_lossy()),
            other => bail!("expected an element, got {}", type_name(&other)),
        }
    }

    fn child_nodes(&self, p: &Props) -> Result<Vec<Node>> {
        let mut out = Vec::new();
        for c in p.children()? {
            self.nodes(c, &mut out).map_err(|e| if e.to_string().contains(".luau:") { e } else { p.err(e) })?;
        }
        Ok(out)
    }

    fn style(&self, p: &Props, default_fill: bool) -> Result<(Option<Paint>, Option<Stroke>)> {
        let stroke_paint = p.paint("stroke")?.flatten();
        let width = p.len_or("strokeWidth", 1.0)?;
        let cap = p.choice("cap", &[("butt", Cap::Butt), ("round", Cap::Round), ("square", Cap::Square)], Cap::Butt)?;
        let join = p.choice("join", &[("miter", Join::Miter), ("round", Join::Round), ("bevel", Join::Bevel)], Join::Round)?;
        let dash = p
            .with("dash", |v| match v {
                Value::Table(t) => t.sequence_values::<Value>().map(|x| length(&x?)).collect(),
                _ => bail!("expected a list of lengths"),
            })?
            .unwrap_or_default();
        let fill = match p.paint("fill")? {
            Some(f) => f,
            None if default_fill => Some(Paint::Solid { color: [0.0, 0.0, 0.0, 1.0] }),
            None => None,
        };
        let stroke = stroke_paint.map(|paint| Stroke { paint, width, cap, join, dash });
        Ok((fill, stroke))
    }

    /// A path prop, resolved from the file whose code made the element.
    fn file(&self, p: &Props, rel: &str) -> Result<PathBuf> {
        let file = p.at.rsplit_once(':').map_or("", |(f, _)| f);
        let path = self.eng.resolve(&from_file(file, rel).to_string_lossy()).map_err(|e| p.err(e))?;
        self.deps.borrow_mut().insert(path.clone());
        Ok(path)
    }

    /// An effect's input slot: exactly the element its kind takes, never converted.
    fn input(&self, p: &Props, effect: &str, input: &str, kind: InputKind) -> Result<Node> {
        let want = kind.element();
        let hint = format!("wrap it: {input} = {want} {{ range = ..., ... }}");
        let t = match p.t.raw_get::<Option<Value>>(input)? {
            None => return Err(p.err(format!("{effect}: missing input `{input}` (an {want} {{ ... }})"))),
            Some(Value::Table(t)) => t,
            Some(v) => return Err(p.err(format!("{effect}: input `{input}` expects an {want}, got {} ({hint})", type_name(&v)))),
        };
        match t.raw_get::<Option<String>>("_op")? {
            Some(op) if op == want => {}
            Some(op) => return Err(p.err(format!("{effect}: input `{input}` expects an {want}, got {op} ({hint})"))),
            None => return Err(p.err(format!("{effect}: input `{input}` expects an {want}, got a list ({hint})"))),
        }
        let at: String = t.raw_get::<Option<String>>("_at")?.unwrap_or_default();
        for k in ["fill", "opacity", "blend"] {
            if !t.raw_get::<Value>(k)?.is_nil() {
                bail!("{at}: {want}: `{k}` has no effect on an effect input (it is read as data, not drawn); remove it");
            }
        }
        if !t.raw_get::<Value>("blur")?.is_nil() {
            bail!("{at}: {want}: `blur` on an effect input is not supported yet");
        }
        self.node(t)
    }

    fn node(&self, t: Table) -> Result<Node> {
        let op: String = t.raw_get("_op")?;
        let at: String = t.raw_get::<Option<String>>("_at")?.unwrap_or_default();
        let p = Props { t, op: op.clone(), at: at.clone(), known: RefCell::default() };
        let (opacity, blend, blur) = p.common()?;
        p.raw("id")?; // free-form label, kept out of the IR
        let kind = match op.as_str() {
            "Group" => {
                let transform = placement(&p)?;
                let clip = p.with("clip", rect4)?;
                Kind::Group { transform, clip, children: self.child_nodes(&p)? }
            }
            "Mask" => {
                let shape_v = p.raw("shape")?.ok_or_else(|| p.err("missing `shape`"))?;
                let mut shape = Vec::new();
                self.nodes(shape_v, &mut shape).map_err(|e| p.err(format!("shape: {e:#}")))?;
                Kind::Mask {
                    shape,
                    invert: p.boolean("invert")?,
                    luminance: p.boolean("luminance")?,
                    children: self.child_nodes(&p)?,
                }
            }
            "Rect" | "Circle" | "Ellipse" | "Line" | "Path" => {
                let geom = match op.as_str() {
                    "Rect" => Geom::Rect {
                        x: p.len_or("x", 0.0)?,
                        y: p.len_or("y", 0.0)?,
                        w: p.req_len("w")?,
                        h: p.req_len("h")?,
                        r: p.len_or("r", 0.0)?,
                    },
                    "Circle" => Geom::Circle { cx: p.len_or("cx", 0.0)?, cy: p.len_or("cy", 0.0)?, r: p.req_len("r")? },
                    "Ellipse" => Geom::Ellipse {
                        cx: p.len_or("cx", 0.0)?,
                        cy: p.len_or("cy", 0.0)?,
                        rx: p.req_len("rx")?,
                        ry: p.req_len("ry")?,
                        rotate: p.num("rotate")?.unwrap_or(0.0),
                    },
                    "Line" => Geom::Line {
                        x1: p.req_len("x1")?,
                        y1: p.req_len("y1")?,
                        x2: p.req_len("x2")?,
                        y2: p.req_len("y2")?,
                    },
                    _ => {
                        let pts = p.with("points", |v| match v {
                            Value::Table(t) => t.sequence_values::<Value>().map(|q| point(&q?)).collect::<Result<Vec<_>>>(),
                            _ => bail!("expected a list of {{x, y}} points"),
                        })?;
                        let (closed, smooth) = (p.boolean("closed")?, p.boolean("smooth")?);
                        let d = match (p.string("d")?, pts) {
                            (Some(_), Some(_)) => return Err(p.err("give either `d` or `points`, not both")),
                            (Some(d), None) => {
                                if closed || smooth {
                                    return Err(p.err("`closed` and `smooth` apply to `points`, not `d`"));
                                }
                                d
                            }
                            (None, Some(pts)) if pts.len() >= 2 => through(&pts, closed, smooth).to_svg(),
                            (None, Some(_)) => return Err(p.err("`points` needs at least 2 points")),
                            (None, None) => return Err(p.err("missing `d` (SVG path data) or `points`")),
                        };
                        BezPath::from_svg(&d).map_err(|e| p.err(format!("bad path data: {e}")))?;
                        Geom::Path { d, transform: placement(&p)? }
                    }
                };
                for k in ["fill", "fillRule"] {
                    if op == "Line" && p.t.raw_get::<Value>(k)? != Value::Nil {
                        return Err(p.err(format!("a Line has no area to fill, so no `{k}`; give it a `stroke`")));
                    }
                }
                let even_odd = p.choice("fillRule", &[("nonzero", false), ("evenodd", true)], false)?;
                let (fill, mut stroke) = self.style(&p, op != "Line")?;
                if op == "Line" {
                    stroke.get_or_insert(Stroke {
                        paint: Paint::Solid { color: [0.0, 0.0, 0.0, 1.0] },
                        width: p.len_or("strokeWidth", 1.0)?,
                        cap: Cap::Butt,
                        join: Join::Round,
                        dash: vec![],
                    });
                }
                Kind::Shape { geom, fill, stroke, even_odd }
            }
            "Text" => {
                let mut text = String::new();
                for c in p.children()? {
                    match c {
                        Value::String(s) => text.push_str(&s.to_str()?),
                        Value::Integer(_) | Value::Number(_) => text.push_str(&number(&c)?.to_string()),
                        Value::Nil | Value::Boolean(false) => {}
                        other => return Err(p.err(format!("Text children must be strings, got {}", type_name(&other)))),
                    }
                }
                let font = match (p.string("font")?, p.string("fontFile")?) {
                    (Some(_), Some(_)) => return Err(p.err("give either `font` (a family) or `fontFile` (a path), not both")),
                    (None, Some(file)) => Font::File(self.file(&p, &file)?.to_string_lossy().into()),
                    (Some(f), None) if f.contains('/') || [".ttf", ".otf", ".ttc"].iter().any(|e| f.to_lowercase().ends_with(e)) => {
                        return Err(p.err(format!("`font` is a family name; for a font file use fontFile = {f:?}")));
                    }
                    (family, None) => Font::Family(family.unwrap_or_else(|| "Helvetica".into())),
                };
                let w = p.len("w")?;
                let align = p.choice(
                    "align",
                    &[("left", Align::Left), ("center", Align::Center), ("right", Align::Right), ("justify", Align::Justify)],
                    Align::Left,
                )?;
                let anchor = p.choice("anchor", &[("start", Anchor::Start), ("middle", Anchor::Middle), ("end", Anchor::End)], Anchor::Start)?;
                match w {
                    None if p.t.raw_get::<Value>("align")? != Value::Nil => {
                        return Err(p.err("`align` places lines inside a box: set `w`, or use `anchor` (start, middle, end) to place unboxed text at `x`"));
                    }
                    Some(_) if p.t.raw_get::<Value>("anchor")? != Value::Nil => {
                        return Err(p.err("`anchor` places unboxed text at `x`; with `w`, use `align` inside the box"));
                    }
                    _ => {}
                }
                let tags = |k: &'static str| -> Result<Vec<(String, f64)>> {
                    Ok(p.with(k, |v| {
                        let Value::Table(t) = v else { bail!("expected a table like {{ liga = 0 }}") };
                        let mut out: Vec<(String, f64)> =
                            t.pairs::<String, Value>().map(|kv| { let (k, v) = kv?; Ok((k, number(&v)?)) }).collect::<Result<_>>()?;
                        out.sort_by(|a, b| a.0.cmp(&b.0));
                        Ok(out)
                    })?
                    .unwrap_or_default())
                };
                Kind::Text(Text {
                    text,
                    x: p.len_or("x", 0.0)?,
                    y: p.len_or("y", 0.0)?,
                    w,
                    font,
                    size: p.len_or("size", 16.0)?,
                    fill: p.paint("fill")?.flatten().unwrap_or(Paint::Solid { color: [0.0, 0.0, 0.0, 1.0] }),
                    align,
                    anchor,
                    leading: p.num("leading")?.map(|v| v as f32),
                    tracking: p.num("tracking")?.unwrap_or(0.0) as f32,
                    weight: p.num("weight")?.map(|v| v as f32),
                    italic: p.boolean("italic")?,
                    features: tags("features")?.into_iter().map(|(k, v)| (k, v as u16)).collect(),
                    variations: tags("variations")?.into_iter().map(|(k, v)| (k, v as f32)).collect(),
                })
            }
            "Image" => {
                let srcv = p.raw("src")?.ok_or_else(|| p.err("missing `src` (a file path or a raster)"))?;
                let (src, natural) = match &srcv {
                    Value::String(s) => {
                        let path = self.file(&p, &s.to_str()?)?;
                        let (w, h) = self.eng.image_size(&path).map_err(|e| p.err(e))?;
                        (ImageSrc::File(path.to_string_lossy().into()), [w as f64, h as f64])
                    }
                    Value::UserData(ud) => {
                        let r = ud.borrow::<RasterUd>().map_err(|_| p.err("src must be a file path or a raster"))?;
                        (ImageSrc::Raster(r.0.key.clone()), r.0.size)
                    }
                    _ => return Err(p.err("src must be a file path or a raster")),
                };
                let crop = p.with("crop", rect4)?.unwrap_or([0.0, 0.0, natural[0], natural[1]]);
                let (w, h) = (p.len("w")?, p.len("h")?);
                let (w, h) = match (w, h) {
                    (Some(w), Some(h)) => (w, h),
                    (Some(w), None) => (w, w * crop[3] / crop[2]),
                    (None, Some(h)) => (h * crop[2] / crop[3], h),
                    (None, None) => (crop[2], crop[3]),
                };
                Kind::Image {
                    src,
                    dst: [p.len_or("x", 0.0)?, p.len_or("y", 0.0)?, w, h],
                    crop,
                    fit: p.choice("fit", &[("fill", Fit::Fill), ("cover", Fit::Cover), ("contain", Fit::Contain)], Fit::Fill)?,
                }
            }
            "Effect" => {
                let name: String = p.t.raw_get("_effect")?;
                let def = self.eng.effects.get(&name)?;
                let mut uniforms = Vec::new();
                for (u, ty, _) in &def.uniforms {
                    let v: Value = p.t.raw_get(u.as_str())?;
                    if !v.is_nil() {
                        uniforms.push((u.clone(), uvalue(*ty, &v).map_err(|e| p.err(format!("{u}: {e:#}")))?));
                    }
                }
                let def_inputs = def.inputs.clone();
                let mut names: Vec<String> = def.uniforms.iter().map(|(n, ..)| n.clone()).collect();
                names.extend(def_inputs.iter().map(|(n, _)| n.clone()));
                let mut inputs = Vec::new();
                for (input, kind) in &def_inputs {
                    inputs.push((input.clone(), self.input(&p, &name, input, *kind)?));
                }
                for (field, m) in [("reach", &def.reach), ("grow", &def.grow)] {
                    if let crate::effects::Margin::Uniform(u) = m
                        && let Some((_, UValue::Num(v))) = uniforms.iter().find(|(n, _)| n == u)
                        && *v < 0.0
                    {
                        return Err(p.err(format!("{name}: `{u}` is this effect's {field} and can't be negative, got {v}")));
                    }
                }
                let area = p.with("area", rect4)?;
                let children = self.child_nodes(&p)?;
                // Effect props are the effect's uniforms plus the common ones.
                for kv in p.t.pairs::<Value, Value>() {
                    let Value::String(k) = kv?.0 else { continue };
                    let k = k.to_string_lossy();
                    if !k.starts_with('_') && !names.contains(&k) && !p.known.borrow().contains(&k.as_str()) {
                        let best = names.iter().min_by_key(|c| edit_distance(&k, c)).filter(|c| edit_distance(&k, c) <= 2);
                        let hint = match best {
                            Some(b) => format!("did you mean `{b}`?"),
                            None => format!("uniforms and inputs: {}; also opacity, blend, blur, area", names.join(", ")),
                        };
                        return Err(anyhow!("{at}: {name}: unknown prop `{k}` ({hint})"));
                    }
                }
                return Ok(Node { kind: Kind::Effect { effect: name, uniforms, area, inputs, children }, opacity, blend, blur, at });
            }
            "Sdf" => {
                let range = p.len_or("range", 32.0)?;
                if range <= 0.0 {
                    return Err(p.err("`range` must be positive"));
                }
                Kind::Sdf { range, fill: p.with("fill", color)?.unwrap_or([1.0; 4]), children: self.child_nodes(&p)? }
            }
            "Canvas" => return Err(p.err("Canvas can only be the root (use rasterize() to nest a scene)")),
            other => return Err(p.err(format!("unknown element {other:?}"))),
        };
        p.finish()?;
        Ok(Node { kind, opacity, blend, blur, at })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello::kurbo::PathEl;

    fn ends(path: &BezPath) -> Vec<(f64, f64)> {
        path.elements()
            .iter()
            .filter_map(|e| match e {
                PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::CurveTo(_, _, p) => Some((p.x, p.y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn effect_uniforms_cannot_shadow_common_props() {
        let lua = Lua::new();
        let t: Table = lua.load(r#"{ uniforms = { area = { "float", 1 } }, wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }"#).eval().unwrap();
        let err = parse_effect(&t).unwrap_err().to_string();
        assert!(err.contains("\"area\" is taken"), "{err}");
    }

    #[test]
    fn effect_inputs_are_checked_at_definition() {
        let lua = Lua::new();
        let def = |src: &str| parse_effect(&lua.load(src).eval::<Table>().unwrap());
        let ok = def(r#"{ inputs = { body = "sdf" }, wgsl = "fn effect(p: vec2f) -> vec4f { let f = body(p); return vec4f(f.d, f.n, body_march(p, vec2f(0.0, 1.0), 9.0)); }" }"#);
        assert!(ok.is_ok_and(|d| d.inputs.len() == 1));
        let err = |src: &str| format!("{:#}", def(src).unwrap_err());
        assert!(err(r#"{ inputs = { body = "blur" }, wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }"#).contains("unknown input kind"));
        assert!(err(r#"{ inputs = { blur = "sdf" }, wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }"#).contains("\"blur\" is taken"));
        assert!(err(r#"{ inputs = { hash = "sdf" }, wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }"#).contains("taken by the prelude"));
        assert!(err(r#"{ uniforms = { k = { "float", 1 } }, inputs = { k = "sdf" }, wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }"#).contains("both a uniform and an input"));
    }

    #[test]
    fn through_passes_every_point() {
        let pts = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let closed = through(&pts, true, true);
        assert_eq!(ends(&closed), vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)]);
        assert!(matches!(closed.elements().last(), Some(PathEl::ClosePath)));
        let open = through(&pts, false, true);
        assert_eq!(ends(&open).len(), 4);
        let poly = through(&pts, true, false);
        assert_eq!(ends(&poly), vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    }
}
