//! Every piece of UI text, in one place so it can be translated later.

// ---- asset library (objects, materials, skies, lamps) ----
pub const TAB_PLANTS: &str = "Plants";
pub const TAB_OBJECTS: &str = "Objects";
pub const TAB_LIGHTS: &str = "Lights";
pub const RECENT: &str = "Recent";
pub const LIBRARY_LOADING: &str = "The asset library is loading, or was not found.";
pub const LIGHTS_BUTTON: &str = "Lights";
pub const LIGHTS_SUGGEST: &str =
    "The sun is down. Switch the lights on with the Lights button, top right.";
pub const LIGHTS_DROPPED: &str = "lights left out: too many to draw";
pub const LIGHTS_HINT: &str = "Click to place a light, click a light to select it. A spot is aimed by dragging its white handle. Delete removes the selected light.";
pub const LIGHT_POINT: &str = "Point";
pub const LIGHT_SPOT: &str = "Spot";
pub const KELVIN: &str = "Colour temperature (K)";
pub const LUMENS: &str = "Lumens";
pub const RANGE: &str = "Range (m)";
pub const LIGHT_HEIGHT: &str = "Height (m)";
pub const CONE_INNER: &str = "Inner cone";
pub const CONE_OUTER: &str = "Outer cone";
pub const TAB_PROJECT: &str = "Project";
pub const TAB_LIBRARY: &str = "Library";
pub const SURFACE: &str = "Surface";
pub const SURFACE_KINDS: [&str; 4] = ["Standard", "Glass", "Water", "Lawn"];
pub const VARIATION: &str = "Variation";
pub const KIND_ALL: &str = "All";
pub const SKY: &str = "Sky";
pub const SKY_PHYSICAL: &str = "Physical";
pub const SKY_ROTATION: &str = "Rotation";
pub const SKY_INTENSITY: &str = "Brightness";
pub const SKY_USE_SUN: &str = "Use this sky's sun";
pub const SKY_BACK_TO_DATE: &str = "Back to date and time";
pub const SKY_SUN_FROM_IMAGE: &str = "The sun comes from the sky image.";
pub const SKY_NO_SUN: &str = "This sky shows no sun (overcast): the sun stays as set.";
pub const GROUND_COLOR: &str = "Ground colour";

pub const APP_NAME: &str = "VisionCraft";

pub const OPEN: &str = "Open";
pub const SAVE: &str = "Save";
pub const SAVED: &str = "Saved";
pub const UNDO: &str = "Undo";
pub const REDO: &str = "Redo";
pub const LAYERS: &str = "Layers";
pub const CLAY: &str = "Clay view";
pub const QUALITY: &str = "Quality";
pub const WALK: &str = "Walk";
pub const FPS: &str = "fps";
pub const TARGET_FPS: &str = "Target fps";
pub const MODEL_DISTANCE: &str = "Model distance";
pub const MODEL_DISTANCE_TIP: &str = "How far library objects (benches, pots, lamps; not plants or the design itself) are drawn and loaded, in metres: Preset follows the quality (80 m Low, 150 m Medium, 300 m High, no limit Ultra); past it they fade out";
pub const PRESET: &str = "preset";
pub const UNLIMITED: &str = "unlimited";
pub const TARGET_FPS_TIP: &str = "The frame rate Auto aims for: it lowers detail when frames miss it and raises it with room to spare";
pub const HIDE_FPS: &str = "Hide fps";
pub const SHOW_FPS: &str = "Show fps";
pub const LOADING: &str = "Loading…";
pub const MODELS: &str = "Models";
pub const RELOAD_MODEL: &str = "Reload model";
pub const RELINK: &str = "Relink model";
pub const MODEL_UPDATED: &str = "Model updated";
pub const DROP_HINT: &str = "Or drop a .3dcraft, .obj, .glb or project file here";
pub const START_TITLE: &str = "Start a garden";
pub const START_DRAW: &str = "Draw a garden";
pub const START_DRAW_TIP: &str = "Draw the plot, the house and the beds yourself";
pub const START_DRAW_HINT: &str =
    "Click a corner, then type 20, Tab, 30 and Enter for a 20 by 30 m plot";
pub const START_MODEL: &str = "Open a 3dcraft model";
pub const START_MODEL_TIP: &str = "A house or garden designed in 3dcraft (.3dcraft or .obj)";
pub const START_EXAMPLE: &str = "Open the example garden";
pub const START_EXAMPLE_TIP: &str =
    "A small garden with a house, terrace, lawn, hedge, border, trees, a pond and lamps";
pub const EXAMPLE_NAME: &str = "Example garden";
pub const START_ESTATE: &str = "Open the estate";
pub const START_ESTATE_TIP: &str =
    "A large plot round an imported villa, with two lawns, a woodland, a border and a pond";
pub const ESTATE_NAME: &str = "Example estate";
pub const START_BEACH_HOUSE: &str = "Open the beach house";
pub const START_BEACH_HOUSE_TIP: &str =
    "A furnished single-storey house with a deck and carport, inside and out, on sand";
pub const BEACH_HOUSE_NAME: &str = "Beach house";
pub const START_COAST: &str = "Open the coast";
pub const START_COAST_TIP: &str =
    "The beach house on a bluff above a sandy beach, with palms and the sea to the horizon";
pub const COAST_NAME: &str = "Coast";
pub const VIEW_DISTANCE: &str = "View distance";
pub const VIEW_DISTANCE_TIP: &str =
    "How far plants keep their detail, shadows reach and the air stays clear (1 is the default)";
pub const WALK_HINT: &str = "Walking: WASD or arrows move, Shift runs, drag to look, Space to exit";

pub const MODE_LABELS: [(&str, &str); 5] = [
    ("Draw", "Draw"),
    ("Materials", "Mat."),
    ("Objects", "Obj."),
    ("Landscape & Sun", "Sun"),
    ("Viewpoints", "Views"),
];

// Draw
pub const TOOL_SELECT: &str = "Select";
pub const TOOL_AREA: &str = "Area";
pub const TOOL_BLOCK: &str = "Block";
pub const TOOL_WALL: &str = "Wall";
pub const TOOL_FENCE: &str = "Fence";
pub const TOOL_STAIRS: &str = "Stairs";
pub const TOOL_BED: &str = "Raised bed";
pub const TOOL_TERRACE: &str = "Terrace";
pub const TERRACE_HINT: &str = "Draw the outline of a level to cut into the slope: click the corners, Enter or the first corner closes it. The floor is the mean of the ground under it, or a height you set; a retaining wall holds the cut. Esc cancels.";
pub const TERRACE_FLOOR: &str = "Floor";
pub const TERRACE_FLOOR_MEAN: &str = "Floor at the mean ground";
pub const TERRACE_WALL: &str = "Wall";
pub const TERRACE_PARAPET: &str = "Parapet";
pub const TERRACE_COPING: &str = "Coping";
pub const TERRACE_MAKE: &str = "Make it a terrace (cut into the ground)";
pub const TERRACE_CLEAR: &str = "Back to a plain area";
pub const TERRACE_CUT: &str = "Cut";
pub const TERRACE_FILL: &str = "Fill";
pub const TOOL_FURNITURE: &str = "Furniture";
pub const FURNITURE_HINT: &str = "Pick a piece or a set, then click on the plan to place it. Sizes at 0 are the piece's own. Esc leaves the tool.";
pub const FURNITURE_PIECE: &str = "Piece";
pub const FURNITURE_LENGTH: &str = "Length";
pub const FURNITURE_DEPTH: &str = "Depth";
pub const FURNITURE_TURN: &str = "Turn";
pub const FURNITURE_SEATS: &str = "Seats";
pub const FURNITURE_KIND: &str = "Kind";
pub const FURNITURE_ROUND: &str = "Round";
pub const FURNITURE_BACK: &str = "With a back";
pub const FURNITURE_BACK_DEG: &str = "Back angle";
pub const FURNITURE_OPEN: &str = "Open";
pub const FURNITURE_CUSHIONS: &str = "Cushions";
pub const FURNITURE_DOORS: &str = "Doors";
pub const FURNITURE_DRAWERS: &str = "Drawer rows";
pub const FURNITURE_PEDESTAL: &str = "Pedestal";
pub const TOOL_CIRCLE: &str = "Circle";
pub const CIRCLE_HINT: &str =
    "Click the centre, then the edge of the circle. Type the radius and press Enter. Esc cancels.";
pub const TOOL_ELLIPSE: &str = "Ellipse";
pub const ELLIPSE_HINT: &str =
    "Click the centre, then a corner of the box round the ellipse. Esc cancels.";
pub const CURVE_EDGE: &str = "Edge";
pub const CURVE_HINT: &str = "Shift-click an edge to pick it; a plain click on an edge adds a corner. Drag the green dot in the middle of an arc to bend it.";
pub const CURVE_STRAIGHT: &str = "straight";
pub const CURVE_RADIUS: &str = "Radius";
pub const CURVE_LEFT: &str = "Curve left";
pub const CURVE_RIGHT: &str = "Curve right";
pub const CURVE_CLEAR: &str = "Straighten";
pub const SMOOTH: &str = "Smooth";
pub const SMOOTH_TIP: &str = "A smooth curve through all the corners.";
pub const STAIRS_HINT: &str = "Click the bottom of the steps, then the top. Change the rise, riser and tread in the panel. Esc cancels.";
pub const STAIRS_TURN: &str = "Turn (L or U)";
pub const STAIRS_TURN_HINT: &str = "Click the bottom, then each corner and the top; every segment is held at a right angle to the one before. Enter ends at three points, the fourth click ends a U. Esc cancels.";
pub const FLIGHT: &str = "Flight";
pub const LANDING: &str = "Landing";
pub const UP: &str = "up";
pub const STRAIGHTEN_STAIRS: &str = "Straighten";
pub const MAKE_STAIRS: &str = "Make it steps";
pub const STEPS: &str = "steps";
pub const RISER: &str = "Riser";
pub const TREAD: &str = "Tread";
pub const FIT_TREADS: &str = "Fit the treads to the footprint";
pub const STAIRS_SET_IN: &str = "Set into the slope";
pub const STAIRS_CHEEKS: &str = "Cheeks";
pub const STAIRS_GOINGS: &str = "Goings";
pub const BACK_TO_BLOCK: &str = "Back to a block";
pub const WALL: &str = "Wall";
pub const BED_PLAIN: &str = "Plain";
pub const BED_MASONRY: &str = "Masonry";
pub const BED_SLEEPERS: &str = "Sleepers";
pub const BED_CORTEN: &str = "Corten";
pub const BED_GABION: &str = "Gabion";
pub const EDGE: &str = "Edge";
pub const EDGE_NONE: &str = "None";
pub const EDGE_NEEDS_FLAT: &str =
    "A raised edge needs the area to stand at its own height: switch off Follow the ground first.";
pub const MAKE_FENCE: &str = "Make it a fence";
pub const FENCE_KIND: &str = "Fence";
pub const FENCE_SLATS: &str = "Slats";
pub const FENCE_HORIZONTAL: &str = "Boards";
pub const FENCE_PANELS: &str = "Panels";
pub const FENCE_PICKET: &str = "Picket";
pub const FENCE_MESH: &str = "Mesh";
pub const FENCE_SCREEN: &str = "Screen";
pub const FENCE_SLOPE: &str = "On a slope";
pub const FENCE_LEVEL: &str = "Level";
pub const FENCE_STEPPED: &str = "Stepped panels";
pub const FENCE_RAKED: &str = "Raked";
pub const FENCE_GRAVEL_BOARD: &str = "Gravel board";
pub const ADD_GATE: &str = "Add a gate";
pub const REMOVE_LAST_GATE: &str = "Remove the last gate";
pub const BACK_TO_WALL: &str = "Back to a wall";
pub const TOOL_WINDOW: &str = "Window";
pub const TOOL_DOOR: &str = "Door";
pub const OPENING_HINT: &str =
    "Click a wall of a block to put it there; Esc goes back to selecting.";
pub const SILL: &str = "Sill";
pub const FRAME: &str = "Frame";
pub const FRAME_SLIM: &str = "Slim";
pub const FRAME_WIDE: &str = "Wide";
pub const FRAME_SPLIT: &str = "Two panes";
pub const FRAME_NONE: &str = "None";
pub const ROOF: &str = "Roof";
pub const ROOF_GABLE: &str = "Gable";
pub const ROOF_HIP: &str = "Hip";
pub const ROOF_LEAN_TO: &str = "Lean-to";
pub const ROOF_FLAT: &str = "Flat";
pub const ADD_ROOF: &str = "Add a roof";
pub const PITCH: &str = "Pitch";
pub const OVERHANG: &str = "Overhang";
pub const PLINTH: &str = "Plinth";
pub const OPENINGS: &str = "windows and doors";
pub const REMOVE_LAST_OPENING: &str = "Remove the last";
pub const LIGHT_FIXTURES: &str = "Garden lamps (click one, then click where it goes)";
pub const BARE_LIGHTS: &str = "Bare lights";
pub const IN_BLOOM_NOW: &str = "in bloom";
pub const PLAN_VIEW: &str = "Plan";
pub const PLAN_VIEW_TIP: &str =
    "Look straight down, as in Draw: what you place lands exactly under the pointer";
pub const STOREYS: &str = "Storeys";
pub const AREAS_DRAPED: &str =
    "areas now follow the ground (they were drawn before the terrain); undo puts them back";
pub const FOLLOW_GROUND: &str = "Follow the ground";
pub const FOLLOW_GROUND_TIP: &str = "On: the area lies on the terrain, its elevation is above the ground. Off: flat at its elevation (a raised deck, water)";
pub const KEEP_INSIDE: &str = "Keep inside area";
pub const KEEP_INSIDE_TIP: &str = "Plants stay inside the area (border, lawn) the stroke starts on";
pub const INDOOR_LIGHTS: &str = "Indoor and wall lights";
pub const PT_PRESET: &str = "Mix";
pub const PT_MORE: &str = "Size, turning, layer";
pub const PANEL_FOLD: &str = "Fold the panel to its title, to see and click what is behind it";
pub const PANEL_UNFOLD: &str = "Unfold the panel";
pub const PT_STRAIGHT: &str = "Straight (corner to corner)";
pub const WIDTH: &str = "Width";
pub const DEPTH: &str = "Depth";
pub const AIR_AND_GROUND: &str = "Haze, wind, ground plane";
pub const AVOID_BUILDINGS: &str =
    "Avoid buildings (no plants on roofs, in blocks or under a low canopy)";
pub const IN_BUILDING: &str = "That would put it inside a building or under a roof lower than the plant: hold Shift to place it anyway";
pub const KIND_LAWN: &str = "Lawn";
pub const KIND_TERRACE: &str = "Terrace";
pub const KIND_PATH: &str = "Path";
pub const KIND_BORDER: &str = "Border";
pub const KIND_WATER: &str = "Water";
pub const DRAW_HINT: &str = "Click to add points. Click the first point or press Enter to close, Esc cancels, Backspace undoes a point. Alt turns the grid snap off.";
pub const WALL_HINT: &str = "Click to add points. Enter finishes, Esc cancels, Backspace undoes a point. Alt turns the grid snap off.";
pub const SELECT_HINT: &str = "Click a shape to select it. Drag a corner to move it, click an edge to add a corner, Delete removes the selected corner.";
pub const NAME: &str = "Name";
pub const HEIGHT: &str = "Height";
pub const BASE: &str = "Base";
pub const THICKNESS: &str = "Thickness";
pub const ELEVATION: &str = "Elevation";
pub const AREA: &str = "Area";
pub const PERIMETER: &str = "Perimeter";
pub const DELETE_SHAPE: &str = "Delete shape";
pub const DELETE_CORNER: &str = "Delete corner";

// Materials
pub const THIS_FACE: &str = "This face";
pub const WHOLE_BODY: &str = "Whole body";
pub const WHOLE_OBJECT: &str = "Whole object";
pub const EYEDROPPER: &str = "Eyedropper";
pub const MAP_SCALE: &str = "Map scale";
pub const RELIEF: &str = "Relief";
pub const GLOSS: &str = "Gloss";
pub const WEATHERING: &str = "Grime tint";
pub const WEATHERING_SECTION: &str = "Weathering";
pub const WEATHERING_TIP: &str = "Dirt, moss, wear and rain streaks on top of the surface; the amount scales the rest. Not on glass, water or lawns.";
pub const WEATHER_AMOUNT: &str = "Amount";
pub const WEATHER_DIRT: &str = "Dirt";
pub const WEATHER_MOSS: &str = "Moss";
pub const WEATHER_WEAR: &str = "Wear";
pub const WEATHER_STREAKS: &str = "Streaks";
pub const AGE: &str = "Age";
pub const AGE_TIP: &str = "Weathers every material at once: dirt, moss, wear and rain streaks, added to each material's own weathering. Years after planting does not change it.";
pub const RAIN_SECTION: &str = "Weather";
pub const WETNESS: &str = "Wetness";
pub const WETNESS_TIP: &str = "Wet surfaces: porous materials darken and turn glossy, and above about half, puddles form on flat open paving and soil. Rain implies it.";
pub const RAIN: &str = "Rain";
pub const RAIN_TIP: &str = "Falling rain slanted by the wind, rings on puddles and ponds, an overcast sky and a weaker sun; everything is wet.";
pub const MIST: &str = "Mist";
pub const MIST_TIP: &str = "A low mist over the ground: the distance fades into a white or grey veil, the near garden only softens, the sun glows through it and weakens. 0.3 a light morning haze, 0.6 a misty morning, 1 fog. Rain brings its own.";
pub const GUSTS: &str = "Gusts";
pub const GUSTS_TIP: &str = "Gusts of wind travelling downwind: they roll across the lawn as waves of bending, paler grass, sway one crown after another and swing the rain. Stronger with the wind speed.";
pub const SKY_OCCLUSION: &str = "Sky occlusion";
pub const SKY_OCCLUSION_TIP: &str = "Light every surface by the sky it sees: the ground under trees and the corners by walls go gently darker, backlit crowns stay green inside, and wet paving under a tree stops mirroring the open sky. Baked again when the design changes.";
pub const SHAFTS: &str = "Light shafts";
pub const SHAFTS_TIP: &str = "Beams of sunlight through the air where crowns, hedges and houses shadow it, strongest looking towards a low sun. They need something in the air to light: mist (0.2 to 0.4), rain or a thick haze.";
pub const WHITE_BALANCE: &str = "White balance";
pub const WHITE_BALANCE_TIP: &str = "Daylight keeps a low sun orange and shade blue, as on film. Auto takes most of the colour of the light out, as a camera does, and keeps some of the mood. A temperature makes light of that colour read white.";
pub const WHITE_BALANCES: [(&str, &str); 3] = [
    ("daylight", "Daylight"),
    ("auto", "Auto"),
    ("kelvin", "Kelvin"),
];
pub const GRADE: &str = "Grade";
pub const GRADE_TIP: &str = "A colour grade before the picture is developed: warm, cool, or soft (lower contrast and colour).";
pub const GRADES: [(&str, &str); 4] = [
    ("neutral", "Neutral"),
    ("warm", "Warm"),
    ("cool", "Cool"),
    ("soft", "Soft"),
];
pub const MATERIALS_HINT: &str =
    "Hover a surface and click it to choose a material. Drop a .mcmat file to add one.";
pub const MATERIAL_ADDED: &str = "Material added";

// Objects
pub const DENSITY: &str = "Density per m²";
pub const PLANTINGS: &str = "Placed";
pub const SEASON: &str = "Season";
pub const MOWING_STRIPES: &str = "Mowing stripes";
pub const MOWING_STRIPES_TIP: &str =
    "Bands of grass leaning the way the mower went, lighter and darker by turns";
pub const STRIPE_WIDTH: &str = "Width";
pub const STRIPE_DIRECTION: &str = "Direction";
pub const VIEW_KEEP_TIME: &str = "Keep the time of this view";
pub const VIEW_KEEP_TIME_TIP: &str = "This view shows its own date and time (a dusk view, a sun study); otherwise it shows the project's season and time";
pub const SEASONS: [&str; 4] = ["Spring", "Summer", "Autumn", "Winter"];
pub const SUN_MAP: &str = "Sun hours map";
pub const SUN_MAP_SHOW: &str = "Show on the ground";
pub const SUN_MAP_TIP: &str = "Colours the ground by the hours of direct sun it gets on the day: shadows of the house, trees and hedges are cast from the sun's path (clouds are not counted). Flat at the area's height, also on terrain.";
pub const SUN_MAP_AREA: &str = "Area";
pub const SUN_MAP_PLOT: &str = "Whole plot";
pub const SUN_MAP_DAY: &str = "Day";
pub const SUN_MAP_PROJECT_DATE: &str = "Project date";
pub const SUN_MAP_OPACITY: &str = "Opacity";
pub const SUN_MAP_YEARS: &str = "Years after planting";
pub const SUN_MAP_YEARS_TIP: &str =
    "How far the crowns have grown: the same setting as the years in the sun panel";
pub const SUN_MAP_WORKING: &str = "Working out the sun...";
pub const SUN_MAP_LEGEND: &str = "Hours of sun";
pub const SUN_MAP_FULL: &str = "Full sun";
pub const SUN_MAP_PART: &str = "Part shade";
pub const SUN_MAP_SHADE: &str = "Shade";
pub const LIGHT_FOR: &str = "Plants for this light";
pub const LIGHT_NEEDS_MAP: &str = "Show the map on an area to list the plants that suit its light.";
pub const LIGHT_KIND: &str = "Kind";
pub const LIGHT_ALL: &str = "All";
pub const LIGHT_NONE: &str = "No plant suits this light.";
pub const LIGHT_ESTIMATED: &str = "Light needs are estimated, not yet checked against open data.";
pub const LIGHT_CHECK: &str = "Check plant light";
pub const LIGHT_CHECK_TIP: &str = "Measures the sun at every placed plant on the map's day (else the summer solstice) and lists the plants in too much or too little light, with species that would suit.";
pub const LIGHT_MARKS: &str = "Ring on plan";
pub const LIGHT_STALE: &str = "The project changed since this check.";
pub const SUN_MAP_NO_SHAPES: &str = "Draw an area or a house first";
pub const SNOWY: &str = "Snowy";
pub const SNOWY_TIP: &str = "Winter under snow";
pub const SNOW: &str = "Snow";
pub const SNOW_TIP: &str = "How much snow lies on the ground, roofs, hedges and branches";
pub const EXACT_DATE: &str = "Exact date";
pub const EXACT_DATE_TIP: &str =
    "A day of the year for sun studies; the plants show the season of that day";
pub const PT_HIDDEN: &str = "\u{1F441} hidden";
pub const PT_HIDDEN_TIP: &str =
    "Its layer is hidden: the plants are kept, not drawn. Tick the box to show the layer.";
pub const EMPTY_DENSITY: &str = "0 plants: too few for this density over this stroke or area (paint longer or raise the density)";
pub const EMPTY_OUTSIDE: &str = "0 plants: every spot lies outside the area it keeps inside";
pub const EMPTY_BUILDINGS: &str =
    "0 plants: every spot is on a roof, inside a block or model, or under a low canopy";
pub const REMOVE: &str = "Remove";

// Sun
pub const DATE: &str = "Date";
pub const TIME: &str = "Time";
pub const LATITUDE: &str = "Latitude";
pub const LONGITUDE: &str = "Longitude";
pub const UTC_OFFSET: &str = "UTC offset";
pub const CITY: &str = "City";
pub const EXPOSURE: &str = "Exposure";
pub const SUN_HINT: &str =
    "Drag the sun around the compass for the time of day; Exact date opens the month dial.";
pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const CITIES: [(&str, f64, f64, f64); 6] = [
    ("Brussels", 50.85, 4.35, 1.0),
    ("Antwerp", 51.22, 4.40, 1.0),
    ("Ghent", 51.05, 3.72, 1.0),
    ("Amsterdam", 52.37, 4.90, 1.0),
    ("Paris", 48.86, 2.35, 1.0),
    ("London", 51.51, -0.13, 0.0),
];

// Viewpoints
pub const SAVE_VIEW: &str = "Save current view";
pub const UPDATE_VIEW: &str = "Update from current view";
pub const RENAME: &str = "Rename";
pub const DELETE: &str = "Delete";

pub const UNSAVED_PROJECT: &str = "Untitled";
pub const RELINK_PROMPT: &str = "needs its file";

pub const KEEP: &str = "Keep";
pub const SCALE: &str = "Scale";
pub const ROTATION: &str = "Rotation";
pub const DROP_TO_GROUND: &str = "Drop to ground";
pub const PERSON: &str = "Person (1.8 m)";
pub const PLACE_PERSON: &str = "Place person";
pub const SUMMER_TIME: &str = "Summer time follows the date";
pub const EXPOSURE_AUTO: &str = "Auto exposure";
pub const LOOK: &str = "Look";
pub const YEARS: &str = "Years";
pub const COMPARE: &str = "Compare";
pub const VIEW_YEARS: &str = "Years after planting";
pub const COMPARE_TIP: &str = "A wipe between the view as planted and the view at the years set; drag the handle. Click again to close.";
pub const COMPARE_PLANTED: &str = "As planted";
pub const COMPARE_YEAR: &str = "Year";
pub const SPACING_HINT_LINE: &str = "Grown, these close up planted every";
pub const SPACING_HINT_BED: &str = "Grown, these fill a bed at about";
pub const SPACING_HINT_TIP: &str = "From the widest plant's mature width (estimated growth data); set the years after planting in Sun & sky to see them grow.";
pub const YEARS_GROW: &str = "Show growth";
pub const YEARS_TIP: &str = "Years after planting: plants grow from their planting age along their species' curve. Library plants change form as they grow; catalogue models only scale.";
pub const YEARS_CONFLICTS: &str = "Growing into each other";
pub const CLOUDS: &str = "Clouds";
pub const CLOUD_KINDS: [(&str, &str); 4] = [
    ("clear", "Clear"),
    ("cumulus", "Fair"),
    ("overcast", "Overcast"),
    ("storm", "Storm"),
];
pub const WEATHER: &str = "Weather";
pub const WEATHER_TIP: &str = "Sets the clouds, rain, wetness, mist, gusts and wind together; the view eases into the new weather over a few seconds.";
pub const WEATHERS: [(&str, &str); 6] = [
    ("clear", "Clear"),
    ("fair", "Fair"),
    ("cloudy", "Cloudy"),
    ("overcast", "Overcast"),
    ("rain", "Rain"),
    ("storm", "Storm"),
];
/// How long the window takes to ease into a weather picked in the panel (s).
pub const WEATHER_PANEL_TRANSITION_S: f32 = 8.0;
pub const VOLUMETRIC_CLOUDS: &str = "Volumetric";
pub const VOLUMETRIC_CLOUDS_TIP: &str = "Volumetric clouds lit by the sun and the sky, whose shadows drift over the ground (Medium quality and up); off: the flat cloud layer.";
pub const CLOUD_COVER: &str = "Cover";
pub const LOOKS: [(&str, &str); 3] = [
    ("neutral", "Neutral"),
    ("natural", "Natural"),
    ("vivid", "Vivid"),
];
pub const HAZE: &str = "Haze";
pub const WIND: &str = "Wind speed (m/s)";
pub const WIND_GUSTINESS: &str = "Gustiness";
pub const WIND_DIRECTION: &str = "Wind direction";
pub const GROUND: &str = "Ground plane";
pub const EXPORT: &str = "Export";
pub const SCREENSHOT: &str = "Screenshot";
pub const STILL_TEXTURES_LATE: &str = "Captured before every higher-resolution texture had arrived";
pub const TEXTURES: &str = "Textures";
pub const TEXTURES_TIP: &str = "Resolution of the library materials, the sky and the nearest object models: Auto follows the quality (2k at High, 4k at Ultra); higher tiers load on demand and fall back to the base files when memory runs short";
pub const MEASURING: &str = "Measuring this GPU…";
pub const REMEASURE: &str = "Re-measure";
pub const REMEASURE_TIP: &str =
    "Time this GPU again and start Auto from the quality it can carry at this window size";
pub const SCREENSHOT_TIP: &str =
    "Save the 3D view as a lossless PNG at the screen's full resolution, without the panels";
pub const AUTO: &str = "auto";
pub const REPLACE_PROJECT: &str = "Replace the current project? Unsaved changes are lost.";
pub const REPLACE_ANYWAY: &str = "Replace";
pub const CANCEL: &str = "Cancel";
pub const SEARCH: &str = "Search";
pub const EVERGREEN: &str = "Evergreen";
pub const IN_BLOOM: &str = "In bloom now";

// ---- Planting tools (Objects mode) ----
pub const PT_SELECT: &str = "Select";
pub const PT_SINGLE: &str = "Single";
pub const PT_LINE: &str = "Line";
pub const PT_CLUSTER: &str = "Cluster";
pub const PT_AREA: &str = "Area";
pub const PT_SURFACE: &str = "Surface";
pub const PT_BRUSH: &str = "Brush";
pub const PT_SIMPLE: &str = "Simple";
pub const PT_ADD_PLANTS: &str = "+ Add plants or objects";
pub const PT_SCALE: &str = "Scale";
pub const PT_ROTATION: &str = "Rotation";
pub const PT_RANDOM_ROTATION: &str = "Random rotation";
pub const PT_MIN_DISTANCE: &str = "Min distance";
pub const PT_KEEP_CLEAR: &str = "Keep clear";
pub const PT_COUNT: &str = "Count";
pub const PT_SPACING: &str = "Every";
pub const PT_JITTER: &str = "Jitter";
pub const PT_OFFSET: &str = "Offset";
pub const PT_CLOSED: &str = "Closed";
pub const PT_RADIUS: &str = "Radius";
pub const PT_MAX_SLOPE: &str = "Max slope";
pub const PT_RESHUFFLE: &str = "Reshuffle";
pub const PT_RESHUFFLE_HINT: &str = "A new random seed: the same plants, placed differently";
pub const PT_LAYER: &str = "Layer:";
pub const PT_ERASE: &str = "Erase";
pub const PT_EXTENDING: &str = "Extending";
pub const PT_NEW_BRUSH: &str = "New brush";
pub const PT_NEW_BRUSH_NEXT: &str = "The next stroke starts a new brush";
pub const PT_DUPLICATE: &str = "Duplicate";
pub const PT_PAINT_MORE: &str = "Paint more";
pub const PT_PLANT: &str = "plant";
pub const PT_PLANTS: &str = "plants";
pub const PT_CAPPED: &str =
    "Capped: more plants would not be drawn. Lower the density or the area.";
pub const PT_VISIBLE: &str = "Visible (its layer)";
pub const PT_HELP_TIP: &str = "Shortcuts of this tool";
pub const PT_SHORTCUTS: &str = "Shortcuts";
pub const PT_NOTHING_TO_ERASE: &str = "Nothing to erase: paint with these settings first";
pub const PT_KEEP_POINTS: &str =
    "A line keeps two points, an area three. Esc, then Delete removes the whole planting";
pub const PT_SELECT_HINT: &str =
    "Click a plant to select its planting; drag its handles to reshape it.";
pub const PT_SINGLE_HINT: &str =
    "Click to place a plant, drag to turn it, Shift+wheel or + / - to scale.";
pub const PT_LINE_HINT: &str = "Click points along the line, Enter or double-click finishes.";
pub const PT_CLUSTER_HINT: &str = "Click the centre of the clump, drag out its radius.";
pub const PT_AREA_HINT: &str = "Click the outline, Enter or the first point closes it.";
pub const PT_SURFACE_HINT: &str = "Click a surface to fill it, Shift-click adds a surface.";
pub const PT_BRUSH_HINT: &str =
    "Press and drag to paint, Alt-drag to erase, [ and ] size the brush.";

pub const PT_KEYS_COMMON: &[(&str, &str)] = &[
    (
        "1 - 7",
        "Select, Single, Line, Cluster, Area, Surface, Brush",
    ),
    ("Esc", "Cancel what is pending, leave the tool, deselect"),
    ("Ctrl+Z", "Undo"),
    ("?", "This list"),
];
pub const PT_KEYS_SELECT: &[(&str, &str)] = &[
    ("Click", "Select the planting of a plant"),
    (
        "Drag handle",
        "Move a point, a centre, a radius, a rotation",
    ),
    ("Click edge", "Insert a point on a line or an area"),
    ("Delete", "Remove the selected point, else the planting"),
    ("Ctrl+D", "Duplicate with an offset and a new seed"),
];
pub const PT_KEYS_SINGLE: &[(&str, &str)] = &[
    ("Click", "Place a plant"),
    ("Drag", "Set its rotation"),
    ("Shift+wheel, + / -", "Scale"),
    ("Ctrl+drag", "Orbit the camera"),
];
pub const PT_KEYS_LINE: &[(&str, &str)] = &[
    ("Click", "Add a point"),
    ("Enter / double-click", "Finish the line"),
    ("Backspace", "Remove the last point"),
    ("Alt", "Snap off"),
];
pub const PT_KEYS_CLUSTER: &[(&str, &str)] = &[
    ("Click", "Place a clump of the default radius"),
    ("Drag", "Drag out the radius from the centre"),
    ("Ctrl+drag", "Orbit the camera"),
];
pub const PT_KEYS_AREA: &[(&str, &str)] = &[
    ("Click", "Add an outline point"),
    ("Enter / first point", "Close and fill the outline"),
    ("Backspace", "Remove the last point"),
    ("Alt", "Snap off"),
];
pub const PT_KEYS_SURFACE: &[(&str, &str)] = &[
    ("Hover", "Highlight the surface under the cursor"),
    ("Click", "Fill it"),
    ("Shift+click", "Add the surface to the last fill"),
];
pub const PT_KEYS_BRUSH: &[(&str, &str)] = &[
    ("Drag", "Paint a stroke (one undo step)"),
    ("Alt+drag", "Erase"),
    ("[  ]", "Brush radius 0.25 - 40 m"),
    ("Ctrl+drag", "Orbit the camera"),
];

/// "<n> lamps have no light: at most <cap> fixtures shine."
pub const LAMPS_OVER_CAP: &str = "lamps stay dark: lights from lamps are capped at";

pub const ALLOW_AGENTS: &str = "Allow AI agents";
pub const ALLOW_AGENTS_TIP: &str = "Lets visioncraft-cli mcp --bridge (an AI agent on this computer) change this project while you watch; Ctrl+Z undoes its steps. Untick to disconnect.";
pub const AGENT_LAST: &str = "agent:";
/// The note while several things are selected (Shift+click adds or removes one).
pub fn selected_many(n: usize) -> String {
    format!("{n} selected (Shift+click adds or removes; Delete removes them all)")
}
pub const WEBMCP: &str = "Experimental: browser AI agent (WebMCP)";
pub const WEBMCP_TIP: &str = "Lets this browser's own AI agent (Chrome's WebMCP) use VisionCraft's tools on this page, films included. Destructive steps ask you first; Ctrl+Z undoes its steps. Untick to take the tools away. Also switched on by ?webmcp=1 in the address.";
pub const WEBMCP_UNSUPPORTED: &str = "this browser offers no WebMCP";
pub const WEBMCP_FAILED: &str = "the browser refused the tools";
pub const WEBMCP_ASK_TITLE: &str = "Browser AI agent asks";
pub const WEBMCP_ALLOW: &str = "Allow";
pub const WEBMCP_DENY: &str = "Deny";
pub const PT_UNDO_STROKE: &str = "Remove last stroke";
pub const PT_UNDO_STROKE_TIP: &str =
    "Takes back this brush's most recent stroke, even after other changes.";

pub const TAB_SUN: &str = "Sun & sky";
pub const TAB_TERRAIN: &str = "Terrain";
pub const TR_RAISE: &str = "Raise";
pub const TR_LOWER: &str = "Lower";
pub const TR_SMOOTH: &str = "Smooth";
pub const TR_FLATTEN: &str = "Flatten";
pub const TR_PAINT: &str = "Paint";
pub const TR_POND: &str = "Pond";
pub const TR_BRUSH_HINT: &str = "Drag over the ground; release to apply. Ctrl+drag turns the view.";
pub const TR_FLATTEN_HINT: &str =
    "Drag from the height you want: the ground under the first point.";
pub const TR_POND_HINT: &str = "Click the pond's outline, then Enter or click the first point: the ground is dug out and filled with water.";
pub const TR_NONE: &str = "This project has no terrain yet: the ground is flat.";
pub const TR_CREATE: &str = "Make terrain";
pub const TR_REMOVE: &str = "Remove terrain";
pub const TR_STRENGTH: &str = "Strength";
pub const TR_DEPTH: &str = "Depth";
pub const TR_LAYERS: &str = "Paint with";
pub const TR_ADD_LAYER: &str = "+ Material from the library";
pub const TR_CELLS: &str = "cells";

pub const SP_OWN: &str = "own";
pub const SP_EDIT: &str = "Edit";
pub const SP_EDIT_TIP: &str = "Make a species of your own from this one.";
pub const SP_TITLE: &str = "Species";
pub const SP_NAME: &str = "Name";
pub const SP_HINT: &str = "The plant at the centre of the view shows each change once you let go.";
pub const SP_SAVE: &str = "Save";
pub const SP_SAVE_NEW: &str = "Save as my species";
pub const SP_SAVED: &str = "Saved: the species is in the plant list of every tool.";
pub const EXPORT_ALL_VIEWS: &str = "All viewpoints";
pub const EXPORT_PHOTO: &str = "Photo";
pub const EXPORT_PHOTO_TIP: &str = "Averages many frames: soft shadows with real penumbrae, smooth edges and smooth shading in corners. Takes seconds to minutes.";
pub const PHOTO_SAMPLES: &str = "frames";
pub const PHOTO_TAKING: &str = "Taking a photo";
pub const PHOTO_FOCUSING: &str = "Focusing…";
pub const RENDER_PHOTO: &str = "Render photo";
pub const PHOTO_APERTURE: &str = "Aperture";
pub const PHOTO_APERTURE_TIP: &str = "Depth of field, as on a full-frame camera with a lens for this view: f/2.8 blurs what lies behind a subject a few metres away, f/8 keeps the garden sharp.";
pub const PHOTO_ALL_SHARP: &str = "All sharp";
pub const PHOTO_F_NUMBERS: [f32; 7] = [1.4, 2.0, 2.8, 4.0, 5.6, 8.0, 16.0];
pub const PHOTO_FOCUS: &str = "Focus";
pub const PHOTO_FOCUS_TIP: &str =
    "Auto: on what the centre of the picture shows. Target: at the point the camera turns round.";
pub const PHOTO_FOCUSES: [(&str, &str); 2] = [("auto", "Auto"), ("target", "Target")];
pub const VIGNETTE: &str = "Vignette";
pub const VIGNETTE_TIP: &str = "The lens' light fall-off towards the corners.";
pub const GRAIN: &str = "Grain";
pub const GRAIN_TIP: &str = "Film grain, most in the mid-tones.";
pub const EXPORT_ALL_TIP: &str = "One picture per saved viewpoint, each with its own time of day: into a folder you pick, or one zip download in the browser.";
pub const EXPORTING_VIEWS: &str = "Rendering every viewpoint at";
pub const CLAY_NOTICE: &str = "Clay view: materials and plant colours are hidden.";
pub const CLAY_OFF: &str = "Show materials";
pub const LAYER_SURROUNDINGS: &str = "Surroundings";
pub const LAYER_SURROUNDINGS_TIP: &str = "Surroundings, not part of the design: they cast shadow and draw as plain grey massing, and stay out of the quantities.";
pub const LAYER_WORD: &str = "Layer";
pub const LAYER_HIDDEN_NOTICE: &str = "is hidden: what you add there is not drawn.";
pub const LAYER_SHOW: &str = "Show it";
pub const LAYERS_HIDDEN_ON_OPEN: &str = "Hidden layers in this project (Layers to show them)";
pub const SP_MY: &str = "My";
pub const OBJECT_IMPORTED: &str = "is in the plant browser under Objects: place it with any tool.";
pub const OBJECTS_MISSING: &str =
    "Imported objects without their file (open the .visioncraft.zip, or import the .glb again):";
pub const OBJECT_UNREADABLE: &str = "the object file in the bundle cannot be read";
pub const OBJECT_MENU_HINT: &str = "Right-click to rename or remove this imported object.";

pub const SP_LOOK_DRAWN: &str = "Leaves and bark are drawn (set Leaves and bark to scanned for photographed ones where the library has them).";
pub const SP_SCAN_OWN: &str = "scanned, its own genus,";
pub const SP_SCAN_ALIKE: &str =
    "scanned look-alike (same leaf shape or bark pattern, another genus),";
pub const SP_SCAN_NONE: &str = "drawn";
pub const SP_LEAVES: &str = "Leaves";
pub const SP_BARK: &str = "Bark";

// Sea & water (water_panel.rs).
pub const WA_HEADER: &str = "Sea & water";
pub const WA_ON: &str = "Sea level";
pub const WA_HINT: &str =
    "Water at one height out to the horizon: the terrain and ground above it make the coast.";
pub const WA_SEA: &str = "Sea";
pub const WA_LAKE: &str = "Lake";
pub const WA_RIVER: &str = "River";
pub const WA_LEVEL: &str = "Level";
pub const WA_STATE: &str = "Sea state";
pub const WA_HEIGHT: &str = "Wave height";
pub const WA_FOLLOW_WIND: &str = "Waves follow the wind";
pub const WA_DIRECTION: &str = "Wave direction";
pub const WA_FLOW_DIR: &str = "Flow direction";
pub const WA_FLOW: &str = "Flow";
pub const WA_CLARITY: &str = "Clarity";
pub const WA_FOAM: &str = "Foam";
pub const WA_COLOR: &str = "Deep colour";
pub const WA_BEACH: &str = "Beach across the view";
pub const WA_BEACH_HINT: &str =
    "A sandy beach along a line through the middle of the view, the sea beyond it.";

// ---- plan underlay ----
pub const TOOL_PLAN: &str = "Plan image";
pub const PLAN_ADD: &str = "Add plan image…";
pub const PLAN_NONE: &str =
    "No plan image yet. Add a PNG or JPEG of a floor plan or site plan to trace over.";
pub const PLAN_IMPORTED: &str =
    "is under the drawing as a plan image: set its scale with Set scale or Calibrate.";
pub const PLAN_MISSING: &str =
    "Plan image files not found (open the project's bundle, or put them beside the project file):";
pub const PLAN_UNREADABLE: &str = "the plan image in the bundle cannot be read";
pub const PLAN_FILE_MISSING: &str = "Image file not found";
pub const PLAN_SHOW: &str = "Show";
pub const PLAN_OPACITY: &str = "Opacity";
pub const PLAN_REMOVE: &str = "Remove";
pub const PLAN_SET_SCALE: &str = "Set scale";
pub const PLAN_SET_SCALE_TIP: &str =
    "Click two points on the image, then type the real distance between them.";
pub const PLAN_CALIBRATE: &str = "Calibrate";
pub const PLAN_CALIBRATE_TIP: &str = "Click two points on the image, then the two points in the drawing they must land on: scale, turn and position follow.";
pub const PLAN_DISTANCE: &str = "Distance";
pub const PLAN_APPLY: &str = "Apply";
pub const PLAN_CANCEL: &str = "Cancel";
pub const PLAN_SCALE_NOW: &str = "m per pixel";
pub const PLAN_HINT_IDLE: &str = "Set scale or Calibrate to fit the image to the drawing. Then draw over it with the other tools.";
pub const PLAN_HINT_A: &str =
    "Click the first point on the image (an end of a dimension you know).";
pub const PLAN_HINT_B: &str = "Click the second point on the image.";
pub const PLAN_HINT_DISTANCE: &str =
    "Type the real distance between the two points, then Apply (Enter).";
pub const PLAN_HINT_WORLD_A: &str =
    "Click where the first point must land in the drawing (it snaps to the grid and to corners).";
pub const PLAN_HINT_WORLD_B: &str = "Click where the second point must land in the drawing.";

// ---- photos on walls ----
pub const WALL_PHOTO_BUTTON: &str = "Photo on wall…";
pub const WALL_PHOTO_EDIT: &str = "Straighten photo…";
pub const WALL_PHOTO_REMOVE: &str = "Remove photo";
pub const WALL_PHOTO_TITLE: &str = "Photo on wall";
pub const WALL_PHOTO_HINT: &str = "Drag the four handles onto the corners of the facade in the photo: 1 bottom left, 2 bottom right, 3 top right, 4 top left, as seen from outside. The photo is straightened to fit.";
pub const WALL_PHOTO_LOADING: &str = "Loading the photo…";
pub const WALL_PHOTO_WHOLE: &str = "Covers the whole wall";
pub const WALL_PHOTO_REGION: &str =
    "Where on the wall, in metres from its lower left corner (seen from outside):";
pub const WALL_PHOTO_LEFT: &str = "from left";
pub const WALL_PHOTO_RIGHT: &str = "to";
pub const WALL_PHOTO_BOTTOM: &str = "from bottom";
pub const WALL_PHOTO_TOP: &str = "to";
pub const WALL_PHOTO_WALL_IS: &str = "The wall is";
pub const WALL_PHOTO_APPLY: &str = "Apply";
pub const WALL_PHOTO_CANCEL: &str = "Cancel";
pub const WALL_PHOTO_BAD_REGION: &str =
    "The region needs four numbers in metres, with the right and top above the left and bottom.";
pub const WALL_PHOTO_MISSING: &str =
    "Wall photo files not found (open the project's bundle, or put them beside the project file):";
pub const WALL_PHOTO_UNREADABLE: &str = "the wall photo in the bundle cannot be read";

pub const FILM: &str = "Film";
pub const FILM_NEW: &str = "New film";
pub const FILM_DELETE: &str = "Delete film";
pub const FILM_NONE: &str = "No film yet: start one, then add shots.";
pub const FILM_NO_SHOTS: &str = "No shots yet.";
pub const FILM_PLAY: &str = "Play";
pub const FILM_PLAY_SHOT: &str = "Play shot";
pub const FILM_PLAY_FROM_SHOT: &str = "Play from shot";
pub const FILM_STRIP_TIP: &str =
    "The shots on the timeline: click one to select it and jump to its start, drag to scrub.";
pub const FILM_PAUSE: &str = "Pause";
pub const FILM_HOLD_HERE: &str = "Add the current view as a shot";
pub const FILM_ADD_SHOT: &str = "Add shot";
pub const FILM_WALK: &str = "Walk";
pub const FILM_ORBIT: &str = "Orbit";
pub const FILM_FLYOVER: &str = "Flyover";
pub const FILM_PATH: &str = "Path (x z, x z, …)";
pub const FILM_SPEED: &str = "Speed";
pub const FILM_EYE_HEIGHT: &str = "Eye height";
pub const FILM_HEIGHT: &str = "Height";
pub const FILM_RADIUS: &str = "Radius";
pub const FILM_CENTER: &str = "Centre";
pub const FILM_CENTER_HERE: &str = "Centre on the view target";
pub const FILM_FROM_DEG: &str = "From";
pub const FILM_TO_DEG: &str = "To";
pub const FILM_DURATION: &str = "Duration";
pub const FILM_PATH_BAD: &str =
    "The path needs at least two points, each an x and a z in metres, such as `10 12.5, 10 26`.";
pub const FILM_ORBIT_TIP: &str = "Degrees from east of the centre, counter-clockwise seen from above; more than 360 turns more than once.";
pub const FILM_RENDER_HINT: &str =
    "Frames: visioncraft-cli film FILE --out DIR, or the command film.render.";

pub const BORDER_TITLE: &str = "Border";
pub const BORDER_MAKE: &str = "Make border";
pub const BORDER_MAKE_TIP: &str = "Plants a layered perennial border over the area: tall species at the back, short at the front, in drifts that close up as they grow";
pub const BORDER_AREA: &str = "Area";
pub const BORDER_NO_AREAS: &str = "Draw an area first (Draw mode), then plant a border over it";
pub const BORDER_STYLE: &str = "Style";
pub const BORDER_STYLES: &[(&str, &str)] = &[
    ("cottage", "Cottage"),
    ("prairie", "Prairie"),
    ("shade", "Shade"),
    ("dry", "Dry"),
];
pub const BORDER_HEIGHT: &str = "Height front / back";
pub const BORDER_HEIGHT_TIP: &str =
    "The mature plant height wanted at the front edge and at the back edge";
pub const BORDER_DRIFT: &str = "Drift length";
pub const BORDER_SPECIES: &str = "species";
