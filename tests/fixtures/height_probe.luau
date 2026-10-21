-- Measures a Height input against the exact ramp it is drawn from: a linear
-- black-to-white gradient from x0 to x1, so h = scale * (x - x0) / (x1 - x0) and
-- the slope is scale / (x1 - x0) along x, 0 along y. `blur` (-p blur=...) smooths
-- the 8-bit render in float. Tested pixels (alpha 1; away from the ends and edges,
-- where the blur legitimately rounds the ramp) encode their errors:
-- r = |h error| / (1% of scale), g = |slope x error| / (5% of the slope),
-- b = |slope y| / (5% of the slope), or 1 if the absent optional input reads as anything but 0.
local X0, X1, Y0, Y1, SCALE = 50, 350, 50, 250, 20

local Probe = effect {
	name = "hprobe",
	uniforms = { x0 = { "length", X0 }, x1 = { "length", X1 }, y0 = { "length", Y0 }, y1 = { "length", Y1 }, scale = { "length", SCALE }, margin = { "length", 20 } },
	inputs = { surface = "height", spare = "height?" },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  if (p.x < u.x0 + u.margin || p.x > u.x1 - u.margin || p.y < u.y0 + u.margin || p.y > u.y1 - u.margin) { return vec4f(0.0); }
  let s = surface(p);
  let slope = u.scale / (u.x1 - u.x0);
  let want = u.scale * (p.x - u.x0) / (u.x1 - u.x0);
  let none = spare(p);
  let absent_err = select(0.0, 1.0, none.h != 0.0 || any(none.g != vec2f(0.0)) || none.cover != 0.0);
  return vec4f(
    clamp(abs(s.h - want) / (0.01 * u.scale), 0.0, 1.0),
    clamp(abs(s.g.x - slope) / (0.05 * slope), 0.0, 1.0),
    max(clamp(abs(s.g.y) / (0.05 * slope), 0.0, 1.0), absent_err),
    1.0);
}
]],
}

return Canvas { size = { 400, 300 }, bg = "none",
	Probe { area = { 0, 0, 400, 300 },
		surface = Height { scale = SCALE, blur = param("blur", 2),
			Rect { x = X0, y = Y0 - 20, w = X1 - X0, h = Y1 - Y0 + 40, fill = linear({ X0, 0 }, { X1, 0 }, { "#000", "#fff" }) },
		},
	},
}
