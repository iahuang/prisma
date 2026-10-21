-- One case per `-p case=...` for prisma's explicit (no implicit cast or
-- default-by-context) rules: the "ok" cases render something a test measures,
-- the rest must fail with a clear error.
local shape = Circle { cx = 50, cy = 50, r = 30 }
local cases = {
	-- params stay strings; param() reads one as its default's type
	params = function()
		assert(params.n == "007" and type(params.n) == "string", "params.n should be the string typed")
		return Rect { x = 0, y = 0, w = param("n", 3) * 10, h = 10, fill = "#000" }
	end,
	param_number = function() return Rect { w = param("n", 3), h = 1 } end,
	param_required = function() return Rect { w = param("missing"), h = 1 } end,
	-- a stroke doesn't remove the default fill
	fill = function() return Circle { cx = 50, cy = 50, r = 30, stroke = "#00f", strokeWidth = 4 } end,
	-- opacity fades the composited shape: the stroke hides the fill under it
	opacity = function()
		return Circle { cx = 100, cy = 50, r = 30, fill = "#f00", stroke = "#00f", strokeWidth = 20, opacity = 0.5 }
	end,
	anchor = function() return Text { x = 100, y = 70, size = 40, anchor = "middle", "HHH" } end,
	opacity_range = function() return Circle { cx = 50, cy = 50, r = 30, opacity = 1.5 } end,
	blur_negative = function() return Circle { cx = 50, cy = 50, r = 30, blur = -2 } end,
	line_fill = function() return Line { x1 = 0, y1 = 0, x2 = 10, y2 = 10, fill = "#f00" } end,
	align_unboxed = function() return Text { x = 10, y = 50, align = "right", "x" } end,
	anchor_boxed = function() return Text { x = 10, y = 50, w = 80, anchor = "end", "x" } end,
	font_face = function() return Text { x = 10, y = 50, font = "Courier New Bold", "x" } end,
	font_path = function() return Text { x = 10, y = 50, font = "fonts/Inter.ttf", "x" } end,
	font_both = function() return Text { x = 10, y = 50, font = "Helvetica", fontFile = "a.ttf", "x" } end,
	pad = function()
		local Fx = effect { pad = 4, wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }
		return Fx { shape }
	end,
	reach_negative = function()
		local Fx = effect { uniforms = { r = { "length", 4 } }, reach = "r", wgsl = "fn effect(p: vec2f) -> vec4f { return src(p); }" }
		return Fx { r = -3, shape }
	end,
}
return Canvas { size = { 200, 100 }, bg = "#fff", cases[param("case")]() }
