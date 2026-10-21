-- Images and offscreen passes bigger than vello's 8192px image atlas.
-- `-p case=...` picks one.
local case = param("case", "page")

-- A tall page: 1000px bands cycling red, green, blue, with a black 100px
-- square at (250, 10050). Rendered once, then read back by "view".
local function page()
	local bands = {}
	local colors = { "#ff0000", "#00ff00", "#0000ff" }
	for i = 0, 11 do
		bands[#bands + 1] = Rect { y = i * 1000, w = 600, h = 1000, fill = colors[i % 3 + 1] }
	end
	bands[#bands + 1] = Rect { x = 250, y = 10050, w = 100, h = 100, fill = "#000000" }
	return Canvas { size = { 600, 12000 }, table.unpack(bands) }
end

-- The page drawn whole at a tenth of its size (left), and a 1:1 crop around
-- the square (right).
local function view()
	local src = param("src", "")
	return Canvas { size = { 500, 1200 }, bg = "#ffffff",
		Image { src = src, x = 0, y = 0, w = 60, h = 1200 },
		Image { src = src, x = 100, y = 0, w = 400, h = 400, crop = { 100, 9900, 400, 400 } },
	}
end

-- A blurred rect taller than the atlas: its pass texture goes back in tiles.
local function tallblur()
	return Canvas { size = { 400, 10000 }, bg = "#ffffff",
		Rect { x = 100, y = 100, w = 200, h = 9800, fill = "#0000ff", blur = 4 },
	}
end

return ({ page = page, view = view, tallblur = tallblur })[case]()
