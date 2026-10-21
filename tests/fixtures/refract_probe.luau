-- Refract and Gloss over a band whose sides are 45-degree chamfers: with
-- thickness = bevel = 40 and curve = 1 the height rises from 0 at x = 50 to 40
-- at x = 90, is flat to x = 210 and falls to 0 at x = 250 (the band runs far past
-- the canvas vertically, so only its sides matter).
--   case=refract  over a ramp whose red channel is x (one level per doc px), so
--                 a pixel's red says where it read the backdrop
--   case=gloss    a light straight overhead (elevation 90) with shininess 2 over black
local W = 300
local body = Sdf { range = 44, Rect { x = 50, y = -100, w = 200, h = 300 } }
local slab = { body = body, thickness = 40, bevel = 40, curve = 1 }
local fx
if param("case", "refract") == "refract" then
	slab.ior = 1.5
	slab[1] = Rect { w = W, h = 100, fill = linear({ 0, 0 }, { 255, 0 }, { "#000", "#f00" }) }
	fx = Refract(slab)
else
	slab.elevation, slab.shininess, slab.color, slab.area = 90, 2, "#0f0", { 0, 0, W, 100 }
	fx = Gloss(slab)
end
return Canvas { size = { W, 100 }, bg = "#000", fx }
