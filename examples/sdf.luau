-- Distance fields: an Sdf turns shapes into "how far from the edge", which
-- effects can band, offset and outline. Paths through points make the blobs.

-- Left: the raw field of a circle (range 50: white 50px inside, black 50px outside).
local raw = Group {
	Rect { x = 0, y = 0, w = 400, h = 400, fill = "#000" },
	Sdf { range = 50, Circle { cx = 200, cy = 200, r = 100 } },
}

-- Right: contour bands from the field of two smooth blobs.
local Bands = effect {
	name = "bands",
	uniforms = { range = { "length", 60 }, spacing = { "length", 9 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let d = sdf_dist(src(p).a, u.range);
  let t = d / u.spacing;
  let line = smoothstep(0.12, 0.0, abs(fract(t) - 0.5) - 0.32);
  let hue = 0.5 + 0.5 * cos(6.2831853 * (d / (u.range * 2.0) + vec3f(0.0, 0.33, 0.67)));
  let ink = select(vec3f(0.07, 0.08, 0.12), hue, d > 0.0);
  let c = mix(vec3f(0.95, 0.94, 0.9), ink, max(line, step(0.0, d) * 0.85));
  return vec4f(c, 1.0);
}
]],
}

local blobs = {
	Path { closed = true, smooth = true, points = { { 470, 90 }, { 600, 70 }, { 690, 150 }, { 640, 230 }, { 520, 200 }, { 450, 160 } } },
	Path { closed = true, smooth = true, points = { { 520, 300 }, { 640, 260 }, { 740, 320 }, { 700, 360 }, { 560, 350 } } },
}

return Canvas { size = { 800, 400 }, bg = "#000",
	raw,
	Group { clip = { 400, 0, 400, 400 },
		-- the field is read from alpha, so nothing opaque goes under it
		Bands { range = 60, spacing = 9,
			Sdf { range = 60, blobs },
		},
		-- the same points as a polyline, to show the spline passes through them
		Path { points = blobs[1].points, closed = true, fill = "none", stroke = "#fff", strokeWidth = 1.5, dash = { 4, 3 } },
	},
}
