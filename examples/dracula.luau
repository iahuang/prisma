-- Recreation of the "Dracula ou la musique troue le ciel" poster: blurred
-- blobs whose edges are broken up into airbrush-like speckle.
local W, H = 1091, 1600

local C = {
	cream = "#f6dccd", blue = "#2a9be2", red = "#e8383c", darkRed = "#b0101c", pink = "#f2a2a6",
	green = "#2f9468", mint = "#72d2b4", violet = "#5257cc", ink = "#141414",
}

-- Each pixel takes its color from a random nearby pixel. Over a blurred edge
-- this turns the gradient into a spray of dots, which is the poster's texture.
local Scatter = effect {
	name = "scatter",
	uniforms = { amount = { "length", 14 } },
	reach = "amount", -- reads this far around each pixel
	grow = "amount", -- and scatters ink this far past the blobs
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let a = hash(p) * 6.2831853;
  let r = sqrt(hash(p + 31.7)) * u.amount;
  // Clamp so pixels near the canvas edge don't pull in transparency.
  return src(clamp(p + vec2f(cos(a), sin(a)) * r, vec2f(0.5), sys.resolution - 0.5));
}
]],
}

-- A horizontal lens with concave sides and sharp tips.
local function lens(cx, cy, hw, hh)
	local k = 0.34
	return `M{cx - hw},{cy} Q{cx - hw * k},{cy - hh * k} {cx},{cy - hh} Q{cx + hw * k},{cy - hh * k} {cx + hw},{cy}`
		.. ` Q{cx + hw * k},{cy + hh * k} {cx},{cy + hh} Q{cx - hw * k},{cy + hh * k} {cx - hw},{cy} Z`
end

-- Blue sky bands over the cream ground: {cx, cy, rx, ry}.
local bands = {
	{ 420, 60, 520, 95 }, { 1000, 90, 220, 80 }, { 120, 255, 230, 40 }, { 960, 240, 260, 45 },
	{ 430, 345, 480, 38 }, { 1040, 330, 140, 35 }, { 80, 420, 180, 40 }, { 640, 425, 380, 30 },
	{ 545, 540, 620, 50 }, { 545, 650, 640, 62 }, { 110, 760, 240, 55 }, { 1010, 770, 200, 55 },
	{ 90, 870, 200, 38 }, { 1020, 880, 200, 40 }, { 1040, 1265, 140, 26 }, { 60, 1380, 110, 30 },
}

-- The central stack: {cx, cy, rx, ry, fill}.
local stack = {
	{ 535, 742, 465, 92, C.green }, { 545, 882, 400, 74, C.red }, { 530, 1008, 380, 60, C.violet },
	{ 540, 1118, 340, 55, C.mint }, { 540, 1215, 300, 50, "#2a8a66" }, { 553, 1295, 280, 45, C.red },
	{ 545, 1375, 245, 38, "#4a58d6" }, { 550, 1445, 230, 38, C.mint }, { 545, 1525, 290, 40, "#c8282e" },
}

-- Soft shapes bleeding in from the margins.
local sides = {
	{ 90, 1080, 200, 95, "#e24a4a" }, { 1010, 1000, 200, 105, C.green }, { 1000, 1225, 190, 85, C.violet },
	{ 80, 1320, 200, 120, "#2b8a62" }, { 1010, 1450, 170, 90, "#3a9a5c" }, { 60, 1560, 190, 60, C.mint },
	{ 210, 1580, 160, 40, "#e05050" },
}

local function ellipses(list, color)
	local out = {}
	for _, e in list do
		table.insert(out, Ellipse { cx = e[1], cy = e[2], rx = e[3], ry = e[4], fill = e[5] or color })
	end
	return out
end

local stackShapes = {}
for i, e in stack do
	table.insert(stackShapes, Ellipse {
		cx = e[1], cy = e[2], rx = e[3], ry = e[4], fill = e[5],
		blur = 12 + (i - 1) * 0.8, opacity = if i == #stack then 0.7 else 1,
	})
end

local eyes = {}
for _, cx in { 300, 785 } do
	table.insert(eyes, Path { d = lens(cx, 578, 205, 80), fill = C.pink, blur = 4 })
	table.insert(eyes, Circle { cx = cx, cy = 578, r = 74, fill = C.red, blur = 9 })
end

return Canvas { size = { W, H }, bg = C.cream,
	Grain { amount = 0.09, size = 1,
		Scatter { amount = 16,
			Group { blur = 20, ellipses(bands, C.blue) },
			Group { blur = 28, ellipses(sides) },
			stackShapes,

			-- ring
			Group { blur = 10,
				Circle { cx = 790, cy = 332, r = 218, fill = "none", stroke = C.red, strokeWidth = 46 },
				Circle { cx = 790, cy = 332, r = 196, fill = radial({ 800, 312 }, 196, {
					{ 0, "#eeb0c4" }, { 0.3, "#e8a8bc" }, { 0.5, "#2a9a58" }, { 0.78, "#28b070" }, { 1, "#5ccf98" },
				}) },
			},
			Group { blur = 10,
				Circle { cx = 185, cy = 318, r = 80, fill = C.red },
				Circle { cx = 180, cy = 445, r = 62, fill = "#4a4fc6" },
			},
			Ellipse { cx = 505, cy = 395, rx = 305, ry = 52, rotate = -17, fill = C.darkRed, blur = 10 },
			eyes,
		},

		Text { font = "Helvetica", size = 33, x = 8, y = 103, fill = "#16233f", "OU LA MUSIQUE TROUE LE CIEL" },

		-- the green patch sits over the subtitle, half-hiding MUSIQUE
		Scatter { amount = 16,
			Group { blur = 22, opacity = 0.9, Path { d = "M110,95 L300,95 L300,215 L110,215 Z", fill = "#2c8a5e" } },
		},

		-- condensed serif: Times squeezed horizontally
		Group { x = 10, y = -12, scale = { 0.6, 1 }, Text { font = "Times New Roman", size = 98, y = 89, fill = C.ink, "DRACULA" } },
		Text { font = "Helvetica", size = 29, x = 496, y = 141, leading = 1.12, fill = C.ink,
			"DIRECTION MUSICALE: MAXIME PASCAL\nARTISTE INVITÉ: JACQUES" },
	},
}
