//! Shape warps: nonlinear maps applied to vector geometry.
//!
//! A [`Patch`] maps the unit square (u right, v down) onto a curved region:
//! a Coons patch between four boundary curves, a bicubic Bezier patch, or a
//! band along a path. A script's `Warp { map, from }` (or `warpPath`) sends
//! its `from` rect onto that square. A Lua function `(x, y) -> (x, y)` is a
//! map too, over doc px directly.
//!
//! Paths are warped segment by segment into cubics: each output piece is the
//! cubic Hermite through the mapped end points and mapped tangents, split
//! until it stays within `tolerance` of the true mapped curve. The result is
//! plain path data, so warps resolve before rendering.

use anyhow::{Result, anyhow, bail};
use vello::kurbo::{self, Affine, BezPath, CubicBez, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, PathEl, PathSeg, Point, Vec2};

/// Something that moves points: the input is in the warp's local doc px.
pub trait Map {
    /// The mapped point, or an error for a point outside the map's domain.
    fn at(&mut self, p: Point) -> Result<Point>;
    /// The derivative along `d` (J·d), from a central difference. Probes
    /// may step just past the domain, so they skip its check.
    fn deriv(&mut self, p: Point, d: Vec2) -> Result<Vec2>;
    /// Skip the domain check (for a stroke's outline, whose centerline was checked).
    fn lenient(&mut self, _on: bool) {}
}

/// A point in the unit square -> doc px.
#[derive(Debug)]
pub enum Patch {
    Coons { top: Edge, right: Edge, bottom: Edge, left: Edge, corners: [Point; 4] },
    Bezier { p: [[Point; 4]; 4] },
    Along { path: Edge, width: f64 },
}

impl Patch {
    /// Evaluates anywhere: outside the unit square, edges extend along their end tangents.
    pub fn at(&self, u: f64, v: f64) -> Point {
        match self {
            Patch::Coons { top, right, bottom, left, corners: [tl, tr, br, bl] } => {
                let (t, b, l, r) = (top.at(u).to_vec2(), bottom.at(u).to_vec2(), left.at(v).to_vec2(), right.at(v).to_vec2());
                let ruled = (1.0 - v) * t + v * b + (1.0 - u) * l + u * r;
                let bilinear =
                    (1.0 - u) * (1.0 - v) * tl.to_vec2() + u * (1.0 - v) * tr.to_vec2() + u * v * br.to_vec2() + (1.0 - u) * v * bl.to_vec2();
                (ruled - bilinear).to_point()
            }
            Patch::Bezier { p } => {
                let bu = bernstein(u);
                let bv = bernstein(v);
                let mut out = Vec2::ZERO;
                for (j, row) in p.iter().enumerate() {
                    for (i, q) in row.iter().enumerate() {
                        out += bu[i] * bv[j] * q.to_vec2();
                    }
                }
                out.to_point()
            }
            Patch::Along { path, width } => {
                let c = path.at(u);
                let t = path.tangent(u);
                // the left of travel (y down), so v = 0 is on the left: "up" for a path going right
                let n = Vec2::new(t.y, -t.x);
                c + n * ((0.5 - v) * width)
            }
        }
    }

    /// (P, dP/du, dP/dv) by central differences.
    pub fn frame(&self, u: f64, v: f64) -> (Point, Vec2, Vec2) {
        const H: f64 = 1e-5;
        let du = (self.at(u + H, v) - self.at(u - H, v)) / (2.0 * H);
        let dv = (self.at(u, v + H) - self.at(u, v - H)) / (2.0 * H);
        (self.at(u, v), du, dv)
    }

    /// A Coons patch from four edges, each running TL->TR (top), TR->BR (right),
    /// BL->BR (bottom), TL->BL (left). Their ends must meet at the corners.
    pub fn coons(top: Edge, right: Edge, bottom: Edge, left: Edge) -> Result<Patch> {
        let tl = top.start();
        let tr = top.end();
        let bl = bottom.start();
        let br = bottom.end();
        let scale = [tl, tr, bl, br].iter().map(|p| p.to_vec2().hypot()).fold(1.0, f64::max);
        let meet = |a: Point, b: Point, what: &str| -> Result<()> {
            if (a - b).hypot() > 1e-6 * scale + 1e-3 {
                bail!("the {what} corner doesn't meet: ({:.3}, {:.3}) vs ({:.3}, {:.3})", a.x, a.y, b.x, b.y);
            }
            Ok(())
        };
        meet(tl, left.start(), "top-left (top's start, left's start)")?;
        meet(tr, right.start(), "top-right (top's end, right's start)")?;
        meet(br, right.end(), "bottom-right (bottom's end, right's end)")?;
        meet(bl, left.end(), "bottom-left (bottom's start, left's end)")?;
        Ok(Patch::Coons { top, right, bottom, left, corners: [tl, tr, br, bl] })
    }

    /// A Coons patch over a closed outline, cut into four edges at the points
    /// nearest `corners` (TL, TR, BR, BL). Equal corners collapse an edge.
    pub fn coons_outline(outline: &BezPath, corners: [Point; 4], arclength: bool) -> Result<Patch> {
        let mut segs = single_subpath(outline, "outline")?;
        if segs.is_empty() {
            bail!("outline is empty");
        }
        let mut pos = corners.map(|c| nearest(&segs, c));
        if !cyclic(&pos, segs.len()) {
            segs = reverse(&segs);
            pos = corners.map(|c| nearest(&segs, c));
            if !cyclic(&pos, segs.len()) {
                bail!("the corners must lie around the outline in the order top-left, top-right, bottom-right, bottom-left");
            }
        }
        let piece = |a: (usize, f64), b: (usize, f64)| Edge::new(between(&segs, a, b), arclength);
        let top = piece(pos[0], pos[1]);
        let right = piece(pos[1], pos[2]);
        let bottom = Edge::new(reverse(&between(&segs, pos[2], pos[3])), arclength);
        let left = Edge::new(reverse(&between(&segs, pos[3], pos[0])), arclength);
        Patch::coons(top, right, bottom, left)
    }
}

fn bernstein(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

pub fn to_cubic(seg: PathSeg) -> CubicBez {
    match seg {
        PathSeg::Line(l) => CubicBez::new(l.p0, l.p0.lerp(l.p1, 1.0 / 3.0), l.p0.lerp(l.p1, 2.0 / 3.0), l.p1),
        PathSeg::Quad(q) => q.raise(),
        PathSeg::Cubic(c) => c,
    }
}

/// The segments of a path with exactly one subpath (its closing line included).
pub fn single_subpath(path: &BezPath, what: &str) -> Result<Vec<CubicBez>> {
    let moves = path.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count();
    if moves > 1 {
        bail!("{what} must be one subpath, got {moves}");
    }
    Ok(path.segments().map(to_cubic).filter(|c| (c.p3 - c.p0).hypot() > 0.0 || (c.p1 - c.p0).hypot() > 0.0 || (c.p2 - c.p0).hypot() > 0.0).collect())
}

fn reverse(segs: &[CubicBez]) -> Vec<CubicBez> {
    segs.iter().rev().map(|c| CubicBez::new(c.p3, c.p2, c.p1, c.p0)).collect()
}

/// The (segment, t) on the outline closest to `p`.
fn nearest(segs: &[CubicBez], p: Point) -> (usize, f64) {
    let mut best = (0, 0.0, f64::INFINITY);
    for (i, c) in segs.iter().enumerate() {
        let n = c.nearest(p, 1e-9);
        if n.distance_sq < best.2 {
            best = (i, n.t, n.distance_sq);
        }
    }
    // a segment's end is the next one's start; keep t < 1 so ordering is unambiguous
    if best.1 >= 1.0 { ((best.0 + 1) % segs.len(), 0.0) } else { (best.0, best.1) }
}

/// Whether the four positions run forward around the outline from the first (ties allowed).
fn cyclic(pos: &[(usize, f64); 4], n: usize) -> bool {
    let key = |p: (usize, f64)| p.0 as f64 + p.1;
    let s = key(pos[0]);
    let unwrapped: Vec<f64> = pos.iter().map(|&p| if key(p) < s { key(p) + n as f64 } else { key(p) }).collect();
    unwrapped.windows(2).all(|w| w[0] <= w[1])
}

/// The outline from `a` forward to `b` (wrapping), as segments. Empty when a == b.
fn between(segs: &[CubicBez], a: (usize, f64), b: (usize, f64)) -> Vec<CubicBez> {
    let n = segs.len();
    let mut out = Vec::new();
    if a == b {
        let p = segs[a.0].eval(a.1);
        return vec![CubicBez::new(p, p, p, p)];
    }
    let (mut i, mut t) = a;
    loop {
        if i == b.0 && t <= b.1 {
            if b.1 > t {
                out.push(segs[i].subsegment(t..b.1));
            }
            break;
        }
        if t < 1.0 {
            out.push(segs[i].subsegment(t..1.0));
        }
        i = (i + 1) % n;
        t = 0.0;
    }
    out
}

/// A curve parameterised over [0, 1], by arc length or by the segments' own
/// parameter, extended linearly past its ends.
#[derive(Debug)]
pub struct Edge {
    segs: Vec<CubicBez>,
    /// Per segment: cumulative arc length at sample k/SAMPLES, from the edge's start.
    table: Vec<[f64; SAMPLES + 1]>,
    total: f64,
    arclength: bool,
}

const SAMPLES: usize = 32;

impl Edge {
    pub fn new(segs: Vec<CubicBez>, arclength: bool) -> Edge {
        let mut table = Vec::with_capacity(segs.len());
        let mut acc = 0.0;
        for c in &segs {
            let mut row = [0.0; SAMPLES + 1];
            row[0] = acc;
            for k in 1..=SAMPLES {
                let t0 = (k - 1) as f64 / SAMPLES as f64;
                let t1 = k as f64 / SAMPLES as f64;
                acc += c.subsegment(t0..t1).arclen(1e-9);
                row[k] = acc;
            }
            table.push(row);
        }
        Edge { segs, table, total: acc, arclength }
    }

    pub fn from_path(path: &BezPath, arclength: bool, what: &str) -> Result<Edge> {
        let segs = single_subpath(path, what)?;
        if segs.is_empty() {
            bail!("{what} is empty");
        }
        Ok(Edge::new(segs, arclength))
    }

    fn start(&self) -> Point {
        self.segs[0].p0
    }
    fn end(&self) -> Point {
        self.segs[self.segs.len() - 1].p3
    }

    /// (segment, local t) for f in [0, 1].
    fn locate(&self, f: f64) -> (usize, f64) {
        let n = self.segs.len();
        if !self.arclength || self.total <= 0.0 {
            let x = (f * n as f64).clamp(0.0, n as f64);
            let i = (x.floor() as usize).min(n - 1);
            return (i, x - i as f64);
        }
        let s = f * self.total;
        let i = self.table.partition_point(|row| row[SAMPLES] < s).min(n - 1);
        let row = &self.table[i];
        let k = row.partition_point(|&a| a < s).clamp(1, SAMPLES);
        let (a, b) = (row[k - 1], row[k]);
        let frac = if b > a { ((s - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        (i, ((k - 1) as f64 + frac) / SAMPLES as f64)
    }

    /// Unit tangent at f (the end tangents beyond the ends).
    pub fn tangent(&self, f: f64) -> Vec2 {
        let (i, t) = self.locate(f.clamp(0.0, 1.0));
        let c = &self.segs[i];
        let mut d = c.deriv().eval(t).to_vec2();
        if d.hypot() < 1e-12 {
            // a zero derivative at a cusp or a degenerate control point: look a little inward
            let t2 = if t < 0.5 { t + 1e-3 } else { t - 1e-3 };
            d = if t < 0.5 { c.eval(t2) - c.eval(t) } else { c.eval(t) - c.eval(t2) };
        }
        let len = d.hypot();
        if len > 0.0 { d / len } else { Vec2::new(1.0, 0.0) }
    }

    pub fn at(&self, f: f64) -> Point {
        // extend along the end tangents, at the edge's length per unit of f
        let per = if self.arclength { self.total } else { self.segs.len() as f64 };
        if f < 0.0 {
            return self.start() + self.tangent(0.0) * (f * per.max(1e-9));
        }
        if f > 1.0 {
            return self.end() + self.tangent(1.0) * ((f - 1.0) * per.max(1e-9));
        }
        let (i, t) = self.locate(f);
        self.segs[i].eval(t)
    }
}

/// A patch over a `from` rect in the warp's local doc px.
pub struct PatchMap<'a> {
    pub patch: &'a Patch,
    pub from: [f64; 4],
    pub extend: bool,
    pub lenient: bool,
}

impl PatchMap<'_> {
    fn uv(&self, p: Point) -> (f64, f64) {
        ((p.x - self.from[0]) / self.from[2], (p.y - self.from[1]) / self.from[3])
    }
}

impl Map for PatchMap<'_> {
    fn at(&mut self, p: Point) -> Result<Point> {
        let (u, v) = self.uv(p);
        const EPS: f64 = 1e-9;
        let inside = (-EPS..=1.0 + EPS).contains(&u) && (-EPS..=1.0 + EPS).contains(&v);
        if !inside && !self.extend && !self.lenient {
            let [x, y, w, h] = self.from;
            bail!(
                "the point ({:.2}, {:.2}) is outside `from` {{ {x}, {y}, {w}, {h} }}: grow `from` to cover the content, or set outside = \"extend\"",
                p.x,
                p.y
            );
        }
        Ok(self.patch.at(u, v))
    }

    fn deriv(&mut self, p: Point, d: Vec2) -> Result<Vec2> {
        let (u, v) = self.uv(p);
        let (_, pu, pv) = self.patch.frame(u, v);
        Ok(pu * (d.x / self.from[2]) + pv * (d.y / self.from[3]))
    }

    fn lenient(&mut self, on: bool) {
        self.lenient = on;
    }
}

/// Warps a path: every output piece is a cubic within `tol` of the mapped curve.
pub fn warp_path(path: &BezPath, map: &mut dyn Map, tol: f64) -> Result<BezPath> {
    let mut out = BezPath::new();
    let (mut start, mut cur) = (Point::ZERO, Point::ZERO);
    let mut open = false;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.move_to(map.at(p)?);
                (start, cur, open) = (p, p, true);
            }
            PathEl::LineTo(p) => {
                warp_seg(&to_cubic(PathSeg::Line(kurbo::Line::new(cur, p))), map, tol, &mut out)?;
                cur = p;
            }
            PathEl::QuadTo(a, p) => {
                warp_seg(&kurbo::QuadBez::new(cur, a, p).raise(), map, tol, &mut out)?;
                cur = p;
            }
            PathEl::CurveTo(a, b, p) => {
                warp_seg(&CubicBez::new(cur, a, b, p), map, tol, &mut out)?;
                cur = p;
            }
            PathEl::ClosePath => {
                if open && (cur - start).hypot() > 0.0 {
                    warp_seg(&to_cubic(PathSeg::Line(kurbo::Line::new(cur, start))), map, tol, &mut out)?;
                }
                out.close_path();
                cur = start;
            }
        }
    }
    Ok(out)
}

const MAX_DEPTH: u32 = 14;

fn warp_seg(c: &CubicBez, map: &mut dyn Map, tol: f64, out: &mut BezPath) -> Result<()> {
    let d = c.deriv();
    let mut sample = |t: f64, map: &mut dyn Map| -> Result<(Point, Vec2)> {
        let p = c.eval(t);
        Ok((map.at(p)?, map.deriv(p, d.eval(t).to_vec2())?))
    };
    let a = sample(0.0, map)?;
    let b = sample(1.0, map)?;
    piece(c, 0.0, 1.0, a, b, map, tol, out, 0, &mut sample)
}

#[allow(clippy::too_many_arguments)]
fn piece(
    c: &CubicBez,
    t0: f64,
    t1: f64,
    a: (Point, Vec2),
    b: (Point, Vec2),
    map: &mut dyn Map,
    tol: f64,
    out: &mut BezPath,
    depth: u32,
    sample: &mut dyn FnMut(f64, &mut dyn Map) -> Result<(Point, Vec2)>,
) -> Result<()> {
    let dt = t1 - t0;
    let hermite = CubicBez::new(a.0, a.0 + a.1 * (dt / 3.0), b.0 - b.1 * (dt / 3.0), b.0);
    let mid = sample(t0 + 0.5 * dt, map)?;
    let mut err = (hermite.eval(0.5) - mid.0).hypot();
    if err <= tol {
        for s in [0.25, 0.75] {
            let q = map.at(c.eval(t0 + s * dt))?;
            err = err.max((hermite.eval(s) - q).hypot());
        }
    }
    if err <= tol || depth >= MAX_DEPTH {
        out.curve_to(hermite.p1, hermite.p2, hermite.p3);
        return Ok(());
    }
    let tm = t0 + 0.5 * dt;
    piece(c, t0, tm, a, mid, map, tol, out, depth + 1, sample)?;
    piece(c, tm, t1, mid, b, map, tol, out, depth + 1, sample)
}

/// Checks that an affine can be undone, for transforms folded around a warp.
pub fn invertible(a: Affine) -> Result<()> {
    if a.determinant().abs() < 1e-12 {
        return Err(anyhow!("a transform inside a Warp is singular (it flattens everything to a line or point)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_edges(x: f64, y: f64, w: f64, h: f64) -> Patch {
        let line = |a: Point, b: Point| Edge::new(vec![to_cubic(PathSeg::Line(kurbo::Line::new(a, b)))], true);
        let (tl, tr, br, bl) = (Point::new(x, y), Point::new(x + w, y), Point::new(x + w, y + h), Point::new(x, y + h));
        Patch::coons(line(tl, tr), line(tr, br), line(bl, br), line(tl, bl)).unwrap()
    }

    #[test]
    fn coons_of_a_rect_is_the_identity() {
        let patch = rect_edges(10.0, 20.0, 200.0, 100.0);
        let mut m = PatchMap { patch: &patch, from: [10.0, 20.0, 200.0, 100.0], extend: false, lenient: false };
        let circle = kurbo::Circle::new((110.0, 70.0), 40.0);
        let src = kurbo::Shape::to_path(&circle, 1e-6);
        let warped = warp_path(&src, &mut m, 0.01).unwrap();
        for k in 0..64 {
            let a = k as f64 / 64.0 * std::f64::consts::TAU;
            let p = Point::new(110.0 + 40.0 * a.cos(), 70.0 + 40.0 * a.sin());
            let d = warped.segments().map(|s| s.nearest(p, 1e-9).distance_sq).fold(f64::INFINITY, f64::min).sqrt();
            assert!(d < 0.02, "{d}");
        }
    }

    #[test]
    fn coons_outline_finds_its_edges() {
        let mut sq = BezPath::new();
        sq.move_to((0.0, 0.0));
        sq.line_to((0.0, 100.0));
        sq.line_to((100.0, 100.0));
        sq.line_to((100.0, 0.0));
        sq.close_path();
        // given counter-clockwise; corners TL, TR, BR, BL
        let p = Patch::coons_outline(&sq, [(0.0, 0.0).into(), (100.0, 0.0).into(), (100.0, 100.0).into(), (0.0, 100.0).into()], true).unwrap();
        assert!((p.at(0.25, 0.75) - Point::new(25.0, 75.0)).hypot() < 1e-6);
        // a collapsed right edge: a triangle
        let t = Patch::coons_outline(&sq, [(0.0, 0.0).into(), (100.0, 50.0).into(), (100.0, 50.0).into(), (0.0, 100.0).into()], true);
        assert!(t.is_ok());
    }

    #[test]
    fn outside_is_an_error_unless_extended() {
        let patch = rect_edges(0.0, 0.0, 10.0, 10.0);
        let mut m = PatchMap { patch: &patch, from: [0.0, 0.0, 10.0, 10.0], extend: false, lenient: false };
        assert!(m.at(Point::new(11.0, 5.0)).unwrap_err().to_string().contains("outside `from`"));
        let mut m = PatchMap { patch: &patch, from: [0.0, 0.0, 10.0, 10.0], extend: true, lenient: false };
        assert!((m.at(Point::new(11.0, 5.0)).unwrap() - Point::new(11.0, 5.0)).hypot() < 1e-6);
    }

    #[test]
    fn along_a_line_offsets_by_v() {
        let mut d = BezPath::new();
        d.move_to((0.0, 0.0));
        d.line_to((100.0, 0.0));
        let p = Patch::Along { path: Edge::from_path(&d, true, "path").unwrap(), width: 20.0 };
        assert!((p.at(0.5, 0.0) - Point::new(50.0, -10.0)).hypot() < 1e-9);
        assert!((p.at(0.5, 1.0) - Point::new(50.0, 10.0)).hypot() < 1e-9);
    }
}
