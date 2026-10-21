-- Type as geometry: glyph outlines become a Path you can stroke, dash and mask with.
local d = outline(Text { font = "Arial Black", size = 220, x = 30, y = 252, "Glass" })
return Canvas { size = { 760, 320 }, bg = "#101418",
	Path { d = d, fill = "none", stroke = "#7fd1ff", strokeWidth = 2, dash = { 6, 4 } },
	Mask { shape = Path { d = d },
		(function()
			local t = {}
			for i = 0, 60 do
				table.insert(t, Line { x1 = -100 + i * 16, y1 = 320, x2 = 100 + i * 16, y2 = 0, stroke = "#ff6a3d", strokeWidth = 5 })
			end
			return t
		end)(),
	},
}
