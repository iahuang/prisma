-- transformPath and pathBounds agree with each other, and an effect written in
-- doc px (to_doc / src_doc) lands in the same place at any --scale.
local function near(a, b, what) assert(math.abs(a - b) < 1e-6, `{what}: {a} vs {b}`) end
local function box(b, x, y, w, h, what)
	near(b.x, x, what .. " x"); near(b.y, y, what .. " y"); near(b.w, w, what .. " w"); near(b.h, h, what .. " h")
end

local tri = "M10,20 L50,20 L50,80 Z"
box(pathBounds(tri), 10, 20, 40, 60, "bounds")
assert(pathBounds("") == nil, "empty path has no bounds")
-- relative commands and arcs come back as absolute path data
box(pathBounds(transformPath("m10,20 h40 v60 z", {})), 10, 20, 40, 60, "relative commands")
local b = pathBounds("M0,0 A10,10 0 1,0 20,0 A10,10 0 1,0 0,0 Z")
assert(math.abs(b.w - 20) < 0.01 and math.abs(b.h - 20) < 0.01, `circle bounds are tight: {b.w}x{b.h}`)

box(pathBounds(transformPath(tri, { x = 5, y = -20 })), 15, 0, 40, 60, "translate")
box(pathBounds(transformPath(tri, { scale = { 2, 0.5 } })), 20, 10, 80, 30, "scale")
box(pathBounds(transformPath(tri, { rotate = 90 })), -80, 10, 60, 40, "rotate")
box(pathBounds(transformPath(tri, { 2, 0, 0, 0.5, 1, 1 })), 21, 11, 80, 30, "matrix")
box(pathBounds(transformPath(tri, { transform = { 1, 0, 0, 1, "1mm", 0 } })), 10 + 96 / 25.4, 20, 40, 60, "transform key")

-- fit a glyph's outline into a box, stretching it
local d = outline(Text { font = "Helvetica", size = 50, "O" })
local g = pathBounds(d)
local sx, sy = 60 / g.w, 40 / g.h
local fitted = transformPath(d, { sx, 0, 0, sy, 20 - g.x * sx, 20 - g.y * sy })
box(pathBounds(fitted), 20, 20, 60, 40, "fitted")

local Shift = effect {
	name = "shift",
	reach = 20,
	wgsl = "fn effect(p: vec2f) -> vec4f { return src_doc(to_doc(p) + vec2f(20.0, 0.0)); }",
}

return Canvas { size = { 220, 160 }, bg = "#fff",
	Path { d = transformPath("M0,0 H1 V1 H0 Z", { 60, 0, 0, 40, 20, 20 }) },
	Path { d = fitted, fill = "none", stroke = "#000" },
	Shift { area = { 100, 100, 100, 50 }, Rect { x = 150, y = 110, w = 20, h = 30, fill = "#f00" } },
}
