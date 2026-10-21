-- Every primitive once: shapes, gradients, strokes, text, blend, mask, blur, effects.
local W, H = 800, 600

return Canvas { size = { W, H }, bg = "#f3efe6",
	Rect { x = 40, y = 40, w = 220, h = 140, r = 12, fill = linear({ 40, 40 }, { 260, 180 }, { "#ff4b1f", "#2b50ff" }) },
	Circle { cx = 400, cy = 110, r = 70, fill = radial({ 380, 90 }, 70, { "#fff", "#00a35c" }), stroke = "#141414", strokeWidth = 4 },
	Ellipse { cx = 640, cy = 110, rx = 110, ry = 50, rotate = -20, fill = "#e8402a" },
	Line { x1 = 40, y1 = 230, x2 = 760, y2 = 230, stroke = "#141414", strokeWidth = 3, dash = { 12, 6 }, cap = "round" },
	Path { d = "M40,300 C140,240 240,360 340,300 S540,240 640,300", fill = "none", stroke = "#2b50ff", strokeWidth = 8, cap = "round" },

	-- multiply: red and blue overlap to near-black
	Group { x = 40, y = 340,
		Circle { cx = 60, cy = 60, r = 55, fill = "#e8402a", blend = "multiply" },
		Circle { cx = 120, cy = 60, r = 55, fill = "#2a5cd6", blend = "multiply" },
	},

	-- a soft mask: a blurred circle cuts the stripes
	Mask { shape = Circle { cx = 340, cy = 400, r = 60, blur = 12 },
		(function()
			local t = {}
			for i = 0, 14 do
				table.insert(t, Rect { x = 270 + i * 10, y = 330, w = 5, h = 140, fill = "#141414" })
			end
			return t
		end)(),
	},

	Group { blur = 6, Rect { x = 450, y = 340, w = 120, h = 120, fill = "#00a35c" } },

	Halftone { cell = 9, angle = 30, ink = "#1d2a6b",
		Circle { cx = 680, cy = 400, r = 70, fill = radial({ 660, 380 }, 80, { "#fff", "#000" }) },
	},

	Text { x = 40, y = 537, size = 48, font = "Helvetica", weight = 700, fill = "#141414", "prisma" },
	Text { x = 760, y = 532, size = 14, font = "Courier New", anchor = "end", fill = "#555", "vello + wgpu + luau" },
}
