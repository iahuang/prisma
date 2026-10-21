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
