-- Sdf inputs: an effect reads a shape's distance field as float data, not as a
-- picture. `body(p)` gives the exact signed distance, outward normal, nearest
-- contour point and coverage; `body_march` finds the contour along a ray.
-- Here the normal lights the shapes, the distance draws contour lines around
-- them, and a march straight up casts each shape's shadow onto the ground.

local Lit = effect {
	name = "lit",
	uniforms = {
		light = { "angle", -120 },   -- direction the light comes from
		shadow = { "length", 140 },  -- how far below a shape its shadow reaches
		spacing = { "length", 9 },   -- contour line spacing
		ink = { "color", "#1d2a6b" },
		lit = { "color", "#ffb347" },
		paper = { "color", "#f3efe6" },
	},
	inputs = { body = "sdf" },
	reach = "shadow", -- marches read this far beyond what is drawn, so crops render it
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let f = body(p);
  let to_light = vec2f(cos(u.light), sin(u.light));
  // Inside: lambert shading from the normal, fading to flat away from the edge
  // (a cushion: the edge bends toward the light, the middle faces the viewer).
  let bend = 1.0 - smoothstep(0.0, 30.0 * sys.px_per_unit, f.d);
  let k = 0.55 + 0.45 * dot(f.n, to_light) * bend;
  let body_col = mix(unpm(u.ink).rgb, unpm(u.lit).rgb, clamp(k, 0.0, 1.0));
  // Outside: contour lines every `spacing` px, and a shadow where a shape sits above.
  let up = body_march(p, vec2f(0.0, -1.0), u.shadow);
  let shade = select(0.0, 0.35 * (1.0 - up / u.shadow), up < u.shadow);
  // Contours: 1px lines at whole multiples of `spacing` outside, fading out by the 5th.
  let t = -f.d / u.spacing;
  let dist_to_line = abs(fract(t + 0.5) - 0.5) * u.spacing;
  let line = (1.0 - smoothstep(0.5 * sys.px_per_unit, sys.px_per_unit, dist_to_line)) * step(0.5, t) * (1.0 - smoothstep(2.0, 5.0, t));
  var ground = unpm(u.paper).rgb * (1.0 - shade);
  ground = mix(ground, unpm(u.ink).rgb, 0.5 * line);
  return vec4f(mix(ground, body_col, f.cover), 1.0);
}
]],
}

local blob = Path { closed = true, smooth = true, points = {
	{ 120, 150 }, { 230, 90 }, { 330, 130 }, { 300, 230 }, { 190, 250 }, { 110, 220 },
} }

return Canvas { size = { 800, 400 }, bg = "#f3efe6",
	Lit { area = { 0, 0, 800, 400 },
		body = Sdf { range = 60,
			blob,
			Circle { cx = 470, cy = 170, r = 70 },
			Text { x = 560, y = 230, size = 150, font = "Arial Black", "F" },
		},
	},
}
