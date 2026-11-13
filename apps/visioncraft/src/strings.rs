//! Every piece of UI text, in one place so it can be translated later.

pub const APP_NAME: &str = "VisionCraft";

pub const OPEN: &str = "Open";
pub const SAVE: &str = "Save";
pub const SAVED: &str = "Saved";
pub const UNDO: &str = "Undo";
pub const REDO: &str = "Redo";
pub const LAYERS: &str = "Layers";
pub const CLAY: &str = "Clay";
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
pub const DROP_HINT: &str = "Open or drop a .3dcraft or .obj model, or draw with the Draw tool";
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
pub const TOOL_SINGLE: &str = "Single";
pub const TOOL_SCATTER: &str = "Area scatter";
pub const DENSITY: &str = "Density per m²";
pub const SINGLE_HINT: &str = "Click the ground to place one.";
pub const SCATTER_HINT: &str = "Click points around the area, then Enter or click the first point.";
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
pub const VIEWS_FULL: &str = "Eight viewpoints are saved. Delete one to add another.";

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
pub const HAZE: &str = "Haze";
pub const WIND: &str = "Wind";
pub const WIND_DIRECTION: &str = "Wind direction";
pub const GROUND: &str = "Ground plane";
pub const EXPORT: &str = "Export";
pub const NEW: &str = "New";
pub const AUTO: &str = "auto";
pub const REPLACE_PROJECT: &str = "Replace the current project? Unsaved changes are lost.";
pub const REPLACE_ANYWAY: &str = "Replace";
pub const CANCEL: &str = "Cancel";
pub const SEARCH: &str = "Search";
pub const EVERGREEN: &str = "Evergreen";
pub const IN_BLOOM: &str = "In bloom now";
pub const PALETTE: &str = "Palette";

// ---- Planting tools (Objects mode) ----
pub const PT_SELECT: &str = "Select";
pub const PT_SINGLE: &str = "Single";
pub const PT_LINE: &str = "Line";
pub const PT_CLUSTER: &str = "Cluster";
pub const PT_AREA: &str = "Area";
pub const PT_SURFACE: &str = "Surface";
pub const PT_BRUSH: &str = "Brush";
pub const PT_SIMPLE: &str = "Simple";
pub const PT_ADD_PLANTS: &str = "+ Add plants";
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
pub const PT_SINGLE_HINT: &str = "Click to place a plant, drag to turn it, Shift+wheel to scale.";
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
    ("Shift+wheel", "Scale"),
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
