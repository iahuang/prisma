-- A named effect whose code depends on a param: every sweep cell (and every
-- watch re-render) must be able to redefine it.
local v = param("v", 0.5)
local Solid = effect { name = "solid", wgsl = `fn effect(p: vec2f) -> vec4f \{ return vec4f({v}, 0.0, 0.0, 1.0); }` }
return Canvas { size = { 10, 10 }, Solid { area = { 0, 0, 10, 10 } } }
