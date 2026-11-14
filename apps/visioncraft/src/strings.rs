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
pub const WEATHERING: &str = "Weathering";
pub const MATERIALS_HINT: &str =
    "Hover a surface and click it to choose a material. Drop a .mcmat file to add one.";
pub const MATERIAL_ADDED: &str = "Material added";

// Objects
pub const DENSITY: &str = "Density per m²";
pub const PLANTINGS: &str = "Placed";
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
    "Drag the sun around the compass for the time of day, click the month dial for the season.";
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
pub const CLOUD_KINDS: [(&str, &str); 3] = [
    ("clear", "Clear"),
    ("cumulus", "Fair"),
    ("overcast", "Overcast"),
];
pub const CLOUD_COVER: &str = "Cover";
pub const LOOKS: [(&str, &str); 3] = [
    ("neutral", "Neutral"),
    ("natural", "Natural"),
    ("vivid", "Vivid"),
];
pub const HAZE: &str = "Haze";
pub const WIND: &str = "Wind";
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
pub const TR_POND_NAME: &str = "Pond";
pub const TR_POND_EDGE: &str = "Pond edge";
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
pub const EXPORT_ALL_TIP: &str = "One picture per saved viewpoint, each with its own time of day: into a folder you pick, or one zip download in the browser.";
pub const EXPORTING_VIEWS: &str = "Rendering every viewpoint at";
pub const CLAY_NOTICE: &str = "Clay view: materials and plant colours are hidden.";
pub const CLAY_OFF: &str = "Show materials";
pub const SP_MY: &str = "My";
pub const OBJECT_IMPORTED: &str = "is in the plant browser under Objects: place it with any tool.";
pub const OBJECTS_MISSING: &str =
    "Imported objects without their file (open the .visioncraft.zip, or import the .glb again):";
pub const OBJECT_UNREADABLE: &str = "the object file in the bundle cannot be read";
pub const OBJECT_MENU_HINT: &str = "Right-click to rename or remove this imported object.";
