-- Type on baselines: `y` is where the first line sits, so mixed sizes share a
-- rule. A text gradient is in canvas coordinates like any other fill (here,
-- cap height y = 280 to baseline y = 360). An effect with an `area` paints a
-- full-bleed paper ground without needing children to cover the canvas.
local W, H, BASE = 800, 400, 200

local Paper = effect {
	name = "paper",
	uniforms = { tone = { "color", "#f1e9da" }, fiber = { "length", 6 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let n = fbm(p / u.fiber) - 0.5;
  return vec4f(clamp(u.tone.rgb + n * 0.06, vec3f(0.0), vec3f(1.0)), 1.0);
}
]],
}

return Canvas { size = { W, H },
	Paper { area = { 0, 0, W, H } },

	Line { x1 = 30, y1 = BASE, x2 = W - 30, y2 = BASE, stroke = "#e8402a", strokeWidth = 1 },
	Text { font = "Helvetica", weight = 700, size = 140, x = 40, y = BASE, fill = "#141414", "Hx" },
	Text { font = "Helvetica", size = 36, x = 250, y = BASE, fill = "#141414", "one baseline" },
	Text { font = "Courier New", size = 14, x = W - 40, y = BASE, anchor = "end", fill = "#555", "y = 200" },

	Text { font = "Arial Black", size = 110, x = 40, y = 360,
		fill = linear({ 0, 280 }, { 0, 360 }, { "#ff4b1f", "#2b50ff" }), "GRADIENT" },
}
