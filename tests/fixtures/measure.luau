-- measure() agrees with how Text is placed. Asserts fail the render; the test
-- then checks pixels: "HH" fills its measured box and a blue marker starts
-- exactly at its advance end.
local function near(a, b, what) assert(math.abs(a - b) < 0.01, `{what}: {a} vs {b}`) end

local hh = Text { x = 40, y = 100, size = 80, font = "Helvetica", "HH" }
local m = measure(hh)
near(m.x, 40, "start x")
near(m.baseline, 100, "baseline")
assert(m.ascent > 0 and m.descent > 0, "ascent and descent are positive")
near(m.y, 100 - m.ascent - (m.h - m.ascent - m.descent) / 2, "box top")
assert(#m.lines == 1, "one line")
assert(m.ink.x > m.x and m.ink.x + m.ink.w < m.x + m.w, "H's ink sits inside its side bearings")
near(m.ink.y + m.ink.h, 100, "H sits on the baseline")

-- anchor = "end" places the same advance box to end at x
local e = measure(Text { x = 300, y = 100, size = 80, font = "Helvetica", anchor = "end", "HH" })
near(e.x + e.w, 300, "end anchor")
near(e.w, m.w, "same width")

-- wrapped and centered lines sit inside the box, each centered
local b = measure(Text { x = 10, y = 200, w = 120, size = 20, font = "Helvetica", align = "center", "one two three four five" })
assert(#b.lines > 1, "wraps")
for _, l in b.lines do
	near(l.x - 10, 130 - (l.x + l.w), "centered line")
end

return Canvas { size = { 400, 150 }, bg = "#fff",
	hh,
	Rect { x = m.x + m.w, y = 60, w = 20, h = 20, fill = "#00f" },
}
