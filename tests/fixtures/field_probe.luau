-- Measures an Sdf input against the exact distance field of a square rotated
-- 17 degrees (a polygon, so the rendered edge is exact; curves are flattened
-- into chords by the rasterizer, which the field faithfully follows). Each
-- tested pixel (alpha 1; the rest are transparent) encodes its errors:
-- r = |d error| / 0.1px, g = normal angle error / 8 degrees, b = |march error| / 0.5px for a ray
-- straight down. Skipped: the square's diagonals (no unique normal), the
-- wedges outside each corner and 4px around it (antialiasing rounds a corner,
-- so the drawn contour there is legitimately not the square's), rays that graze a corner,
-- and marches that start within 0.25px of the contour (inside vs outside is
-- ambiguous there).
local C, HALF, ANGLE = 200, 90, 17

local Probe = effect {
	name = "probe",
	uniforms = {
		c = { "length", C }, half = { "length", HALF }, angle = { "angle", ANGLE },
		range = { "length", 60 }, maxd = { "length", 150 },
	},
	inputs = { body = "sdf" },
	reach = "maxd", -- marches read this far
	grow = "range", -- draws every pixel within the field's range of the square
	wgsl = [[
// True signed distance, positive inside, and the point in the square's frame.
fn local_p(p: vec2f) -> vec2f { return rot(-u.angle) * (p - vec2f(u.c)); }
fn truth(p: vec2f) -> f32 {
  let q = abs(local_p(p)) - vec2f(u.half);
  return -(length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0));
}
// Ray/square intersection (slabs) in the square's frame: distance to the first
// boundary crossing along dir from p, or 1e9.
fn hit(p: vec2f, dir: vec2f) -> f32 {
  let o = local_p(p);
  let d = rot(-u.angle) * dir;
  let t1 = (vec2f(-u.half) - o) / d;
  let t2 = (vec2f(u.half) - o) / d;
  let tn = max(min(t1.x, t2.x), min(t1.y, t2.y));
  let tf = min(max(t1.x, t2.x), max(t1.y, t2.y));
  if (tn > tf || tf < 0.0) { return 1e9; }
  return select(tf, tn, tn > 0.0);
}

fn effect(p: vec2f) -> vec4f {
  let want = truth(p);
  let q = abs(local_p(p)) - vec2f(u.half);
  if (abs(want) > u.range - 2.0 || (want > 0.0 && abs(q.x - q.y) < 3.0) || min(q.x, q.y) > -2.0 || length(q) < 4.0) { return vec4f(0.0); }
  let f = body(p);
  let d_err = abs(f.d - want);
  let h = 0.05;
  let g = vec2f(truth(p + vec2f(h, 0.0)) - truth(p - vec2f(h, 0.0)), truth(p + vec2f(0.0, h)) - truth(p - vec2f(0.0, h)));
  let ang_err = degrees(acos(clamp(dot(f.n, -normalize(g)), -1.0, 1.0)));
  // Vertical rays within 3px of a corner's column graze it; skip those.
  var m_err = 0.0;
  var grazes = false;
  for (var k = 0; k < 4; k++) {
    let corner = vec2f(u.c) + rot(u.angle) * (vec2f(f32(k & 1), f32(k >> 1u)) * 2.0 - 1.0) * u.half;
    grazes = grazes || abs(corner.x - p.x) < 3.0;
  }
  if (!grazes && abs(want) > 0.25) {
    m_err = abs(body_march(p, vec2f(0.0, 1.0), u.maxd) - min(hit(p, vec2f(0.0, 1.0)), u.maxd));
  }
  return vec4f(clamp(d_err / 0.1, 0.0, 1.0), clamp(ang_err / 8.0, 0.0, 1.0), clamp(m_err / 0.5, 0.0, 1.0), 1.0);
}
]],
}

local corners = {}
for k = 0, 3 do
	local a = math.rad(ANGLE + 45 + 90 * k)
	table.insert(corners, { C + HALF * math.sqrt(2) * math.cos(a), C + HALF * math.sqrt(2) * math.sin(a) })
end

return Canvas { size = { 400, 400 }, bg = "none",
	Probe { body = Sdf { range = 60, Path { points = corners, closed = true } } },
}
