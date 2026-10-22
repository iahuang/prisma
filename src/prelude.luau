-- Loaded into every script before the sandbox is sealed: helpers, then the
-- standard effects (written exactly as a user would write their own).

-- Seeded random: random() in [0, 1), random(max), random(min, max). Same stream as math.random.
function random(a: number?, b: number?): number
	local r = math.random()
	if a == nil then
		return r
	elseif b == nil then
		return r * a
	end
	return a + r * (b - a)
end

function pick<T>(xs: { T }): T
	return xs[math.random(#xs)]
end

-- Gradients for `fill`/`stroke`. Stops are colors (evenly spaced) or {t, color} pairs.
function linear(from, to, stops)
	return { _grad = "linear", from = from, to = to, stops = stops }
end

function radial(center, r, stops)
	return { _grad = "radial", center = center, r = r, stops = stops }
end

-- Cell centers of a square grid over size = {w, h}: { x, y, i, j }.
function grid(size, step: number, offset)
	local ox, oy = step / 2, step / 2
	if offset then
		ox, oy = offset[1], offset[2]
	end
	local pts = {}
	local j, y = 0, oy
	while y < size[2] do
		local i, x = 0, ox
		while x < size[1] do
			table.insert(pts, { x = x, y = y, i = i, j = j })
			i, x = i + 1, x + step
		end
		j, y = j + 1, y + step
	end
	return pts
end

-- Centers of a hex grid (rows offset by half a step).
function hexGrid(size, step: number)
	local dy = step * math.sqrt(3) / 2
	local pts = {}
	local j, y = 0, dy / 2
	while y < size[2] do
		local i, x = 0, if j % 2 == 1 then step else step / 2
		while x < size[1] do
			table.insert(pts, { x = x, y = y, i = i, j = j })
			i, x = i + 1, x + step
		end
		j, y = j + 1, y + dy
	end
	return pts
end

-- Each point with the raster's r, g, b, a, lum at it.
function sample(r, pts)
	return r:sample(pts)
end

-- ---- easing ----------------------------------------------------------------
-- Curves from 0 to 1 over k in [0, 1]. k is clamped, so a time window is
-- easeOut((t - start) / length). p is the power: 2 quad, 3 cubic, 5 quint.

function easeIn(k: number, p: number?): number
	return math.clamp(k, 0, 1) ^ (p or 3)
end

function easeOut(k: number, p: number?): number
	return 1 - (1 - math.clamp(k, 0, 1)) ^ (p or 3)
end

function easeInOut(k: number, p: number?): number
	k = math.clamp(k, 0, 1)
	if k < 0.5 then
		return (2 * k) ^ (p or 3) / 2
	end
	return 1 - (2 - 2 * k) ^ (p or 3) / 2
end

-- Overshoots and settles: crosses 1 `swings` + 1 times, the last at k = 1.
-- Higher `damping` shrinks the overshoot.
function easeSpring(k: number, swings: number?, damping: number?): number
	k = math.clamp(k, 0, 1)
	return 1 - math.cos(math.pi * ((swings or 2) + 0.5) * k) * math.exp(-(damping or 4) * k)
end

-- ---- standard effects ------------------------------------------------------

Halftone = effect({
	name = "halftone",
	uniforms = { cell = { "length", "1.5mm" }, angle = { "angle", 45 }, ink = { "color", "#000" }, paper = { "color", "#0000" } },
	reach = "cell",
	grow = "cell",
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let q = rot(-u.angle) * p;
  let ctr = (floor(q / u.cell) + 0.5) * u.cell;
  let c = unpm(src(rot(u.angle) * ctr));
  let cover = (1.0 - luma(c.rgb)) * c.a;
  let r = sqrt(cover) * u.cell * 0.7071;
  let a = (1.0 - smoothstep(r - 0.75, r + 0.75, length(q - ctr))) * clamp(r / 0.75, 0.0, 1.0);
  return mix(u.paper, u.ink, a);
}
]],
})

Duotone = effect({
	name = "duotone",
	uniforms = { dark = { "color", "#1b1b3a" }, light = { "color", "#f4e9d8" } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let c = unpm(src(p));
  let o = mix(unpm(u.dark), unpm(u.light), luma(c.rgb));
  return pm(vec4f(o.rgb, o.a * c.a));
}
]],
})

Posterize = effect({
	name = "posterize",
	uniforms = { levels = { "float", 4 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let c = unpm(src(p));
  let n = max(u.levels, 2.0);
  return pm(vec4f(min(floor(c.rgb * n), vec3f(n - 1.0)) / (n - 1.0), c.a));
}
]],
})

-- Per-pixel noise in lightness; `size` is the grain cell (a length).
Grain = effect({
	name = "grain",
	uniforms = { amount = { "float", 0.12 }, size = { "length", 1 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let c = src(p);
  let n = hash(floor(p / max(u.size, 1.0))) - 0.5;
  return vec4f(clamp(c.rgb + n * u.amount * c.a, vec3f(0.0), vec3f(c.a)), c.a);
}
]],
})

-- Smooth noise displacement: moves pixels up to `amount` (a length).
Displace = effect({
	name = "displace",
	uniforms = { amount = { "length", 6 }, freq = { "length", 40 }, seed = { "float", 0 } },
	reach = "amount",
	grow = "amount",
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let q = p / u.freq + u.seed * 13.1;
  let d = vec2f(fbm(q), fbm(q + 17.3)) - 0.5;
  return src(p + d * 2.0 * u.amount);
}
]],
})

-- Dust and pinholes: at most one dot per `spacing` cell, `density` = chance a cell has one.
-- All in lengths, so the same dots appear at any scale.
Speckle = effect({
	name = "speckle",
	uniforms = {
		spacing = { "length", 20 },
		size = { "length", 0.8 },
		density = { "float", 0.3 },
		light = { "color", "#e0e8e8" },
		dark = { "color", "#000a" },
		darkShare = { "float", 0.3 },
		seed = { "float", 0 },
	},
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let c = src(p);
  let cell = floor(p / u.spacing);
  let h = hash3(cell + u.seed * 17.13);
  if (h.x >= u.density) { return c; }
  let k = hash3(cell + 91.7 + u.seed);
  let r = u.size * (0.5 + k.x);
  let ctr = (cell + 0.2 + 0.6 * h.yz) * u.spacing;
  let cover = 1.0 - smoothstep(r - 0.5, r + 0.5, length(p - ctr));
  let ink = select(u.light, u.dark, k.y < u.darkShare);
  // Ink is premultiplied; scale to the underlying coverage so dots stay inside shapes.
  let dot = vec4f(unpm(ink).rgb * c.a, c.a);
  return mix(c, dot, cover * ink.a);
}
]],
})

-- Ragged, printed-looking edges: re-threshold alpha against noise.
Rough = effect({
	name = "rough",
	uniforms = { amount = { "float", 0.5 }, freq = { "length", 3 }, seed = { "float", 0 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let c = unpm(src(p));
  let n = fbm(p / u.freq + u.seed * 7.7) - 0.5;
  let a = smoothstep(0.45, 0.55, c.a + n * u.amount);
  return pm(vec4f(c.rgb, a));
}
]],
})

-- ---- glass -----------------------------------------------------------------

-- The surface Refract and Gloss share: a slab `thickness` high whose edge falls
-- to 0 across a `bevel`-wide rim, shaped by a superellipse of exponent `curve`
-- (1 chamfer, 2 round, 4 squircle; higher is flatter on top and steeper at the
-- edge), plus the optional `surface` height map. Heights in device px.
local RELIEF = [[
struct Relief { h: f32, g: vec2f, cover: f32 }
fn relief(p: vec2f) -> Relief {
  let f = body(p);
  var r: Relief;
  r.cover = f.cover;
  r.h = u.thickness;
  r.g = vec2f(0.0);
  if (u.bevel > 0.0) {
    let s = clamp(max(f.d, 0.5) / u.bevel, 0.0, 1.0);
    if (s < 1.0) {
      let k = max(u.curve, 1.0);
      let om = 1.0 - s;
      let a = max(1.0 - pow(om, k), 1e-5);
      r.h = u.thickness * pow(a, 1.0 / k);
      // d grows inward, so the height rises against the outward normal.
      r.g = -f.n * min(u.thickness / u.bevel * pow(om, k - 1.0) * pow(a, 1.0 / k - 1.0), 12.0);
    }
  }
  let m = surface(p);
  r.h += m.h;
  r.g += m.g;
  return r;
}
]]

-- What's behind a transparent body, seen through it: the backdrop (its
-- children) refracted by Snell's law through the body's top surface and flat
-- bottom, across `gap` to the backdrop's plane. `dispersion` refracts each of
-- `bands` wavelengths separately, `frost` scatters over a disk (`samples`
-- taps), and the light absorbed on the way is `tint` per `thickness` crossed
-- (Beer-Lambert). Fresnel reflection mixes in `env`, the colour around the
-- glass, by its alpha (transparent: no reflection). Draws only inside `body`.
Refract = effect({
	name = "refract",
	inputs = { body = "sdf", surface = "height?" },
	uniforms = {
		thickness = { "length", 20 },
		bevel = { "length", 20 },
		curve = { "float", 2 },
		gap = { "length", 0 },
		ior = { "float", 1.5 },
		dispersion = { "float", 0 },
		bands = { "float", 7 },
		bend = { "float", 1 },
		frost = { "length", 0 },
		samples = { "float", 16 },
		tint = { "color", "#fff" },
		env = { "color", "#0000" },
		reach = { "length", 200 },
	},
	reach = "reach",
	wgsl = RELIEF .. [[
// Weights of wavelength t (0 red .. 1 violet) in r, g, b.
fn spectrum(t: f32) -> vec3f {
  let x = (vec3f(t) - vec3f(0.12, 0.5, 0.88)) / 0.32;
  return max(vec3f(1.0) - x * x, vec3f(0.0));
}

fn effect(p: vec2f) -> vec4f {
  let base = src(p);
  let r = relief(p);
  if (r.cover <= 0.0) { return base; }
  let n3 = normalize(vec3f(-r.g, 1.0));
  let nb = select(1, clamp(i32(u.bands), 1, 16), u.dispersion != 0.0);
  let nf = select(1, clamp(i32(u.samples), 1, 64), u.frost > 0.0);
  let spin = hash(p) * 6.2831853;
  var acc = vec3f(0.0);
  var alpha = vec3f(0.0); // weighted like the colour, so partly transparent reads keep their hue
  var wsum = vec3f(0.0);
  var path = 0.0;
  var tir = 0.0;
  for (var b = 0; b < nb; b++) {
    let t = (f32(b) + 0.5) / f32(nb);
    let eta = max(u.ior + u.dispersion * (t - 0.5), 1e-3);
    let w = select(vec3f(1.0), spectrum(t), nb > 1);
    let r1 = refract(vec3f(0.0, 0.0, -1.0), n3, 1.0 / eta);
    var off = vec2f(0.0);
    if (dot(r1, r1) < 1e-6) {
      tir += 1.0; // total internal reflection (ior < 1): no light gets in
    } else {
      off = r1.xy / max(-r1.z, 0.05) * r.h;
      path += max(r.h, 0.0) / max(-r1.z, 0.05);
      if (u.gap > 0.0) {
        var r2 = refract(r1, vec3f(0.0, 0.0, 1.0), eta);
        if (dot(r2, r2) < 1e-6) { r2 = r1; }
        off += r2.xy / max(-r2.z, 0.05) * u.gap;
      }
      off *= u.bend;
    }
    for (var j = 0; j < nf; j++) {
      var o = off;
      if (nf > 1) {
        let a = spin + f32(j) * 2.3999632;
        o += vec2f(cos(a), sin(a)) * u.frost * sqrt((f32(j) + 0.5) / f32(nf));
      }
      let ol = length(o);
      if (ol > u.reach) { o *= u.reach / ol; }
      let c = src(p + o);
      acc += c.rgb * w;
      alpha += c.a * w;
    }
    wsum += w * f32(nf);
  }
  let ta = dot(alpha / wsum, vec3f(1.0 / 3.0));
  var c = acc / max(alpha, vec3f(1e-4));
  let lit = f32(nb) - tir;
  if (lit > 0.0) {
    let k = select(1.0, path / lit / u.thickness, u.thickness > 0.0);
    c *= pow(max(unpm(u.tint).rgb, vec3f(1e-4)), vec3f(k));
  }
  let f0 = pow((u.ior - 1.0) / (u.ior + 1.0), 2.0);
  let fres = max(f0 + (1.0 - f0) * pow(1.0 - n3.z, 5.0), tir / f32(nb));
  let env = unpm(u.env);
  c = mix(c, env.rgb, fres * env.a);
  let a = ta + (1.0 - ta) * fres * env.a;
  return mix(base, vec4f(min(c, vec3f(1.0)) * a, a), r.cover);
}
]],
})

-- A light's highlight on the same surface as Refract (give it the same body,
-- surface, thickness, bevel and curve): Blinn-Phong for a light from direction
-- `light` at `elevation` above the surface, added over the children in `color`
-- (alpha = strength). Stack several for several lights.
Gloss = effect({
	name = "gloss",
	inputs = { body = "sdf", surface = "height?" },
	uniforms = {
		thickness = { "length", 20 },
		bevel = { "length", 20 },
		curve = { "float", 2 },
		light = { "angle", -135 },
		elevation = { "angle", 45 },
		shininess = { "float", 50 },
		color = { "color", "#fff" },
	},
	wgsl = RELIEF .. [[
fn effect(p: vec2f) -> vec4f {
  let base = src(p);
  let r = relief(p);
  if (r.cover <= 0.0) { return base; }
  let n3 = normalize(vec3f(-r.g, 1.0));
  let l3 = vec3f(vec2f(cos(u.light), sin(u.light)) * cos(u.elevation), sin(u.elevation));
  let k = pow(max(dot(n3, normalize(l3 + vec3f(0.0, 0.0, 1.0))), 0.0), u.shininess) * r.cover;
  let c = u.color * k;
  return vec4f(base.rgb + c.rgb, base.a + c.a * (1.0 - base.a));
}
]],
})
