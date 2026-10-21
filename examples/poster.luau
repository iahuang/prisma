-- An event poster: grid, rotated info columns, random-case title repeated off
-- the page, grid-snapped dots, a rough scribble. Try --sweep seed=1..9.
local W, H, COLS, ROWS = 600, 850, 6, 9
local cw, rh = W / COLS, H / ROWS
local INK, ACCENT = "#141414", pick({ "#ff4b1f", "#2b50ff", "#00a35c" })

local function randomCase(s: string): string
	return (s:gsub("%a", function(c)
		return if random() < 0.45 then c:lower() else c:upper()
	end))
end

-- A wandering line through random control points, smoothed with Catmull-Rom.
local function scribble(n: number): string
	local pts = {}
	for i = 1, n do
		pts[i] = { random(60, W - 60), random(200, H - 120) }
	end
	local d = `M{pts[1][1]},{pts[1][2]}`
	for i = 1, n - 1 do
		local p0, p1, p2, p3 = pts[math.max(1, i - 1)], pts[i], pts[i + 1], pts[math.min(n, i + 2)]
		d ..= ` C{p1[1] + (p2[1] - p0[1]) / 6},{p1[2] + (p2[2] - p0[2]) / 6}`
			.. ` {p2[1] - (p3[1] - p1[1]) / 6},{p2[2] - (p3[2] - p1[2]) / 6} {p2[1]},{p2[2]}`
	end
	return d
end

local gridLines = {}
for i = 1, COLS - 1 do
	table.insert(gridLines, Line { x1 = i * cw, y1 = 0, x2 = i * cw, y2 = H, stroke = "#0002", strokeWidth = 0.75 })
end
for j = 1, ROWS - 1 do
	table.insert(gridLines, Line { x1 = 0, y1 = j * rh, x2 = W, y2 = j * rh, stroke = "#0002", strokeWidth = 0.75 })
end

local dots = {}
for k = 0, COLS * ROWS - 1 do
	if random() < 0.22 then
		local i, j = k % COLS, k // COLS
		table.insert(dots, Circle { cx = (i + 0.5) * cw, cy = (j + 0.5) * rh, r = random(4, 16), fill = ACCENT })
	end
end

local titles = {}
for k = 0, 3 do
	table.insert(titles, Text {
		font = "Arial Black", size = 118, x = -14 + k * 38, y = rh * 2 + 130 + k * 104, tracking = -0.04, fill = INK,
		randomCase("GRAPHIC DESIGN"),
	})
end

local info = { "OPEN STUDIO / GRAPHIC DESIGN DEPT.", "THU 14 NOV — SAT 16 NOV 2026", "LECTURES, PRINT SHOP, PORTFOLIO REVIEW" }
local columns = {}
for k, line in info do
	table.insert(columns, Group { x = cw * (COLS - 1) + 12 + (k - 1) * 22, y = H - 24, rotate = -90,
		Text { font = "Courier New", weight = 700, size = 11, y = 9, fill = INK, line },
	})
end

return Canvas { size = { W, H }, bg = "#efebe2",
	Grain { amount = 0.1, size = 1,
		gridLines,
		dots,
		Rough { amount = 0.9, freq = 2.5, titles },
		Displace { amount = 5, freq = 18, seed = random(0, 100),
			Rough { amount = 1.2, freq = 1.6,
				Path { d = scribble(7), fill = "none", stroke = ACCENT, strokeWidth = 12, cap = "round", blend = "multiply" },
			},
		},
		columns,
		Rect { x = 0, y = 0, w = W, h = rh * 1.4, fill = INK },
		Text { font = "DIN Condensed", weight = 700, size = 64, x = 18, y = 74, fill = "#efebe2", "ORGD 2026" },
		Text { font = "Courier New", size = 11, x = W - 18, y = 43, anchor = "end", fill = "#efebe2", "OPEN RESEARCH\nGRAPHIC DESIGN" },
	},
}
